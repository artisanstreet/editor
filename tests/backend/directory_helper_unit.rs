//! Private white-box tests for the internal directory helper: the chooser
//! seam, operation mapping, real path policy, and argument dispatch.
//!
//! This file is contract-named and compiled as a private `cfg(test)` module
//! of the backend crate through the supplied backend-crate `rust_test`
//! wiring, so it may reach `pub(crate)` items via `crate::` paths. No test
//! here opens a native chooser: every `Pick` flows through an injected
//! [`DirectoryChooser`] double, and `NativeDirectoryChooser` is never
//! constructed.

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use crate::directory_helper::{
    ArgumentDispatch, DirectoryChooser, INTERNAL_DIRECTORY_HELPER_FLAG, PickFailure,
    ResolutionFailure, classify_arguments, perform_operation, resolve_selected_directory,
};
use crate::directory_helper_codec::{HelperRequest, Response};

static TEMP_SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// A uniquely named temporary directory removed again on drop.
struct TemporaryDirectory {
    path: PathBuf,
}

impl TemporaryDirectory {
    fn new(label: &str) -> Self {
        let sequence = TEMP_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "artisan-dir-helper {label} héllo 🦈 {}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("temporary directory should be created");
        Self { path }
    }

    fn text(&self) -> String {
        self.path
            .to_str()
            .expect("temporary directory should be valid unicode")
            .to_owned()
    }
}

impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        let _cleanup_result = fs::remove_dir_all(&self.path);
    }
}

#[derive(Debug)]
struct CancelledChooser;

impl DirectoryChooser for CancelledChooser {
    fn choose_directory(&self) -> Result<Option<PathBuf>, PickFailure> {
        Ok(None)
    }
}

#[derive(Debug)]
struct FailingChooser;

impl DirectoryChooser for FailingChooser {
    fn choose_directory(&self) -> Result<Option<PathBuf>, PickFailure> {
        Err(PickFailure::Failed)
    }
}

#[derive(Debug)]
struct UnsupportedChooser;

impl DirectoryChooser for UnsupportedChooser {
    fn choose_directory(&self) -> Result<Option<PathBuf>, PickFailure> {
        Err(PickFailure::UnsupportedPlatform)
    }
}

#[derive(Debug)]
struct FixedSelection(PathBuf);

impl DirectoryChooser for FixedSelection {
    fn choose_directory(&self) -> Result<Option<PathBuf>, PickFailure> {
        Ok(Some(self.0.clone()))
    }
}

#[test]
fn cancelled_failed_and_unsupported_choosers_map_to_distinct_tags() {
    let pick = HelperRequest::Pick;
    assert_eq!(
        perform_operation(&pick, &CancelledChooser),
        Response::Cancelled
    );
    assert_eq!(
        perform_operation(&pick, &FailingChooser),
        Response::DialogFailed
    );
    assert_eq!(
        perform_operation(&pick, &UnsupportedChooser),
        Response::UnsupportedPlatform
    );
}

#[test]
fn selected_choice_resolves_through_real_policy() {
    let temporary = TemporaryDirectory::new("selected");
    let selection = FixedSelection(temporary.path.clone());

    let response = perform_operation(&HelperRequest::Pick, &selection);
    match response {
        Response::Selected { canonical_path } => {
            let independent = std::fs::canonicalize(&temporary.path)
                .expect("independent canonicalization should succeed");
            assert_eq!(Path::new(&canonical_path), independent.as_path());
        }
        unexpected => panic!("real directory choice must select, got {unexpected:?}"),
    }

    let anchor = TemporaryDirectory::new("validate-wiring");
    let missing = anchor.path.join("still-missing");
    let response = perform_operation(
        &HelperRequest::Validate {
            path_text: missing.to_str().expect("anchor stays unicode").to_owned(),
        },
        &CancelledChooser,
    );
    assert_eq!(response, Response::InvalidPath);
}

#[cfg(windows)]
#[test]
fn selections_beyond_lossless_utf8_become_encoding_failures() {
    use std::os::windows::ffi::OsStringExt;

    // A lone surrogate cannot cross as lossless UTF-8; the failure must be
    // classified before any filesystem access happens.
    let beyond_utf8 = PathBuf::from(std::ffi::OsString::from_wide(&[
        0x005C, 0x005C, 0x003F, 0x005C, 0x0043, 0x003A, 0x005C, 0xD800,
    ]));

    let response = perform_operation(&HelperRequest::Pick, &FixedSelection(beyond_utf8));
    assert_eq!(response, Response::UnsupportedEncoding);
}

