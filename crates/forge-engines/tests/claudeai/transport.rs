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

#[tokio::test]
async fn an_uploaded_image_is_one_form_part_around_its_exact_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("shot \"one\".png");
    let image: Vec<u8> = (0..=255).cycle().take(300_000).collect();
    std::fs::write(&path, &image).unwrap();
    let body = super::multipart(&path, "edge").await.unwrap();
    let head = b"--edge\r\nContent-Disposition: form-data; name=\"file\"; filename=\"shot one.png\"\r\nContent-Type: image/png\r\n\r\n";
    let tail = b"\r\n--edge--\r\n";
    assert_eq!(&body[..head.len()], head);
    assert_eq!(&body[head.len()..body.len() - tail.len()], &image[..]);
    assert_eq!(&body[body.len() - tail.len()..], tail);
}
