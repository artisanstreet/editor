use std::path::Path;
use std::process::Command;

fn launch_without_fixture(data: &Path, temporary: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_editor-aqua"))
        .arg("--source-encoded")
        .env("ARTISAN_EDITOR_DATA", data)
        .env_remove("ARTISAN_AQUA_FIXTURE_DIR")
        .env_remove("RUST_LOG")
        .env("TMPDIR", temporary)
        .env("TMP", temporary)
        .env("TEMP", temporary)
        .output()
        .unwrap()
}

#[test]
fn a_launch_that_fails_before_the_window_opens_is_in_the_editor_log() {
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("data");
    let output = launch_without_fixture(&data, root.path());
    assert!(!output.status.success(), "the launch must fail");
    let log = std::fs::read_to_string(data.join("logs").join("editor.log")).unwrap();
    assert!(
        log.contains(
            "the Editor could not launch: qualification requires ARTISAN_AQUA_FIXTURE_DIR"
        ),
        "{log}"
    );
}

#[test]
fn an_unusable_log_folder_falls_back_to_a_temporary_log_that_names_the_cause() {
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("data");
    std::fs::write(&data, b"").unwrap();
    let output = launch_without_fixture(&data, root.path());
    assert!(!output.status.success(), "the launch must fail");
    let fallback = root
        .path()
        .join("artisan-editor")
        .join("logs")
        .join("editor.log");
    let log = std::fs::read_to_string(&fallback).unwrap();
    assert!(
        log.contains("the Editor logs to a temporary folder"),
        "{log}"
    );
    assert!(
        log.contains(&format!(
            "create the log folder {}",
            data.join("logs").display()
        )),
        "{log}"
    );
    assert!(log.contains("the Editor could not launch"), "{log}");
}