#[test]
fn temporary_directories_with_spaces_and_unicode_resolve() {
    let temporary = TemporaryDirectory::new("resolve");
    let resolved = resolve_selected_directory(&temporary.text())
        .expect("a real created directory must resolve");

    assert!(Path::new(&resolved).is_absolute());
    assert!(
        fs::metadata(&resolved)
            .expect("resolved root should still exist")
            .is_dir()
    );
    let independent = std::fs::canonicalize(&temporary.path)
        .expect("independent canonicalization should succeed");
    assert_eq!(Path::new(&resolved), independent.as_path());
}

#[test]
fn relative_empty_and_missing_candidates_are_invalid_paths() {
    assert_eq!(
        resolve_selected_directory(""),
        Err(ResolutionFailure::InvalidPath)
    );
    assert_eq!(
        resolve_selected_directory("some/relative/path"),
        Err(ResolutionFailure::InvalidPath)
    );

    let anchor = TemporaryDirectory::new("missing-anchor");
    let missing = anchor.path.join("definitely-missing-child");
    let missing_text = missing.to_str().expect("anchor stays unicode");
    assert_eq!(
        resolve_selected_directory(missing_text),
        Err(ResolutionFailure::InvalidPath)
    );
}

#[test]
fn existing_files_are_invalid_roots() {
    let anchor = TemporaryDirectory::new("file-root");
    let file = anchor.path.join("regular-file.txt");
    fs::write(&file, b"not a directory").expect("temporary file should be written");
    let file_text = file.to_str().expect("anchor stays unicode");

    assert_eq!(
        resolve_selected_directory(file_text),
        Err(ResolutionFailure::InvalidPath)
    );
}

#[cfg(windows)]
#[test]
fn device_and_verbatim_relative_prefixes_are_refused_before_filesystem_access() {
    // Device namespace and verbatim-relative prefixes fail at the prefix
    // gate, so these probes never touch the filesystem. The canonical result
    // of a valid selection is exercised by the resolving tests above; on
    // Windows it carries an allowed VerbatimDisk prefix.
    assert_eq!(
        resolve_selected_directory(r"\\.\PhysicalDrive0"),
        Err(ResolutionFailure::InvalidPath)
    );
    assert_eq!(
        resolve_selected_directory(r"\\?\artisan-helper-relative-probe"),
        Err(ResolutionFailure::InvalidPath)
    );
}

#[test]
fn argument_dispatch_accepts_only_the_exact_sole_flag() {
    let none: [&OsStr; 0] = [];
    assert_eq!(classify_arguments(none), ArgumentDispatch::NotRequested);

    assert_eq!(
        classify_arguments([OsStr::new(INTERNAL_DIRECTORY_HELPER_FLAG)]),
        ArgumentDispatch::RunHelper
    );

    assert_eq!(
        classify_arguments([
            OsStr::new(INTERNAL_DIRECTORY_HELPER_FLAG),
            OsStr::new("--other"),
        ]),
        ArgumentDispatch::RejectInvocation
    );
    assert_eq!(
        classify_arguments([
            OsStr::new(INTERNAL_DIRECTORY_HELPER_FLAG),
            OsStr::new(INTERNAL_DIRECTORY_HELPER_FLAG),
        ]),
        ArgumentDispatch::RejectInvocation
    );
    assert_eq!(
        classify_arguments([OsStr::new("--other")]),
        ArgumentDispatch::NotRequested
    );
}

#[cfg(windows)]
#[test]
fn arguments_beyond_unicode_never_match_or_abort_dispatch() {
    use std::os::windows::ffi::OsStringExt;

    // A lone surrogate is not valid UTF-16 text for a flag; byte-exact
    // matching simply refuses it without panicking.
    let beyond_unicode = std::ffi::OsString::from_wide(&[0xD800]);
    assert_eq!(
        classify_arguments([beyond_unicode]),
        ArgumentDispatch::NotRequested
    );
}
