use super::*;

fn message(event: &str, data: &str) -> SseEvent {
    SseEvent::Message(sse_core::MessageEvent {
        event: event.to_owned().into(),
        data: data.to_owned(),
        last_event_id: None,
    })
}

#[test]
fn an_unreadable_stream_event_fails_the_stream_instead_of_vanishing() {
    let damaged = stream_event(
        "conv-1",
        Ok(message(
            "content_block_delta",
            r#"{"type":"content_block_delta","delta":{"te"#,
        )),
    );
    assert!(
        matches!(
            damaged,
            Some(Err(StreamError::Unreadable { ref conversation, ref event, .. }))
                if conversation == "conv-1" && event == "content_block_delta"
        ),
        "{damaged:?}"
    );
    let readable = stream_event(
        "conv-1",
        Ok(message("message_stop", r#"{"type":"message_stop"}"#)),
    );
    assert!(
        matches!(readable, Some(Ok(ref value)) if value["type"] == "message_stop"),
        "{readable:?}"
    );
}
