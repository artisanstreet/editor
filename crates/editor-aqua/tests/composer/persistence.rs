use artisan_api::tls::{Identity, fingerprint};
use editor_aqua::composer::draft_body::DraftBody;
use editor_aqua::composer::persistence::DraftSessions;
use editor_aqua::composer::persistence::{DraftHandle, DraftLease};
use editor_aqua::core::{Core, ForgeClient, HostConfig, HostId, HostKind};
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};
use tokio_rustls::{TlsAcceptor, server::TlsStream};

fn typed(text: &str) -> DraftBody {
    DraftBody {
        text: text.to_owned(),
        ..DraftBody::default()
    }
}

async fn server() -> (TcpListener, TlsAcceptor, ForgeClient) {
    let (listener, acceptor, pin) = pinned_server().await;
    let client = ForgeClient::new(
        &listener.local_addr().unwrap().to_string(),
        &pin,
        &Identity::generate("synthetic-draft-editor").unwrap(),
    )
    .unwrap();
    (listener, acceptor, client)
}

async fn pinned_server() -> (TcpListener, TlsAcceptor, String) {
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
    (listener, TlsAcceptor::from(Arc::new(config)), pin)
}

async fn request(
    listener: &TcpListener,
    acceptor: &TlsAcceptor,
) -> (TlsStream<TcpStream>, String, Value) {
    let (socket, route, body) = request_bytes(listener, acceptor).await;
    let body = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body).unwrap()
    };
    (socket, route, body)
}

async fn request_bytes(
    listener: &TcpListener,
    acceptor: &TlsAcceptor,
) -> (TlsStream<TcpStream>, String, Vec<u8>) {
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
    assert!(length < 1024 * 1024);
    let mut body = vec![0; length];
    socket.read_exact(&mut body).await.unwrap();
    (socket, headers.lines().next().unwrap().to_owned(), body)
}

async fn composer_request(
    listener: &TcpListener,
    acceptor: &TlsAcceptor,
) -> (TlsStream<TcpStream>, String, Value) {
    loop {
        let (socket, route, body) = request(listener, acceptor).await;
        if route.contains(" /drafts/") || route.starts_with("POST /threads") {
            return (socket, route, body);
        }
        drop(socket);
    }
}

async fn respond(socket: &mut TlsStream<TcpStream>, body: Value) {
    respond_status(socket, "200 OK", body).await;
}

async fn respond_status(socket: &mut TlsStream<TcpStream>, status: &str, body: Value) {
    let body = serde_json::to_vec(&body).unwrap();
    let headers = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    socket.write_all(headers.as_bytes()).await.unwrap();
    socket.write_all(&body).await.unwrap();
    socket.flush().await.unwrap();
}

#[test]
fn rejected_write_unlocks_queued_send_and_retry_preserves_the_message() {
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
        let mut changed = lease.subscribe();
        let draft = json!({"text": "preserve this prompt"});
        lease.queue(typed("preserve this prompt"));
        lease.submit(Submission {
            thread: ThreadId("thread".into()),
            create: None,
            message: SendMessage {
                id: ItemId("original-message".into()),
                text: "preserve this prompt".into(),
                images: Vec::new(),
                config: None,
                draft: None,
            },
        });
        assert!(lease.read(editor_aqua::composer::draft_session::DraftSession::sending));
        let (mut socket, route, _) = request(&listener, &acceptor).await;
        assert_eq!(route, "PUT /drafts/new HTTP/1.1");
        respond_status(
            &mut socket,
            "500 Internal Server Error",
            json!({"error": {"code": "internal", "message": "draft write rejected"}}),
        )
        .await;
        drop(socket);
        while lease.read(|state| state.error().is_none()) {
            changed.changed().await.unwrap();
        }
        assert!(!lease.read(editor_aqua::composer::draft_session::DraftSession::sending));
        assert_eq!(
            lease.read(|state| state.body().clone()),
            typed("preserve this prompt")
        );
        assert!(lease.read(editor_aqua::composer::draft_session::DraftSession::pending));

        lease.retry();
        let (mut socket, route, _) = request(&listener, &acceptor).await;
        assert_eq!(route, "GET /drafts/new HTTP/1.1");
        respond(&mut socket, json!({"revision": 0, "body": null})).await;
        drop(socket);
        let (mut socket, route, body) = request(&listener, &acceptor).await;
        assert_eq!(route, "PUT /drafts/new HTTP/1.1");
        assert_eq!(body["body"], draft);
        respond(
            &mut socket,
            json!({"saved": true, "current": {"revision": 1, "body": draft}}),
        )
        .await;
        drop(socket);
        let (mut socket, route, body) = request(&listener, &acceptor).await;
        assert_eq!(route, "POST /threads/thread/messages HTTP/1.1");
        assert_eq!(body["id"], "original-message");
        assert_eq!(body["text"], "preserve this prompt");
        assert_eq!(body["draft"], json!({"scope": "new", "revision": 1}));
        respond(&mut socket, json!({"id": "original-message", "thread_id": "thread", "position": 1, "seq": 1, "run_id": null, "parent_id": null, "status": "done", "created_at": 0, "updated_at": 0, "kind": {"type": "user_message", "text": "preserve this prompt"}})).await;
        drop(socket);
        let (mut socket, route, _) = request(&listener, &acceptor).await;
        assert_eq!(route, "GET /drafts/new HTTP/1.1");
        respond(&mut socket, json!({"revision": 2, "body": null})).await;
        drop(socket);
        while !lease.read(|state| state.is_loaded() && state.evictable()) {
            changed.changed().await.unwrap();
        }
        assert!(!lease.read(editor_aqua::composer::draft_session::DraftSession::sending));
        assert!(!lease.read(editor_aqua::composer::draft_session::DraftSession::pending));
    });
}

