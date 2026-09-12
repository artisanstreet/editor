//! Binary discovery for `bazel run //:dev`.
//!
//! The launcher must find the four Bazel-built binaries without a wrapper
//! script: explicit `--bin-dir` first, then the Bazel runfiles directory
//! (Bzlmod `_main`, legacy workspace, and flat layouts), then the runfiles
//! manifest with the same prefixes, then the `bazel-bin` sibling layout.

use std::path::PathBuf;

use native_dev::{find_in_manifest, find_prefixed_in_manifest, locate_in_dir, runfiles_candidates};

fn fixture_bin_dir(case: &str) -> PathBuf {
    let root =
        std::env::temp_dir().join(format!("artisan-native-dev-{case}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("fixture bin dir");
    for stem in ["ae", "editor", "forge", "installer"] {
        std::fs::write(
            root.join(native_dev::exe_name(stem)),
            format!("fixture-{stem}"),
        )
        .expect("fixture binary");
    }
    root
}

fn cleanup(path: &std::path::Path) {
    let _ = std::fs::remove_dir_all(path);
}

#[test]
fn explicit_directory_locates_all_four_binaries() {
    let directory = fixture_bin_dir("bindir");
    let set = locate_in_dir(&directory).expect("binaries locate");
    assert_eq!(set.forge, directory.join(native_dev::exe_name("forge")));
    assert_eq!(set.editor, directory.join(native_dev::exe_name("editor")));
    assert_eq!(set.ae, directory.join(native_dev::exe_name("ae")));
    assert_eq!(
        set.installer,
        directory.join(native_dev::exe_name("installer"))
    );
    cleanup(&directory);
}

#[test]
fn explicit_directory_reports_every_missing_binary() {
    let directory = fixture_bin_dir("bindir-missing");
    std::fs::remove_file(directory.join(native_dev::exe_name("editor"))).expect("remove editor");
    std::fs::remove_file(directory.join(native_dev::exe_name("forge"))).expect("remove forge");
    let error = locate_in_dir(&directory).expect_err("missing binaries fail");
    let message = error.to_string();
    assert!(
        message.contains(&native_dev::exe_name("editor")),
        "unexpected: {message}"
    );
    assert!(
        message.contains(&native_dev::exe_name("forge")),
        "unexpected: {message}"
    );
    assert!(message.contains("--bin-dir"), "unexpected: {message}");
    cleanup(&directory);
}

#[test]
fn manifest_lookup_matches_exact_runfile_names() {
    let manifest = if cfg!(windows) {
        "artisan_editor/modules/backend/forge.exe C:/bazel/out/forge.exe\n\
         artisan_editor/modules/frontend/editor.exe C:/bazel/out/editor.exe\n"
    } else {
        "artisan_editor/modules/backend/forge /bazel/out/forge\n\
         artisan_editor/modules/frontend/editor /bazel/out/editor\n"
    };
    let forge = if cfg!(windows) {
        "artisan_editor/modules/backend/forge.exe"
    } else {
        "artisan_editor/modules/backend/forge"
    };
    assert_eq!(
        find_in_manifest(manifest, forge),
        Some(if cfg!(windows) {
            PathBuf::from("C:/bazel/out/forge.exe")
        } else {
            PathBuf::from("/bazel/out/forge")
        })
    );
    assert_eq!(
        find_in_manifest(manifest, "artisan_editor/modules/cli/ae"),
        None
    );
    assert_eq!(find_in_manifest("", forge), None);
    assert_eq!(
        find_in_manifest("malformed-line-without-space", forge),
        None
    );
}

#[test]
fn manifest_lookup_rejects_prefix_impostors() {
    let manifest = "artisan_editor/modules/backend/forge-extra /bazel/out/other\n";
    assert_eq!(
        find_in_manifest(manifest, "artisan_editor/modules/backend/forge"),
        None
    );
}

#[test]
fn manifest_lookup_supports_bzlmod_main_and_spaces() {
    let manifest = if cfg!(windows) {
        "_main/modules/backend/forge.exe C:/Program Files/artisan/out/forge.exe\n\
         artisan_editor/modules/backend/forge.exe C:/other/forge.exe\n"
    } else {
        "_main/modules/backend/forge /opt/artisan dir/forge\n\
         artisan_editor/modules/backend/forge /other/forge\n"
    };
    let found = find_prefixed_in_manifest(manifest, "modules/backend/forge")
        .expect("bzlmod entry resolves");
    assert_eq!(
        found,
        if cfg!(windows) {
            PathBuf::from("C:/Program Files/artisan/out/forge.exe")
        } else {
            PathBuf::from("/opt/artisan dir/forge")
        }
    );
}

#[test]
fn manifest_lookup_falls_back_through_prefixes() {
    let manifest = "artisan_editor/modules/backend/forge /fallback/forge\n";
    assert_eq!(
        find_prefixed_in_manifest(manifest, "modules/backend/forge"),
        Some(PathBuf::from("/fallback/forge"))
    );
    assert_eq!(
        find_prefixed_in_manifest(manifest, "modules/backend/editor"),
        None
    );
}

#[test]
fn runfiles_candidates_cover_workspace_layouts() {
    let candidates = runfiles_candidates("modules/backend/forge", "/runfiles");
    assert_eq!(candidates.len(), 3);
    assert!(candidates[0].starts_with("/runfiles"));
    assert!(candidates[0].to_string_lossy().contains("_main"));
    assert!(candidates[1].to_string_lossy().contains("artisan_editor"));
    for candidate in &candidates {
        assert!(candidate.ends_with(native_dev::exe_name("forge")));
    }
}
