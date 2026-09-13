use std::path::PathBuf;

use native_dev::{locate_binaries, locate_in_dir};

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
    let directory = fixture_bin_dir("bin dir with spaces");
    let set = locate_binaries(Some(&directory)).expect("binaries locate");
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