async fn load(lease: &DraftLease, listener: &TcpListener, acceptor: &TlsAcceptor) {
    let mut changed = lease.subscribe();
    let (mut socket, route, body) = request(listener, acceptor).await;
    assert_eq!(route, "GET /drafts/new HTTP/1.1");
    assert_eq!(body, Value::Null);
    respond(&mut socket, json!({"revision": 0, "body": null})).await;
    drop(socket);
    while !lease.read(editor_aqua::composer::draft_session::DraftSession::is_loaded) {
        changed.changed().await.unwrap();
    }
}

fn run(future: impl Future<Output = ()>) {
    editor_aqua::core::runtime::runtime().block_on(async {
        tokio::time::timeout(Duration::from_secs(10), future)
            .await
            .expect("synthetic draft server must finish");
    });
}

fn fixture() -> (tempfile::TempDir, Core, HostConfig, ForgeClient) {
    let directory = tempfile::tempdir().unwrap();
    let mut core = test_support::editor::core::test_core(directory.path());
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
    test_support::editor::core::connect_client(&mut core, &config, client.clone());
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
    old.queue(typed("old pairing"));
    core.remove_host(&config.id).unwrap();
    test_support::editor::core::connect_client(&mut core, &config, client.clone());
    let current_instance = core.host(&config.id).unwrap().instance();
    let current = sessions
        .open(config.id.clone(), current_instance, "new".into(), client)
        .unwrap();
    current.queue(typed("current pairing"));

    sessions.retain_hosts(&core);

    assert!(old.read(editor_aqua::composer::draft_session::DraftSession::removed));
    assert!(!current.read(editor_aqua::composer::draft_session::DraftSession::removed));
    old.queue(typed("stale edit"));
    old.retry();
    assert_eq!(old.read(|state| state.body().clone()), DraftBody::default());
    assert_eq!(
        current.read(|state| state.body().clone()),
        typed("current pairing")
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
        lease.queue(typed("draft survives workspace teardown"));

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
        while !handle.read(editor_aqua::composer::draft_session::DraftSession::evictable) {
            changed.changed().await.unwrap();
        }
        assert!(!handle.read(editor_aqua::composer::draft_session::DraftSession::removed));
        assert_eq!(persisted, expected);
        assert_eq!(
            handle.read(|state| state.body().clone()),
            typed("draft survives workspace teardown")
        );
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
        lease.queue(typed("send survives teardown"));
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
        while !handle.read(|state| state.is_loaded() && state.evictable()) {
            changed.changed().await.unwrap();
        }
        assert!(!handle.read(editor_aqua::composer::draft_session::DraftSession::removed));
        assert_eq!(
            handle.read(|state| state.body().clone()),
            DraftBody::default()
        );
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
        lease.queue(typed("stale pairing"));
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

        assert!(lease.read(editor_aqua::composer::draft_session::DraftSession::removed));
        assert!(!lease.read(editor_aqua::composer::draft_session::DraftSession::sending));
        assert_eq!(
            lease.read(|state| state.body().clone()),
            DraftBody::default()
        );
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
        lease.queue(typed("persist before process exit"));
        drop(lease);
        drop(sessions);

        let mut shutdown = Box::pin(drain_pending(Duration::from_secs(5)));
        assert!(test_support::poll::poll_once(shutdown.as_mut()).is_pending());
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
        lease.queue(typed("server never acknowledges"));
        drop(lease);
        drop(sessions);
        let (mut socket, route, _) = request(&listener, &acceptor).await;
        assert_eq!(route, "PUT /drafts/new HTTP/1.1");

        assert_eq!(
            drain_pending(Duration::ZERO).await,
            ShutdownOutcome::Incomplete {
                failed: 0,
                timed_out: 1,
                kept_locally: 0,
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
        lease.queue(typed("local draft"));
        drop(lease);
        drop(sessions);

        let mut shutdown = Box::pin(drain_pending(Duration::from_secs(5)));
        assert!(test_support::poll::poll_once(shutdown.as_mut()).is_pending());
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
                timed_out: 0,
                kept_locally: 0,
            }
        );
    });
}

