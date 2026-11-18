use super::Decoder;
use crate::EngineEvent;
use serde_json::{Value, json};

fn root() -> Decoder {
    let mut decoder = Decoder::default();
    let id = decoder.request("thread/start", json!({}));
    decoder
        .feed(&json!({"id": id, "result": {"thread": {"id": "root"}}}).to_string())
        .unwrap();
    decoder
}

fn command(n: usize, method: &str, status: &str) -> Value {
    json!({"method": method, "params": {"threadId": "root", "item": {
        "type": "commandExecution", "id": format!("c{n}"), "status": status,
        "command": "/bin/bash -lc ls", "aggregatedOutput": "ok", "exitCode": 0
    }}})
}

#[test]
fn a_long_run_keeps_only_a_bounded_record_of_settled_items() {
    let mut decoder = root();
    for n in 0..10_000 {
        decoder
            .feed(&command(n, "item/started", "inProgress").to_string())
            .unwrap();
        decoder
            .feed(&command(n, "item/completed", "completed").to_string())
            .unwrap();
    }
    assert!(
        decoder.items.len() < 16,
        "{} items kept",
        decoder.items.len()
    );
    assert!(decoder.retired.len() <= 4096);
    let replayed = decoder
        .feed(&command(9_999, "item/completed", "completed").to_string())
        .unwrap();
    assert!(
        !replayed
            .iter()
            .any(|event| matches!(event, EngineEvent::Item { key, .. } if key == "c9999")),
        "a replayed completion of a settled command must not reopen it: {replayed:?}"
    );
}
