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
    let name = if cfg!(windows) {
        "shot one.png"
    } else {
        "shot \"one\".png"
    };
    let path = dir.path().join(name);
    let image: Vec<u8> = (0..=255).cycle().take(300_000).collect();
    std::fs::write(&path, &image).unwrap();
    let body = multipart(&path, "edge").await.unwrap();
    let head = b"--edge\r\nContent-Disposition: form-data; name=\"file\"; filename=\"shot one.png\"\r\nContent-Type: image/png\r\n\r\n";
    let tail = b"\r\n--edge--\r\n";
    assert_eq!(&body[..head.len()], head);
    assert_eq!(&body[head.len()..body.len() - tail.len()], &image[..]);
    assert_eq!(&body[body.len() - tail.len()..], tail);
}

#[cfg(unix)]
fn signed_in() -> Session {
    Session::new(
        artisan_api::ClaudeAiSession {
            cookies: vec![("sessionKey".into(), "old".into())],
            user_agent: "agent".into(),
        },
        "org".into(),
        None,
    )
}

#[cfg(unix)]
fn set_cookie(value: &str) -> reqwest::header::HeaderMap {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        reqwest::header::SET_COOKIE,
        format!("sessionKey={value}; Domain=.claude.ai; Path=/; Secure")
            .parse()
            .unwrap(),
    );
    headers
}

#[cfg(unix)]
#[derive(Debug, PartialEq, Eq)]
enum SessionRelease {
    WhileWaiting,
    AfterGivingUp,
}

#[cfg(unix)]
#[test]
fn other_requests_run_while_a_new_cookie_waits_for_a_slow_disk() {
    let home = tempfile::tempdir().unwrap();
    signed_in().save(home.path()).unwrap();
    let (opened_tx, opened) = tokio::sync::oneshot::channel();
    let (release_tx, release) = std::sync::mpsc::channel::<()>();
    let slow_disk = std::thread::spawn({
        move || {
            let held = super::super::session::FILE_LOCK
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            opened_tx.send(()).unwrap();
            let released = match release.recv_timeout(std::time::Duration::from_secs(10)) {
                Ok(()) => SessionRelease::WhileWaiting,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => SessionRelease::AfterGivingUp,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    unreachable!("the test holds the release sender until it releases")
                }
            };
            drop(held);
            released
        }
    });
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let transport = Transport::new(home.path(), signed_in()).unwrap();
        opened.await.unwrap();
        transport.keep_cookies(&set_cookie("new"));
        tokio::spawn(async {}).await.unwrap();
        release_tx.send(()).unwrap();
        transport.writer.flushed().await;
    });
    assert_eq!(slow_disk.join().unwrap(), SessionRelease::WhileWaiting);
    let kept = Session::load(home.path()).unwrap().unwrap();
    assert_eq!(cookie(&kept, "sessionKey").as_deref(), Some("new"));
}

#[cfg(unix)]
#[tokio::test]
async fn a_response_repeating_the_current_cookie_does_not_rewrite_the_session() {
    use std::os::unix::fs::MetadataExt as _;
    let home = tempfile::tempdir().unwrap();
    signed_in().save(home.path()).unwrap();
    let transport = Transport::new(home.path(), signed_in()).unwrap();
    transport.keep_cookies(&set_cookie("new"));
    transport.writer.flushed().await;
    let written = std::fs::metadata(Session::path(home.path())).unwrap().ino();
    transport.keep_cookies(&set_cookie("new"));
    transport.writer.flushed().await;
    assert_eq!(
        std::fs::metadata(Session::path(home.path())).unwrap().ino(),
        written
    );
    assert_eq!(
        cookie(&Session::load(home.path()).unwrap().unwrap(), "sessionKey").as_deref(),
        Some("new")
    );
}

#[cfg(unix)]
fn cookie(session: &Session, name: &str) -> Option<String> {
    session.cookie_header().split("; ").find_map(|pair| {
        pair.strip_prefix(name)?
            .strip_prefix('=')
            .map(str::to_owned)
    })
}