#[test]
fn closing_offline_keeps_the_draft_on_this_device_until_forge_takes_it() {
    use editor_aqua::composer::persistence::{ShutdownOutcome, drain_pending};
    use editor_aqua::composer::spool::DraftSpool;

    if !child_process(
        "persistence::closing_offline_keeps_the_draft_on_this_device_until_forge_takes_it",
    ) {
        return;
    }
    let (directory, core, config, _client) = fixture();
    let spool = DraftSpool::new(directory.path());
    let instance = core.host(&config.id).unwrap().instance();
    run(async {
        let (listener, acceptor, client) = server().await;
        let mut sessions = DraftSessions::with_spool(spool.clone());
        let lease = sessions
            .open(config.id.clone(), instance, "new".into(), client.clone())
            .unwrap();
        load(&lease, &listener, &acceptor).await;
        lease.queue(typed("typed while Forge was unreachable"));
        drop(lease);
        drop(sessions);
        let (socket, route, _) = request(&listener, &acceptor).await;
        assert_eq!(route, "PUT /drafts/new HTTP/1.1");
        assert_eq!(
            drain_pending(Duration::ZERO).await,
            ShutdownOutcome::Incomplete {
                failed: 0,
                timed_out: 1,
                kept_locally: 1,
            }
        );
        drop(socket);

        let mut sessions = DraftSessions::with_spool(spool.clone());
        let lease = sessions
            .open(config.id.clone(), instance, "new".into(), client)
            .unwrap();
        let (mut socket, route, _) = request(&listener, &acceptor).await;
        assert_eq!(route, "GET /drafts/new HTTP/1.1");
        respond(&mut socket, json!({"revision": 0, "body": null})).await;
        drop(socket);
        let (mut socket, route, body) = request(&listener, &acceptor).await;
        assert_eq!(route, "PUT /drafts/new HTTP/1.1");
        assert_eq!(
            body["body"],
            json!({"text": "typed while Forge was unreachable"}),
            "the draft typed before closing comes back after restarting"
        );
        let mut changed = lease.subscribe();
        respond(
            &mut socket,
            json!({"saved": true, "current": {"revision": 1, "body": body["body"]}}),
        )
        .await;
        drop(socket);
        while lease.read(editor_aqua::composer::draft_session::DraftSession::pending) {
            changed.changed().await.unwrap();
        }
        assert_eq!(spool.take(&config.id, "new").unwrap(), None);
    });
}

fn pasted_png() -> Vec<u8> {
    let mut png = Vec::new();
    image::RgbaImage::from_pixel(4, 4, image::Rgba([30, 90, 200, 255]))
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    png
}

fn hand_over_pasted_image(lease: &DraftLease, client: ForgeClient) {
    use editor_aqua::composer::attachment::{Attachments, Source};

    let mut attachments = Attachments::new(client);
    assert!(attachments.add(Source::Bytes(pasted_png()), "pasted.png".into()));
    let uploads = attachments.take_unfinished();
    assert_eq!(uploads.len(), 1);
    drop(attachments);
    for upload in uploads {
        lease.adopt_image(upload);
    }
}

async fn wait_for_session(
    session: &DraftHandle,
    until: impl Fn(&editor_aqua::composer::draft_session::DraftSession) -> bool,
) {
    let mut changed = session.subscribe();
    while !session.read(&until) {
        changed.changed().await.unwrap();
    }
}

fn connect_host(core: &mut Core, listener: &TcpListener, pin: &str, name: &str) -> HostId {
    let address = listener.local_addr().unwrap().to_string();
    let host = HostId(name.into());
    test_support::editor::core::connect_client(
        core,
        &HostConfig {
            id: host.clone(),
            name: "Forge".into(),
            address: address.clone(),
            fingerprint: pin.to_owned(),
            kind: HostKind::Local,
        },
        ForgeClient::new(&address, pin, &Identity::generate(name).unwrap()).unwrap(),
    );
    host
}

