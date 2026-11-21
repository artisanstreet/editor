use std::process::Command;

const LOG: &str = "OPEN_TIMING_TEST_LOG";

#[test]
fn open_timing_lands_in_the_editor_log() {
    if let Some(log) = std::env::var_os(LOG) {
        let file = std::fs::File::create(log).unwrap();
        diagnostics::subscribe("info", std::sync::Mutex::new(file)).unwrap();
        editor_core::diagnostics::open_timing::begin();
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let log = directory.path().join("editor.log");
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "open_timing::open_timing_lands_in_the_editor_log",
            "--exact",
            "--test-threads=1",
        ])
        .env(LOG, &log)
        .env("AQUA_OPEN_TIMING", "1")
        .env_remove("RUST_LOG")
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let text = std::fs::read_to_string(&log).unwrap();
    assert!(text.contains("open-timing: stage=open_thread"), "{text}");
}
