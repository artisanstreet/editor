use super::decoder::Decoder;
use crate::{EngineEvent, LiveKind};
use serde_json::{Value, json};

fn statuses(decoder: &mut Decoder, id: u32, kind: &str, data: &Value) -> Vec<String> {
    decoder
        .feed(&json!({ "id": format!("evt_{id}"), "type": kind, "data": data }).to_string())
        .unwrap()
        .into_iter()
        .filter_map(|event| {
            if let EngineEvent::Live {
                target: LiveKind::Status,
                text,
                ..
            } = event
            {
                Some(text)
            } else {
                None
            }
        })
        .collect()
}

#[test]
fn the_wait_is_announced_only_between_turns_while_a_shell_runs() {
    let session = "ses_root";
    let call = "call_1";
    let mut decoder = Decoder::new(Some(session.to_owned()));
    let root = json!({ "sessionID": session });
    let mut seen = Vec::new();
    for (id, kind, data) in [
        (0, "session.execution.started", root.clone()),
        (
            1,
            "session.tool.input.started",
            json!({ "sessionID": session, "id": call, "name": "shell" }),
        ),
        (
            2,
            "session.tool.called",
            json!({ "sessionID": session, "id": call, "input": { "command": "sleep 60", "background": true } }),
        ),
        (
            3,
            "session.tool.success",
            json!({ "sessionID": session, "id": call,
                "metadata": { "status": "running", "shellID": "sh_1", "truncated": false },
                "content": [{ "type": "text", "text": "Command moved to the background (shell ID: sh_1)" }] }),
        ),
    ] {
        seen.extend(statuses(&mut decoder, id, kind, &data));
    }
    assert!(
        seen.is_empty(),
        "the model is still in its turn, so it is not waiting: {seen:?}"
    );
    assert_eq!(
        statuses(&mut decoder, 4, "session.execution.succeeded", &root),
        ["OpenCode is waiting on a background job"],
        "the turn ended with the shell still running"
    );
    assert_eq!(
        statuses(&mut decoder, 5, "session.execution.started", &root),
        [""],
        "a new turn is work again, whatever runs behind it"
    );
}