#[test]
fn a_draft_kept_on_this_device_shows_while_forge_is_still_unreachable() {
    use editor_aqua::composer::controller::{Composer, ComposerTarget};
    use editor_aqua::composer::draft_session::DraftSession;
    use editor_aqua::composer::persistence::{ShutdownOutcome, drain_pending};
    use editor_aqua::composer::spool::DraftSpool;

    if !child_process(
        "persistence::a_draft_kept_on_this_device_shows_while_forge_is_still_unreachable",
    ) {
        return;
    }
    let (directory, mut core, _config, _client) = fixture();
    let spool = DraftSpool::new(directory.path());
    run(async {
        let (listener, acceptor, pin) = pinned_server().await;
        let host = connect_host(&mut core, &listener, &pin, "recovering");
        let target = ComposerTarget::NewThread {
            host: host.clone(),
            project: Some(artisan_api::ProjectId::from("project")),
        };
        let scope = target.scope();

        let mut sessions = DraftSessions::with_spool(spool.clone());
        let mut composer = Composer::new(&core, &mut sessions, target.clone());
        let (mut socket, route, _) = composer_request(&listener, &acceptor).await;
        assert!(route.starts_with("GET /drafts/"), "{route}");
        respond(&mut socket, json!({"revision": 0, "body": null})).await;
        drop(socket);
        wait_for_session(composer.session.as_ref().unwrap(), DraftSession::is_loaded).await;
        composer.sync(&core);
        composer
            .editor
            .borrow_mut()
            .set_text("written before closing");
        composer.edited();
        drop(composer);
        drop(sessions);
        let (socket, route, _) = composer_request(&listener, &acceptor).await;
        assert!(route.starts_with("PUT /drafts/"), "{route}");
        assert_eq!(
            drain_pending(Duration::ZERO).await,
            ShutdownOutcome::Incomplete {
                failed: 0,
                timed_out: 1,
                kept_locally: 1,
            }
        );
        drop(socket);

        let mut sessions = DraftSessions::with_spool(spool.clone());
        let mut composer = Composer::new(&core, &mut sessions, target);
        composer.sync(&core);
        assert_eq!(
            composer.editor.borrow().text(),
            "written before closing",
            "the draft kept on this device is shown before Forge answers"
        );
        let (mut socket, route, _) = composer_request(&listener, &acceptor).await;
        assert!(route.starts_with("GET /drafts/"), "{route}");
        respond_status(
            &mut socket,
            "500 Internal Server Error",
            json!({"error": {"code": "internal", "message": "Forge is restarting"}}),
        )
        .await;
        drop(socket);
        wait_for_session(composer.session.as_ref().unwrap(), DraftSession::halted).await;
        composer.sync(&core);

        let typed = format!("{} and after reopening", composer.editor.borrow().text());
        composer.editor.borrow_mut().set_text(&typed);
        composer.edited();
        let (mut socket, route, _) = composer_request(&listener, &acceptor).await;
        assert!(route.starts_with("GET /drafts/"), "{route}");
        respond(&mut socket, json!({"revision": 0, "body": null})).await;
        drop(socket);
        let (mut socket, route, body) = composer_request(&listener, &acceptor).await;
        assert!(route.starts_with("PUT /drafts/"), "{route}");
        assert_eq!(
            body["body"]["text"], "written before closing and after reopening",
            "typing after reopening adds to the kept draft instead of replacing it"
        );
        respond(
            &mut socket,
            json!({"saved": true, "current": {"revision": 1, "body": body["body"]}}),
        )
        .await;
        drop(socket);
        wait_for_session(composer.session.as_ref().unwrap(), |state| !state.pending()).await;
        assert_eq!(spool.take(&host, &scope).unwrap(), None);
    });
}

async fn draft_or_blob_request(
    listener: &TcpListener,
    acceptor: &TlsAcceptor,
) -> (TlsStream<TcpStream>, String, Vec<u8>) {
    loop {
        let (socket, route, body) = request_bytes(listener, acceptor).await;
        if route.contains(" /drafts/") || route.starts_with("PUT /blobs") {
            return (socket, route, body);
        }
        drop(socket);
    }
}

