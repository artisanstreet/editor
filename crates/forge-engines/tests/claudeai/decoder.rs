use super::*;
use serde_json::json;

fn feed(decoder: &mut ClaudeAiDecoder, events: &[Value]) -> Vec<EngineEvent> {
    let mut out = Vec::new();
    for event in events {
        decoder.feed(event, &mut out);
    }
    out
}

#[test]
fn a_recap_loses_its_closing_full_stop_only() {
    assert_eq!(recap_line("Reading the config."), "Reading the config");
    assert_eq!(recap_line("  Reading the config.  "), "Reading the config");
    assert_eq!(recap_line("Reading the config"), "Reading the config");
    assert_eq!(recap_line("Reading the config..."), "Reading the config...");
    assert_eq!(
        recap_line("Reading the config\u{2026}"),
        "Reading the config\u{2026}"
    );
    assert_eq!(recap_line("Is it v2.1?"), "Is it v2.1?");
}

#[test]
fn the_providers_recap_is_published_whole() {
    let mut decoder = ClaudeAiDecoder::default();
    let out = feed(
        &mut decoder,
        &[json!({
            "type": "content_block_delta",
            "index": 0,
            "delta": {
                "type": "thinking_summary_delta",
                "summary": { "summary": "Checking which setup steps the task actually requires." }
            }
        })],
    );
    assert_eq!(
        out,
        vec![EngineEvent::Live {
            target: LiveKind::Narration,
            text: "Checking which setup steps the task actually requires".into(),
            append: false
        }]
    );
}

#[test]
fn the_reply_settles_with_its_text() {
    let mut decoder = ClaudeAiDecoder::default();
    let out = feed(
        &mut decoder,
        &[
            json!({ "type": "message_start", "message": { "id": "msg_1", "model": "claude-opus-5-5" } }),
            json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "text", "text": "" } }),
            json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "text_delta", "text": "Hello" } }),
            json!({ "type": "content_block_stop", "index": 0 }),
            json!({ "type": "message_delta", "delta": { "stop_reason": "end_turn" } }),
            json!({ "type": "message_stop" }),
        ],
    );
    assert_eq!(decoder.stop(), Some(&Stop::Completed));
    assert_eq!(
        out.last(),
        Some(&EngineEvent::Item {
            key: "msg_1:0".into(),
            parent: None,
            status: Status::Done,
            kind: ItemKind::AssistantMessage {
                text: "Hello".into(),
                phase: Phase::Final
            }
        })
    );
    assert!(
        !out.iter()
            .any(|event| matches!(event, EngineEvent::Usage(_))),
        "no usage is reported when claude.ai reports none"
    );
}

#[test]
fn text_before_a_callers_tool_is_kept_when_the_call_is_taken() {
    let mut decoder = ClaudeAiDecoder::default();
    feed(
        &mut decoder,
        &[
            json!({ "type": "message_start", "message": { "id": "msg_1" } }),
            json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "text" } }),
            json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "text_delta", "text": "Let me look." } }),
            json!({ "type": "content_block_stop", "index": 0 }),
            json!({ "type": "content_block_start", "index": 1, "content_block": { "type": "tool_use", "id": "toolu_1", "name": "Read", "tool_origin": null } }),
            json!({ "type": "content_block_delta", "index": 1, "delta": { "type": "input_json_delta", "partial_json": "{\"file_path\":\"/a\"}" } }),
            json!({ "type": "content_block_stop", "index": 1 }),
        ],
    );
    assert!(decoder.waiting_on_caller());
    let mut out = Vec::new();
    let calls = decoder.take_calls(&mut out);
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].input["file_path"], "/a");
    assert_eq!(
        out,
        vec![EngineEvent::Item {
            key: "msg_1:0".into(),
            parent: None,
            status: Status::Done,
            kind: ItemKind::AssistantMessage {
                text: "Let me look.".into(),
                phase: Phase::Commentary
            }
        }]
    );
}

