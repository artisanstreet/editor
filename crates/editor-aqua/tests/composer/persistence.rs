use artisan_api::tls::{Identity, fingerprint};
use editor_aqua::composer::persistence::DraftSessions;
use editor_aqua::composer::persistence::{DraftHandle, DraftLease};
use editor_aqua::core::{Core, EditorFiles, ForgeClient, HostConfig, HostId, HostKind};
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};
use tokio_rustls::{TlsAcceptor, server::TlsStream};

async fn server() -> (TcpListener, TlsAcceptor, ForgeClient) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let identity = Identity::generate("synthetic-draft-forge").unwrap();
    let certificate = identity.certificate_der().unwrap();
    let pin = fingerprint(certificate.as_ref());
    let config = rustls::ServerConfig::builder_with_provider(artisan_api::tls::provider())
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![certificate], identity.key_der().unwrap())
        .unwrap();
    let client = ForgeClient::new(
        &listener.local_addr().unwrap().to_string(),
        &pin,
        &Identity::generate("synthetic-draft-editor").unwrap(),
    )
    .unwrap();
    (listener, TlsAcceptor::from(Arc::new(config)), client)
}

async fn request(
    listener: &TcpListener,
    acceptor: &TlsAcceptor,
) -> (TlsStream<TcpStream>, String, Value) {
    let (socket, _) = listener.accept().await.unwrap();
    let mut socket = acceptor.accept(socket).await.unwrap();
    let mut headers = Vec::new();
    while !headers.ends_with(b"\r\n\r\n") {
        assert!(headers.len() < 16 * 1024);
        headers.push(socket.read_u8().await.unwrap());
    }
    let headers = String::from_utf8(headers).unwrap();
    let length = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().unwrap())
        })
        .unwrap_or(0);
    assert!(length < 64 * 1024);
    let mut body = vec![0; length];
    socket.read_exact(&mut body).await.unwrap();
    let body = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body).unwrap()
    };
    (socket, headers.lines().next().unwrap().to_owned(), body)
}

async fn respond(socket: &mut TlsStream<TcpStream>, body: Value) {
    let body = serde_json::to_vec(&body).unwrap();
    let headers = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    socket.write_all(headers.as_bytes()).await.unwrap();
    socket.write_all(&body).await.unwrap();
    socket.flush().await.unwrap();
}

async fn load(lease: &DraftLease, listener: &TcpListener, acceptor: &TlsAcceptor) {
    let mut changed = lease.subscribe();
    let (mut socket, route, body) = request(listener, acceptor).await;
    assert_eq!(route, "GET /drafts/new HTTP/1.1");
    assert_eq!(body, Value::Null);
    respond(&mut socket, json!({"revision": 0, "body": null})).await;
    drop(socket);
    while !lease.read(|state| state.loaded) {
        changed.changed().await.unwrap();
    }
}

fn run(future: impl std::future::Future<Output = ()>) {
    editor_aqua::core::runtime::runtime().block_on(async {
        tokio::time::timeout(Duration::from_secs(10), future)
            .await
            .expect("synthetic draft server must finish");
    });
}

fn fixture() -> (tempfile::TempDir, Core, HostConfig, ForgeClient) {
    let directory = tempfile::tempdir().unwrap();
    let mut core = Core::for_tests(EditorFiles {
        dir: directory.path().to_owned(),
    })
    .unwrap();
    let identity = Identity::generate("draft-lifetime-test").unwrap();
    let fingerprint = fingerprint(identity.certificate_der().unwrap().as_ref());
    let config = HostConfig {
        id: HostId("draft-host".into()),
        name: "Draft Forge".into(),
        address: "127.0.0.1:1".into(),
        fingerprint,
        kind: HostKind::Local,
    };
    let client = ForgeClient::new(&config.address, &config.fingerprint, &identity).unwrap();
    core.add_test_host(&config, client.clone());
    (directory, core, config, client)
}

#[test]
fn replacing_host_invalidates_old_draft_writer_but_preserves_current_writer() {
    let (_directory, mut core, config, client) = fixture();
    let mut sessions = DraftSessions::default();
    let old_instance = core.host(&config.id).unwrap().instance();
    let old = sessions
        .open(
            config.id.clone(),
            old_instance,
            "new".into(),
            client.clone(),
        )
        .unwrap();
    old.queue(serde_json::json!({"text": "old pairing"}));
    core.remove_host(&config.id).unwrap();
    core.add_test_host(&config, client.clone());
    let current_instance = core.host(&config.id).unwrap().instance();
    let current = sessions
        .open(config.id.clone(), current_instance, "new".into(), client)
        .unwrap();
    current.queue(serde_json::json!({"text": "current pairing"}));

    sessions.retain_hosts(&core);

    assert!(old.read(|state| state.removed()));
    assert!(!current.read(|state| state.removed()));
    old.queue(serde_json::json!({"text": "stale edit"}));
    old.retry();
    assert_eq!(
        old.read(|state| state.body.clone()),
        serde_json::Value::Null
    );
    assert_eq!(
        current.read(|state| state.body.clone()),
        serde_json::json!({"text": "current pairing"})
    );
}