#[test]
fn a_kept_image_that_uploads_before_the_composer_shows_its_draft_keeps_the_text() {
    use editor_aqua::composer::controller::{Composer, ComposerTarget};
    use editor_aqua::composer::draft_session::DraftSession;
    use editor_aqua::composer::persistence::{ShutdownOutcome, drain_pending};
    use editor_aqua::composer::spool::DraftSpool;

    if !child_process(
        "persistence::a_kept_image_that_uploads_before_the_composer_shows_its_draft_keeps_the_text",
    ) {
        return;
    }
    let (directory, mut core, _config, _client) = fixture();
    let spool = DraftSpool::new(directory.path());
    run(async {
        let (listener, acceptor, pin) = pinned_server().await;
        let host = connect_host(&mut core, &listener, &pin, "recovering-image");
        let client = core.host(&host).unwrap().client.clone();
        let target = ComposerTarget::NewThread {
            host: host.clone(),
            project: Some(artisan_api::ProjectId::from("project")),
        };

        let mut sessions = DraftSessions::with_spool(spool.clone());
        let mut composer = Composer::new(&core, &mut sessions, target.clone());
        let (mut socket, route, _) = composer_request(&listener, &acceptor).await;
        assert!(route.starts_with("GET /drafts/"), "{route}");
        respond(&mut socket, json!({"revision": 0, "body": null})).await;
        drop(socket);
        wait_for_session(composer.session.as_ref().unwrap(), DraftSession::is_loaded).await;
        composer.sync(&core);
        composer
            .editor
            .borrow_mut()
            .set_text("written before closing");
        composer.edited();
        hand_over_pasted_image(composer.session.as_ref().unwrap(), client);
        drop(composer);
        drop(sessions);
        let mut held = Vec::new();
        let mut routes = Vec::new();
        for _ in 0..2 {
            let (socket, route, _) = draft_or_blob_request(&listener, &acceptor).await;
            routes.push(route);
            held.push(socket);
        }
        routes.sort();
        assert!(routes[0].starts_with("PUT /blobs"), "{routes:?}");
        assert!(routes[1].starts_with("PUT /drafts/"), "{routes:?}");
        assert_eq!(
            drain_pending(Duration::ZERO).await,
            ShutdownOutcome::Incomplete {
                failed: 0,
                timed_out: 1,
                kept_locally: 1,
            }
        );
        drop(held);

        let mut sessions = DraftSessions::with_spool(spool.clone());
        let mut composer = Composer::new(&core, &mut sessions, target);
        let hash = "e".repeat(64);
        let mut loading = None;
        for _ in 0..2 {
            let (mut socket, route, body) = draft_or_blob_request(&listener, &acceptor).await;
            if route.starts_with("PUT /blobs") {
                respond(&mut socket, json!({"hash": hash, "bytes": body.len()})).await;
            } else {
                assert!(route.starts_with("GET /drafts/"), "{route}");
                loading = Some(socket);
            }
        }
        wait_for_session(composer.session.as_ref().unwrap(), |state| {
            state.body().images.len() == 1
        })
        .await;
        composer.sync(&core);
        assert_eq!(
            composer.editor.borrow().text(),
            "written before closing",
            "the text kept with the draft shows although its image finished uploading first"
        );
        assert_eq!(
            composer.attachments.as_ref().unwrap().items.len(),
            1,
            "the image kept with the draft shows beside its text"
        );
        assert_eq!(
            composer
                .session
                .as_ref()
                .unwrap()
                .read(|state| state.body().text.clone()),
            "written before closing",
            "showing the draft does not replace its text"
        );
        drop(loading);
    });
}

