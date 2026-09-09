//! Binary discovery for `bazel run //:dev`.
//!
//! The launcher must find the four Bazel-built binaries without a wrapper
//! script: explicit `--bin-dir` first, then the Bazel runfiles directory,
//! then the runfiles manifest, then the `bazel-bin` sibling layout.

use std::path::PathBuf;

use native_dev::{find_in_manifest, locate_in_dir, runfiles_candidates};

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
fn runfiles_candidates_cover_workspace_layouts() {
    let candidates = runfiles_candidates("modules/backend/forge", "/runfiles");
    assert_eq!(candidates.len(), 2);
    assert!(candidates[0].starts_with("/runfiles"));
    assert!(candidates[0].to_string_lossy().contains("artisan_editor"));
    assert!(candidates[0].ends_with(native_dev::exe_name("forge")));
    assert!(candidates[1].ends_with(native_dev::exe_name("forge")));
}