#[test]
fn dropping_lease_and_manager_flushes_a_loaded_dirty_draft() {
    let (_directory, core, config, _client) = fixture();
    run(async {
        let (listener, acceptor, client) = server().await;
        let mut sessions = DraftSessions::default();
        let lease = sessions
            .open(
                config.id.clone(),
                core.host(&config.id).unwrap().instance(),
                "new".into(),
                client,
            )
            .unwrap();
        load(&lease, &listener, &acceptor).await;
        let handle = DraftHandle::clone(&lease);
        let mut changed = handle.subscribe();
        let expected = json!({"text": "draft survives workspace teardown"});
        lease.queue(expected.clone());

        drop(lease);
        drop(sessions);

        let (mut socket, route, body) = request(&listener, &acceptor).await;
        assert_eq!(route, "PUT /drafts/new HTTP/1.1");
        assert_eq!(body, json!({"expected_revision": 0, "body": expected}));
        let persisted = body["body"].clone();
        respond(
            &mut socket,
            json!({"saved": true, "current": {"revision": 1, "body": persisted}}),
        )
        .await;
        drop(socket);
        while !handle.read(|state| state.evictable()) {
            changed.changed().await.unwrap();
        }
        assert!(!handle.read(|state| state.removed()));
        assert_eq!(persisted, expected);
        assert_eq!(handle.read(|state| state.body.clone()), persisted);
    });
}

#[test]
fn graceful_teardown_finishes_an_inflight_write_and_queued_post() {
    use artisan_api::{ItemId, SendMessage, ThreadId};
    use editor_aqua::composer::draft_session::Submission;

    let (_directory, core, config, _client) = fixture();
    run(async {
        let (listener, acceptor, client) = server().await;
        let mut sessions = DraftSessions::default();
        let lease = sessions
            .open(
                config.id.clone(),
                core.host(&config.id).unwrap().instance(),
                "new".into(),
                client,
            )
            .unwrap();
        load(&lease, &listener, &acceptor).await;
        lease.queue(json!({"text": "send survives teardown"}));
        lease.submit(Submission {
            thread: ThreadId("thread".into()),
            create: None,
            message: SendMessage {
                id: ItemId("message".into()),
                text: "send survives teardown".into(),
                images: Vec::new(),
                config: None,
                draft: None,
            },
        });
        let handle = DraftHandle::clone(&lease);
        let mut changed = handle.subscribe();
        let (mut socket, route, body) = request(&listener, &acceptor).await;
        assert_eq!(route, "PUT /drafts/new HTTP/1.1");

        drop(lease);
        drop(sessions);

        respond(
            &mut socket,
            json!({"saved": true, "current": {"revision": 1, "body": body["body"]}}),
        )
        .await;
        drop(socket);
        let (mut socket, route, body) = request(&listener, &acceptor).await;
        assert_eq!(route, "POST /threads/thread/messages HTTP/1.1");
        assert_eq!(body["id"], "message");
        assert_eq!(body["text"], "send survives teardown");
        assert_eq!(body["draft"], json!({"scope": "new", "revision": 1}));
        respond(&mut socket, json!({"id": "message", "thread_id": "thread", "position": 1, "seq": 1, "run_id": null, "parent_id": null, "status": "done", "created_at": 0, "updated_at": 0, "kind": {"type": "user_message", "text": "send survives teardown"}})).await;
        drop(socket);
        let (mut socket, route, _) = request(&listener, &acceptor).await;
        assert_eq!(route, "GET /drafts/new HTTP/1.1");
        respond(&mut socket, json!({"revision": 2, "body": null})).await;
        drop(socket);
        while !handle.read(|state| state.loaded && state.evictable()) {
            changed.changed().await.unwrap();
        }
        assert!(!handle.read(|state| state.removed()));
        assert_eq!(handle.read(|state| state.body.clone()), Value::Null);
    });
}