#[test]
fn an_image_whose_upload_failed_after_leaving_the_draft_uploads_on_retry() {
    use editor_aqua::composer::draft_session::DraftSession;

    let (_directory, core, config, _client) = fixture();
    let instance = core.host(&config.id).unwrap().instance();
    run(async {
        let (listener, acceptor, client) = server().await;
        let mut sessions = DraftSessions::default();
        let lease = sessions
            .open(config.id.clone(), instance, "new".into(), client.clone())
            .unwrap();
        load(&lease, &listener, &acceptor).await;
        let mut changed = lease.subscribe();
        hand_over_pasted_image(&lease, client);
        let (mut socket, route, _) = request_bytes(&listener, &acceptor).await;
        assert_eq!(route, "PUT /blobs HTTP/1.1");
        respond_status(
            &mut socket,
            "500 Internal Server Error",
            json!({"error": {"code": "internal", "message": "Forge went away"}}),
        )
        .await;
        drop(socket);
        while !lease.read(DraftSession::halted) {
            changed.changed().await.unwrap();
        }
        assert!(
            lease.read(DraftSession::pending),
            "the image is still waiting for Forge after its upload failed"
        );

        lease.retry();
        let (mut socket, route, webp) = request_bytes(&listener, &acceptor).await;
        assert_eq!(
            route, "PUT /blobs HTTP/1.1",
            "Try again uploads the image again"
        );
        assert_eq!(&webp[8..12], b"WEBP");
        let hash = "b".repeat(64);
        respond(&mut socket, json!({"hash": hash, "bytes": webp.len()})).await;
        drop(socket);
        let (mut socket, route, body) = request(&listener, &acceptor).await;
        assert_eq!(route, "PUT /drafts/new HTTP/1.1");
        assert_eq!(body["body"]["images"], json!([hash]));
        respond(
            &mut socket,
            json!({"saved": true, "current": {"revision": 1, "body": body["body"]}}),
        )
        .await;
        drop(socket);
        while lease.read(DraftSession::pending) {
            changed.changed().await.unwrap();
        }
    });
}

#[test]
fn closing_while_an_image_uploads_keeps_the_image_on_this_device_until_forge_takes_it() {
    use editor_aqua::composer::draft_session::DraftSession;
    use editor_aqua::composer::persistence::{ShutdownOutcome, drain_pending};
    use editor_aqua::composer::spool::DraftSpool;

    if !child_process(
        "persistence::closing_while_an_image_uploads_keeps_the_image_on_this_device_until_forge_takes_it",
    ) {
        return;
    }
    let (directory, core, config, _client) = fixture();
    let spool = DraftSpool::new(directory.path());
    let instance = core.host(&config.id).unwrap().instance();
    run(async {
        let (listener, acceptor, client) = server().await;
        let mut sessions = DraftSessions::with_spool(spool.clone());
        let lease = sessions
            .open(config.id.clone(), instance, "new".into(), client.clone())
            .unwrap();
        load(&lease, &listener, &acceptor).await;
        hand_over_pasted_image(&lease, client.clone());
        let (socket, route, _) = request_bytes(&listener, &acceptor).await;
        assert_eq!(route, "PUT /blobs HTTP/1.1");
        drop(lease);
        drop(sessions);
        assert_eq!(
            drain_pending(Duration::ZERO).await,
            ShutdownOutcome::Incomplete {
                failed: 0,
                timed_out: 1,
                kept_locally: 1,
            }
        );
        drop(socket);

        let mut sessions = DraftSessions::with_spool(spool.clone());
        let lease = sessions
            .open(config.id.clone(), instance, "new".into(), client)
            .unwrap();
        let mut changed = lease.subscribe();
        let hash = "c".repeat(64);
        for _ in 0..2 {
            let (mut socket, route, body) = request_bytes(&listener, &acceptor).await;
            match route.as_str() {
                "GET /drafts/new HTTP/1.1" => {
                    respond(&mut socket, json!({"revision": 0, "body": null})).await;
                }
                "PUT /blobs HTTP/1.1" => {
                    assert_eq!(&body[8..12], b"WEBP", "the kept image is uploaded again");
                    respond(&mut socket, json!({"hash": hash, "bytes": body.len()})).await;
                }
                route => panic!("unexpected request {route}"),
            }
        }
        let (mut socket, route, body) = request(&listener, &acceptor).await;
        assert_eq!(route, "PUT /drafts/new HTTP/1.1");
        assert_eq!(
            body["body"]["images"],
            json!([hash]),
            "the image pasted before closing is part of the draft after restarting"
        );
        respond(
            &mut socket,
            json!({"saved": true, "current": {"revision": 1, "body": body["body"]}}),
        )
        .await;
        drop(socket);
        while lease.read(DraftSession::pending) {
            changed.changed().await.unwrap();
        }
        assert_eq!(spool.take(&config.id, "new").unwrap(), None);
        assert_eq!(
            std::fs::read_dir(directory.path().join("drafts"))
                .unwrap()
                .count(),
            0,
            "nothing is left on this device once Forge has the draft"
        );
    });
}

