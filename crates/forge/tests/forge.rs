//! The `forge` binary as `ae forge start` and the NixOS service run it.

use std::process::Command;

/// Two Forges on one data folder would corrupt it: the second refuses, says
/// why in its log, and exits.
#[test]
fn a_second_forge_on_the_same_data_exits_and_logs_why() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    std::fs::create_dir_all(&data).unwrap();
    let _lock = forge_http::lock_data_dir(&data).unwrap();
    let log = dir.path().join("forge.log");
    let status = Command::new(env!("CARGO_BIN_EXE_forge"))
        .arg("--data")
        .arg(&data)
        .args(["--listen", "127.0.0.1:0", "--log"])
        .arg(&log)
        .status()
        .unwrap();
    assert!(!status.success());
    let text = std::fs::read_to_string(&log).unwrap();
    assert!(text.contains("another Forge is using"), "{text}");
}
