use super::Decoder;
use crate::EngineEvent;
use serde_json::{Value, json};

const SESSION: &str = "ses_root";

fn frame(n: usize, kind: &str, data: &Value) -> String {
    json!({"id": format!("evt_{n}_{kind}"), "type": kind, "data": data}).to_string()
}

fn called(n: usize) -> String {
    frame(
        n,
        "session.tool.called",
        &json!({"sessionID": SESSION, "id": format!("call_{n}"), "input": {"command": "ls"}}),
    )
}

#[test]
fn a_long_run_keeps_only_a_bounded_record_of_settled_tools() {
    let mut decoder = Decoder::new(Some(SESSION.to_owned()));
    for n in 0..10_000 {
        let call = format!("call_{n}");
        decoder
            .feed(&frame(
                n,
                "session.tool.input.started",
                &json!({"sessionID": SESSION, "id": call, "name": "shell"}),
            ))
            .unwrap();
        decoder.feed(&called(n)).unwrap();
        decoder
            .feed(&frame(
                n,
                "session.tool.success",
                &json!({"sessionID": SESSION, "id": call, "metadata": {"exit": 0},
                    "content": [{"type": "text", "text": "ok"}]}),
            ))
            .unwrap();
    }
    assert!(
        decoder.tools.len() < 16,
        "{} tools kept",
        decoder.tools.len()
    );
    assert!(
        decoder.tool_names.len() < 16,
        "{} tool names kept",
        decoder.tool_names.len()
    );
    let replayed = decoder.feed(&called(9_999)).unwrap();
    assert!(
        !replayed
            .iter()
            .any(|event| matches!(event, EngineEvent::Item { key, .. } if key == "call_9999")),
        "a replayed call to a settled tool must not reopen it: {replayed:?}"
    );
}
