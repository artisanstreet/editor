use editor_aqua::core::{AccountPhase, Core, EditorFiles, Services};
use std::{
    io::{BufRead as _, BufReader, Read as _, Write as _},
    net::TcpListener,
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};

fn wait_for(core: &mut Core, phase: impl Fn(&AccountPhase) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !phase(core.account().phase()) {
        assert!(Instant::now() < deadline, "account transition timed out");
        core.drain_with(|_, _| {});
        std::thread::yield_now();
    }
}

#[test]
fn cancelled_requests_cannot_lock_out_retries_or_apply_late_responses() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let (seen, requests) = mpsc::channel();
    let (reply, replies) = mpsc::channel::<(u16, String)>();
    let server = std::thread::spawn(move || {
        for stream in listener.incoming().take(3) {
            let mut stream = stream.unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            assert!(line.starts_with("POST /v1/auth/magic/request "));
            let mut length = 0;
            loop {
                line.clear();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" {
                    break;
                }
                if let Some((name, value)) = line.split_once(':') {
                    if name.eq_ignore_ascii_case("content-length") {
                        length = value.trim().parse().unwrap();
                    }
                }
            }
            let mut body = vec![0; length];
            reader.read_exact(&mut body).unwrap();
            seen.send(serde_json::from_slice::<serde_json::Value>(&body).unwrap())
                .unwrap();
            let (status, body) = replies.recv_timeout(Duration::from_secs(5)).unwrap();
            let _ = write!(
                stream,
                "HTTP/1.1 {status} Response\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    let directory = tempfile::tempdir().unwrap();
    let services = Services::scripted(&url, Arc::new(|_| anyhow::bail!("no ae")), None);
    let mut core = Core::with_services(
        EditorFiles {
            dir: directory.path().to_owned(),
        },
        services,
    )
    .unwrap();
    let mut entry = editor_aqua::shell::entry::Entry::new(None, Default::default());
    core.account_mut().sign_out();
    core.account_mut()
        .request_link(" test@example.invalid ", "Test".into());
    let request = requests.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(request["email"], "test@example.invalid");
    entry.sign_in.emit()(editor_aqua::frame::sign_in::Event::Cancel);
    entry.dispatch(&mut core);
    assert!(matches!(core.account().phase(), AccountPhase::SignedOut));
    let success = serde_json::json!({"request_id":"request", "request_secret":"secret", "expires_at":"2099-01-01T00:00:00Z"}).to_string();
    reply.send((200, success.clone())).unwrap();
    core.account_mut()
        .request_link("retry@example.invalid", "Test".into());
    assert_eq!(
        requests.recv_timeout(Duration::from_secs(5)).unwrap()["email"],
        "retry@example.invalid"
    );
    reply.send((200, success)).unwrap();
    wait_for(&mut core, |phase| {
        matches!(phase, AccountPhase::Pending { .. })
    });
    assert!(
        matches!(core.account().phase(), AccountPhase::Pending { email, .. } if email == "retry@example.invalid")
    );
    entry.sign_in.emit()(editor_aqua::frame::sign_in::Event::Cancel);
    entry.dispatch(&mut core);
    core.account_mut()
        .request_link("retry@example.invalid", "Test".into());
    requests.recv_timeout(Duration::from_secs(5)).unwrap();
    reply.send((503, serde_json::json!({"error":{"code":"internal", "message":"mail delivery failed", "request_id":"request"}}).to_string())).unwrap();
    wait_for(&mut core, |phase| {
        matches!(phase, AccountPhase::Error { .. })
    });
    core.account_mut().dismiss_error();
    assert!(matches!(core.account().phase(), AccountPhase::SignedOut));
    server.join().unwrap();
}

#[test]
fn a_late_cancel_does_not_clear_an_authenticated_profile() {
    let directory = tempfile::tempdir().unwrap();
    let mut core = Core::for_tests(EditorFiles {
        dir: directory.path().to_owned(),
    })
    .unwrap();
    let original = core.account().profile().unwrap().clone();
    let mut entry = editor_aqua::shell::entry::Entry::new(None, Default::default());
    entry.sign_in.emit()(editor_aqua::frame::sign_in::Event::Cancel);
    entry.dispatch(&mut core);
    assert_eq!(core.account().profile(), Some(&original));
}