#[test]
fn an_image_still_uploading_when_leaving_the_draft_is_kept() {
    use editor_aqua::composer::attachment::{Attachments, Source};
    use editor_aqua::composer::persistence::SessionEvent;

    let (_directory, core, config, _client) = fixture();
    let instance = core.host(&config.id).unwrap().instance();
    run(async {
        let (listener, acceptor, client) = server().await;
        let mut sessions = DraftSessions::default();
        let mut lease = sessions
            .open(config.id.clone(), instance, "new".into(), client.clone())
            .unwrap();
        load(&lease, &listener, &acceptor).await;
        let mut png = Vec::new();
        image::RgbaImage::from_pixel(4, 4, image::Rgba([200, 40, 40, 255]))
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let mut attachments = Attachments::new(client);
        assert!(attachments.add(Source::Bytes(png), "pasted.png".into()));
        let uploads = attachments.take_unfinished();
        assert_eq!(uploads.len(), 1);
        drop(attachments);
        for upload in uploads {
            lease.adopt_image(upload);
        }
        assert!(lease.read(editor_aqua::composer::draft_session::DraftSession::pending));
        let (mut socket, route, webp) = request_bytes(&listener, &acceptor).await;
        assert_eq!(route, "PUT /blobs HTTP/1.1");
        assert_eq!(&webp[8..12], b"WEBP");
        let hash = "a".repeat(64);
        respond(&mut socket, json!({"hash": hash, "bytes": webp.len()})).await;
        drop(socket);
        let (mut socket, route, body) = request(&listener, &acceptor).await;
        assert_eq!(route, "PUT /drafts/new HTTP/1.1");
        assert_eq!(
            body["body"]["images"],
            json!([hash]),
            "the image pasted before leaving is part of the saved draft"
        );
        respond(
            &mut socket,
            json!({"saved": true, "current": {"revision": 1, "body": body["body"]}}),
        )
        .await;
        drop(socket);
        assert!(lease.drain_events().iter().any(
            |event| matches!(event, SessionEvent::ImageAdopted(adopted) if adopted.0 == hash)
        ));
    });
}

#[test]
fn a_failed_send_keeps_both_a_long_prompt_and_the_follow_up_typed_meanwhile() {
    use editor_aqua::composer::controller::{Composer, ComposerTarget};

    let (directory, mut core, _config, _client) = fixture();
    run(async {
        let (listener, acceptor, pin) = pinned_server().await;
        let identity = Identity::generate("failed-send").unwrap();
        let address = listener.local_addr().unwrap().to_string();
        let host = HostId("failed-send".into());
        test_support::editor::core::connect_client(
            &mut core,
            &HostConfig {
                id: host.clone(),
                name: "Forge".into(),
                address: address.clone(),
                fingerprint: pin.clone(),
                kind: HostKind::Local,
            },
            ForgeClient::new(&address, &pin, &identity).unwrap(),
        );
        let mut sessions = DraftSessions::default();
        let mut composer = Composer::new(
            &core,
            &mut sessions,
            ComposerTarget::NewThread {
                host: host.clone(),
                project: Some(artisan_api::ProjectId::from("project")),
            },
        );
        let mut changed = composer.session.as_ref().unwrap().subscribe();
        let (mut socket, route, _) = composer_request(&listener, &acceptor).await;
        assert!(route.starts_with("GET /drafts/"), "{route}");
        respond(&mut socket, json!({"revision": 0, "body": null})).await;
        drop(socket);
        while !composer
            .session
            .as_ref()
            .unwrap()
            .read(editor_aqua::composer::draft_session::DraftSession::is_loaded)
        {
            changed.changed().await.unwrap();
        }
        composer.sync(&core);

        let prompt = "p".repeat(64 * 1024 - 8);
        composer.config = Some(artisan_api::EngineConfig::new(artisan_api::EngineId::Codex));
        composer.editor.borrow_mut().set_text(&prompt);
        composer.edited();
        assert!(composer.send(core.host(&host).unwrap()));
        composer.editor.borrow_mut().set_text("the follow-up");
        composer.edited();

        let (mut socket, route, body) = composer_request(&listener, &acceptor).await;
        assert!(route.starts_with("PUT /drafts/"), "{route}");
        respond(
            &mut socket,
            json!({"saved": true, "current": {"revision": 1, "body": body["body"]}}),
        )
        .await;
        drop(socket);
        let (mut socket, route, _) = composer_request(&listener, &acceptor).await;
        assert_eq!(route, "POST /threads HTTP/1.1");
        respond_status(
            &mut socket,
            "500 Internal Server Error",
            json!({"error": {"code": "internal", "message": "send rejected"}}),
        )
        .await;
        drop(socket);
        while !composer
            .session
            .as_ref()
            .unwrap()
            .read(editor_aqua::composer::draft_session::DraftSession::send_failed)
        {
            changed.changed().await.unwrap();
        }
        composer.sync(&core);

        assert_eq!(
            composer.editor.borrow().text(),
            prompt,
            "the prompt that failed to send comes back whole"
        );
        let saved = composer
            .session
            .as_ref()
            .unwrap()
            .read(|state| state.body().clone());
        assert_eq!(
            saved.follow_ups[0].text, "the follow-up",
            "what was typed while sending is kept beside it"
        );
        drop(directory);
    });
}