#[test]
fn a_tool_the_provider_runs_is_shown_with_its_result_and_never_the_callers() {
    let mut decoder = ClaudeAiDecoder::default();
    let out = feed(
        &mut decoder,
        &[
            json!({ "type": "message_start", "message": { "id": "msg_1" } }),
            json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "tool_use", "id": "toolu_1", "name": "bash_tool", "input": {}, "tool_origin": "first_party" } }),
            json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "input_json_delta", "partial_json": "{\"command\":\"ls\"}" } }),
            json!({ "type": "content_block_stop", "index": 0 }),
        ],
    );
    assert!(
        !decoder.waiting_on_caller(),
        "the provider's tool is not Forge's to answer, even before its result"
    );
    assert!(matches!(
        out.last(),
        Some(EngineEvent::Item { status: Status::Running, kind: ItemKind::Tool { tool: ToolKind::Other { name, .. }, .. }, .. }) if name == "bash_tool"
    ));
    let out = feed(
        &mut decoder,
        &[json!({
            "type": "content_block_start", "index": 1,
            "content_block": { "type": "tool_result", "tool_use_id": "toolu_1", "name": "bash_tool", "content": [{ "type": "text", "text": "file.txt" }] }
        })],
    );
    assert!(matches!(
        out.as_slice(),
        [EngineEvent::Item {
            status: Status::Done,
            kind: ItemKind::Tool {
                output: Some(_),
                ..
            },
            ..
        }]
    ));
}

#[test]
fn the_providers_web_search_shows_its_query_and_the_pages_it_found() {
    let mut decoder = ClaudeAiDecoder::default();
    let out = feed(
        &mut decoder,
        &[
            json!({ "type": "message_start", "message": { "id": "msg_1" } }),
            json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "tool_use", "id": "toolu_w", "name": "web_search", "input": {}, "tool_origin": "first_party", "mcp_server_url": null } }),
            json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "input_json_delta", "partial_json": "{\"query\":\"latest Rust release\"}" } }),
            json!({ "type": "content_block_stop", "index": 0 }),
            json!({
                "type": "content_block_start", "index": 1,
                "content_block": { "type": "tool_result", "tool_use_id": "toolu_w", "name": "web_search", "tool_origin": "first_party", "content": [
                    { "type": "knowledge", "title": "Rust 1.99.0 released", "url": "https://lwn.net/Articles/1098068/" }
                ] }
            }),
        ],
    );
    assert!(!decoder.waiting_on_caller());
    assert!(
        !decoder.connector_started(),
        "web search is not a connector"
    );
    let Some(EngineEvent::Item {
        status: Status::Done,
        kind:
            ItemKind::Tool {
                tool: ToolKind::WebSearch { query },
                output: Some(output),
                ..
            },
        ..
    }) = out.last()
    else {
        panic!("the search settles as a web search: {out:?}");
    };
    assert_eq!(query, "latest Rust release");
    assert_eq!(
        output,
        &Output::from_text("Rust 1.99.0 released — https://lwn.net/Articles/1098068/")
    );
}

#[test]
fn an_exceeded_limit_fails_the_turn_with_its_reset() {
    let mut decoder = ClaudeAiDecoder::default();
    feed(
        &mut decoder,
        &[
            json!({ "type": "message_limit", "message_limit": { "type": "exceeded_limit", "representativeClaim": "five_hour", "resetsAt": 100 } }),
        ],
    );
    assert_eq!(
        decoder.take_failure(),
        Some(Refusal::UsageLimit {
            resets_at: Some(100_000),
            message: "session usage limit".to_owned()
        })
    );
}

#[test]
fn streamed_concurrency_limits_are_temporary_for_both_event_shapes() {
    let limit = json!({"type":"exceeded_limit", "representativeClaim":"concurrents"});
    for event in [
        json!({"type":"message_limit", "message_limit":limit}),
        json!({"type":"error", "error":{"type":"rate_limit_error", "message":limit.to_string()}}),
    ] {
        let mut decoder = ClaudeAiDecoder::default();
        feed(&mut decoder, &[event]);
        assert_eq!(
            decoder.take_failure(),
            Some(Refusal::RateLimited(
                "Claude.ai has too many active responses. Try again shortly.".to_owned()
            ))
        );
    }
}

#[test]
fn an_unknown_event_is_ignored() {
    let mut decoder = ClaudeAiDecoder::default();
    assert!(feed(&mut decoder, &[json!({ "type": "something_new" })]).is_empty());
}
