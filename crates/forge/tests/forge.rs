use std::process::Command;

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

#[test]
fn forge_rotates_a_full_log_into_an_archive() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    std::fs::create_dir_all(&data).unwrap();
    let _lock = forge_http::lock_data_dir(&data).unwrap();
    let log = dir.path().join("forge.log");
    let full = vec![b'x'; 16 * 1024 * 1024];
    std::fs::write(&log, &full).unwrap();
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
    assert!(text.len() < 64 * 1024, "the old log moved out of forge.log");
    assert_eq!(
        std::fs::read(dir.path().join("forge.log.1")).unwrap().len(),
        full.len(),
        "the full log is kept as the first archive"
    );
}