#[test]
fn a_failed_image_upload_during_a_conflict_still_asks_which_draft_to_keep() {
    use editor_aqua::composer::attachment::{Attachments, Source};

    let (_directory, core, config, _client) = fixture();
    let instance = core.host(&config.id).unwrap().instance();
    run(async {
        let (listener, acceptor, client) = server().await;
        let mut sessions = DraftSessions::default();
        let lease = sessions
            .open(config.id.clone(), instance, "new".into(), client.clone())
            .unwrap();
        load(&lease, &listener, &acceptor).await;
        let mut changed = lease.subscribe();
        lease.queue(typed("mine"));
        let (mut socket, route, _) = request(&listener, &acceptor).await;
        assert_eq!(route, "PUT /drafts/new HTTP/1.1");
        respond(
            &mut socket,
            json!({"saved": false, "current": {"revision": 3, "body": {"text": "theirs"}}}),
        )
        .await;
        drop(socket);
        while !lease.read(editor_aqua::composer::draft_session::DraftSession::conflict) {
            changed.changed().await.unwrap();
        }

        let mut png = Vec::new();
        image::RgbaImage::from_pixel(4, 4, image::Rgba([40, 200, 40, 255]))
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let mut attachments = Attachments::new(client);
        assert!(attachments.add(Source::Bytes(png), "pasted.png".into()));
        let uploads = attachments.take_unfinished();
        drop(attachments);
        for upload in uploads {
            lease.adopt_image(upload);
        }
        changed.borrow_and_update();
        let (mut socket, route, _) = request_bytes(&listener, &acceptor).await;
        assert_eq!(route, "PUT /blobs HTTP/1.1");
        respond_status(
            &mut socket,
            "507 Insufficient Storage",
            json!({"code": "internal", "message": "the disk is full"}),
        )
        .await;
        drop(socket);
        changed.changed().await.unwrap();

        assert!(
            lease.read(editor_aqua::composer::draft_session::DraftSession::conflict),
            "the failed upload does not hide the other client's draft"
        );
        lease.queue(typed("mine, typed after the upload failed"));
        assert!(
            lease.read(|state| state.conflict() && state.halted()),
            "the next keystroke does not write over the other client's draft"
        );
        assert!(
            lease
                .read(editor_aqua::composer::draft_session::DraftSession::error)
                .is_some_and(|error| error.contains("another client")),
            "the choice between the two drafts is still offered"
        );
    });
}

#[test]
fn a_full_set_of_open_drafts_refuses_a_new_one_until_one_settles_and_closes() {
    use editor_aqua::composer::persistence::MAX_OPEN_DRAFTS;

    let (_directory, core, config, _client) = fixture();
    let instance = core.host(&config.id).unwrap().instance();
    run(async {
        let (listener, acceptor, client) = server().await;
        let forge = tokio::spawn(async move {
            loop {
                let (mut socket, _, _) = request(&listener, &acceptor).await;
                respond(&mut socket, json!({"revision": 0, "body": null})).await;
            }
        });
        let mut sessions = DraftSessions::default();
        let mut leases: Vec<DraftLease> = (0..MAX_OPEN_DRAFTS)
            .map(|draft| {
                sessions
                    .open(
                        config.id.clone(),
                        instance,
                        format!("thread:{draft}"),
                        client.clone(),
                    )
                    .expect("a slot is free")
            })
            .collect();
        let full = sessions
            .open(
                config.id.clone(),
                instance,
                "thread:new".into(),
                client.clone(),
            )
            .expect_err("every slot holds an open draft");
        assert_eq!(full.host, config.id);
        assert_eq!(full.scope, "thread:new");

        let first = leases.remove(0);
        let mut changed = first.subscribe();
        while !first.read(editor_aqua::composer::draft_session::DraftSession::is_loaded) {
            changed.changed().await.unwrap();
        }
        drop(first);
        test_support::wait::wait_until(
            "a closed draft with nothing left to save to free its slot",
            Duration::from_secs(5),
            || {
                sessions
                    .open(
                        config.id.clone(),
                        instance,
                        "thread:new".into(),
                        client.clone(),
                    )
                    .is_ok()
            },
        );
        forge.abort();
    });
}
