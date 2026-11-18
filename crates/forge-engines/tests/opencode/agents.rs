use super::*;
use std::sync::atomic::{AtomicBool, Ordering};

#[test]
fn a_starting_server_never_reads_a_partial_agent_configuration() {
    let home = tempfile::tempdir().unwrap();
    let readable = [
        vec![home.path().join("blobs-a")],
        vec![home.path().join("blobs-b"), home.path().join("blobs-c")],
    ];
    let config = write_agents(home.path(), &readable[0])
        .unwrap()
        .join("opencode.json");
    let writing = AtomicBool::new(true);
    std::thread::scope(|scope| {
        scope.spawn(|| {
            for round in 0..400 {
                write_agents(home.path(), &readable[round % 2]).unwrap();
            }
            writing.store(false, Ordering::Release);
        });
        let mut reads = 0_u32;
        while writing.load(Ordering::Acquire) || reads == 0 {
            let bytes = std::fs::read(&config).unwrap();
            let parsed: Result<Value, _> = serde_json::from_slice(&bytes);
            assert!(
                parsed.is_ok_and(|config| config["agent"].is_object()),
                "OpenCode read {} bytes of a half-written configuration",
                bytes.len()
            );
            reads += 1;
        }
    });
}