#[test]
fn explicit_host_removal_cancels_an_inflight_write_and_queued_post() {
    use artisan_api::{ItemId, SendMessage, ThreadId};
    use editor_aqua::composer::draft_session::Submission;

    let (_directory, mut core, config, _client) = fixture();
    run(async {
        let (listener, acceptor, client) = server().await;
        let mut sessions = DraftSessions::default();
        let lease = sessions
            .open(
                config.id.clone(),
                core.host(&config.id).unwrap().instance(),
                "new".into(),
                client,
            )
            .unwrap();
        load(&lease, &listener, &acceptor).await;
        lease.queue(json!({"text": "stale pairing"}));
        lease.submit(Submission {
            thread: ThreadId("thread".into()),
            create: None,
            message: SendMessage {
                id: ItemId("stale-message".into()),
                text: "stale pairing".into(),
                images: Vec::new(),
                config: None,
                draft: None,
            },
        });
        let (mut socket, route, body) = request(&listener, &acceptor).await;
        assert_eq!(route, "PUT /drafts/new HTTP/1.1");
        let accepted_body = body["body"].clone();

        core.remove_host(&config.id).unwrap();
        sessions.retain_hosts(&core);

        assert!(lease.read(|state| state.removed()));
        assert!(!lease.read(|state| state.sending()));
        assert_eq!(lease.read(|state| state.body.clone()), Value::Null);
        let mut bytes = [0; 1024];
        loop {
            match socket.read(&mut bytes).await {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
        }
        assert_eq!(accepted_body, json!({"text": "stale pairing"}));
        drop(lease);
        drop(sessions);
    });
}

fn child_process(test: &str) -> bool {
    if std::env::var("ARTISAN_DRAFT_SHUTDOWN_TEST").as_deref() == Ok(test) {
        return true;
    }
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", test, "--test-threads=1", "--nocapture"])
        .env("ARTISAN_DRAFT_SHUTDOWN_TEST", test)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "child draft shutdown failed: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    false
}

#[test]
fn process_exit_waits_for_draft_write_acceptance() {
    use editor_aqua::composer::persistence::{ShutdownOutcome, drain_pending};

    if !child_process("persistence::process_exit_waits_for_draft_write_acceptance") {
        return;
    }
    let (_directory, core, config, _client) = fixture();
    run(async {
        let (listener, acceptor, client) = server().await;
        let mut sessions = DraftSessions::default();
        let lease = sessions
            .open(
                config.id.clone(),
                core.host(&config.id).unwrap().instance(),
                "new".into(),
                client,
            )
            .unwrap();
        load(&lease, &listener, &acceptor).await;
        lease.queue(json!({"text": "persist before process exit"}));
        drop(lease);
        drop(sessions);

        let mut shutdown = Box::pin(drain_pending(Duration::from_secs(5)));
        assert!(futures::poll!(shutdown.as_mut()).is_pending());
        let (mut socket, route, body) = request(&listener, &acceptor).await;
        assert_eq!(route, "PUT /drafts/new HTTP/1.1");
        let persisted = body["body"].clone();
        respond(
            &mut socket,
            json!({"saved": true, "current": {"revision": 1, "body": persisted}}),
        )
        .await;
        drop(socket);
        assert_eq!(shutdown.await, ShutdownOutcome::Saved);
        assert_eq!(persisted, json!({"text": "persist before process exit"}));
    });
}

#[test]
fn process_exit_reports_a_pending_draft_at_the_deadline() {
    use editor_aqua::composer::persistence::{ShutdownOutcome, drain_pending};

    if !child_process("persistence::process_exit_reports_a_pending_draft_at_the_deadline") {
        return;
    }
    let (_directory, core, config, _client) = fixture();
    run(async {
        let (listener, acceptor, client) = server().await;
        let mut sessions = DraftSessions::default();
        let lease = sessions
            .open(
                config.id.clone(),
                core.host(&config.id).unwrap().instance(),
                "new".into(),
                client,
            )
            .unwrap();
        load(&lease, &listener, &acceptor).await;
        lease.queue(json!({"text": "server never acknowledges"}));
        drop(lease);
        drop(sessions);
        let (mut socket, route, _) = request(&listener, &acceptor).await;
        assert_eq!(route, "PUT /drafts/new HTTP/1.1");

        assert_eq!(
            drain_pending(Duration::ZERO).await,
            ShutdownOutcome::Incomplete {
                failed: 0,
                timed_out: 1
            }
        );
        let mut bytes = [0; 1024];
        loop {
            match socket.read(&mut bytes).await {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
        }
    });
}

#[test]
fn process_exit_reports_a_rejected_draft_write() {
    use editor_aqua::composer::persistence::{ShutdownOutcome, drain_pending};

    if !child_process("persistence::process_exit_reports_a_rejected_draft_write") {
        return;
    }
    let (_directory, core, config, _client) = fixture();
    run(async {
        let (listener, acceptor, client) = server().await;
        let mut sessions = DraftSessions::default();
        let lease = sessions
            .open(
                config.id.clone(),
                core.host(&config.id).unwrap().instance(),
                "new".into(),
                client,
            )
            .unwrap();
        load(&lease, &listener, &acceptor).await;
        lease.queue(json!({"text": "local draft"}));
        drop(lease);
        drop(sessions);

        let mut shutdown = Box::pin(drain_pending(Duration::from_secs(5)));
        assert!(futures::poll!(shutdown.as_mut()).is_pending());
        let (mut socket, route, _) = request(&listener, &acceptor).await;
        assert_eq!(route, "PUT /drafts/new HTTP/1.1");
        respond(
            &mut socket,
            json!({"saved": false, "current": {"revision": 1, "body": {"text": "remote draft"}}}),
        )
        .await;
        drop(socket);
        assert_eq!(
            shutdown.await,
            ShutdownOutcome::Incomplete {
                failed: 1,
                timed_out: 0
            }
        );
    });
}
