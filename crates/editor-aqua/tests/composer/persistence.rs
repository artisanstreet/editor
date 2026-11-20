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

fn type_into(session: &DraftHandle, body: DraftBody) {
    session.queue(
        body,
        session.read(editor_aqua::composer::draft_session::DraftSession::epoch),
        false,
    );
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
fn rejected_write_hands_the_message_back_and_sending_it_again_keeps_its_id() {
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
        type_into(&lease, typed("preserve this prompt"));
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

        lease.submit(Submission {
            thread: ThreadId("thread".into()),
            create: None,
            message: SendMessage {
                id: ItemId("second-attempt".into()),
                text: "preserve this prompt".into(),
                images: Vec::new(),
                config: None,
                draft: None,
            },
        });
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
    type_into(&old, typed("old pairing"));
    core.remove_host(&config.id).unwrap();
    test_support::editor::core::connect_client(&mut core, &config, client.clone());
    let current_instance = core.host(&config.id).unwrap().instance();
    let current = sessions
        .open(config.id.clone(), current_instance, "new".into(), client)
        .unwrap();
    type_into(&current, typed("current pairing"));

    sessions.retain_hosts(&core);

    assert!(old.read(editor_aqua::composer::draft_session::DraftSession::removed));
    assert!(!current.read(editor_aqua::composer::draft_session::DraftSession::removed));
    type_into(&old, typed("stale edit"));
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
        type_into(&lease, typed("draft survives workspace teardown"));

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
        type_into(&lease, typed("send survives teardown"));
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
        type_into(&lease, typed("stale pairing"));
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
        type_into(&lease, typed("persist before process exit"));
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
        type_into(&lease, typed("server never acknowledges"));
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
        type_into(&lease, typed("local draft"));
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
        type_into(&lease, typed("typed while Forge was unreachable"));
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
        assert!(
            spool.take(&config.id, "new").unwrap().is_none(),
            "the kept draft is forgotten once Forge has it"
        );
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
        assert!(
            spool.take(&host, &scope).unwrap().is_none(),
            "the kept draft is forgotten once Forge has it"
        );
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
fn a_dropped_image_whose_file_is_gone_by_closing_is_kept_with_the_draft() {
    use editor_aqua::composer::attachment::{Attachments, Source};
    use editor_aqua::composer::draft_session::DraftSession;
    use editor_aqua::composer::persistence::{ShutdownOutcome, drain_pending};
    use editor_aqua::composer::spool::DraftSpool;

    if !child_process(
        "persistence::a_dropped_image_whose_file_is_gone_by_closing_is_kept_with_the_draft",
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
        type_into(&lease, typed("text beside a dropped image"));
        let dropped = directory.path().join("dropped.png");
        std::fs::write(&dropped, pasted_png()).unwrap();
        let mut attachments = Attachments::new(client.clone());
        assert!(attachments.add(Source::Path(dropped.clone()), "dropped.png".into()));
        let uploads = attachments.take_unfinished();
        drop(attachments);
        for upload in uploads {
            lease.adopt_image(upload);
        }
        let mut held = Vec::new();
        let mut routes = Vec::new();
        for _ in 0..2 {
            let (socket, route, _) = draft_or_blob_request(&listener, &acceptor).await;
            routes.push(route);
            held.push(socket);
        }
        routes.sort();
        assert_eq!(routes, ["PUT /blobs HTTP/1.1", "PUT /drafts/new HTTP/1.1"]);
        std::fs::remove_file(&dropped).unwrap();
        drop(lease);
        drop(sessions);
        assert_eq!(
            drain_pending(Duration::ZERO).await,
            ShutdownOutcome::Incomplete {
                failed: 0,
                timed_out: 1,
                kept_locally: 1,
            },
            "the draft is kept although the dropped file is gone"
        );
        drop(held);

        let mut sessions = DraftSessions::with_spool(spool.clone());
        let lease = sessions
            .open(config.id.clone(), instance, "new".into(), client)
            .unwrap();
        let mut changed = lease.subscribe();
        let hash = "f".repeat(64);
        let mut revision = 0;
        loop {
            let (mut socket, route, body) = draft_or_blob_request(&listener, &acceptor).await;
            match route.as_str() {
                "GET /drafts/new HTTP/1.1" => {
                    respond(&mut socket, json!({"revision": revision, "body": null})).await;
                }
                "PUT /blobs HTTP/1.1" => {
                    assert_eq!(&body[8..12], b"WEBP", "the dropped image is uploaded again");
                    respond(&mut socket, json!({"hash": hash, "bytes": body.len()})).await;
                }
                "PUT /drafts/new HTTP/1.1" => {
                    let draft: Value = serde_json::from_slice(&body).unwrap();
                    assert_eq!(draft["body"]["text"], "text beside a dropped image");
                    revision += 1;
                    respond(
                        &mut socket,
                        json!({"saved": true, "current": {"revision": revision, "body": draft["body"]}}),
                    )
                    .await;
                    if draft["body"]["images"] == json!([hash]) {
                        break;
                    }
                }
                route => panic!("unexpected request {route}"),
            }
        }
        while lease.read(DraftSession::pending) {
            changed.changed().await.unwrap();
        }
    });
}

#[test]
fn an_image_forge_refuses_comes_back_as_a_failed_image_and_the_draft_keeps_saving() {
    use editor_aqua::composer::controller::{Composer, ComposerTarget};
    use editor_aqua::composer::draft_session::DraftSession;

    let (_directory, mut core, _config, _client) = fixture();
    run(async {
        let (listener, acceptor, pin) = pinned_server().await;
        let host = connect_host(&mut core, &listener, &pin, "refusing");
        let client = core.host(&host).unwrap().client.clone();
        let target = ComposerTarget::NewThread {
            host,
            project: Some(artisan_api::ProjectId::from("project")),
        };
        let mut sessions = DraftSessions::default();
        let mut composer = Composer::new(&core, &mut sessions, target);
        let (mut socket, route, _) = composer_request(&listener, &acceptor).await;
        assert!(route.starts_with("GET /drafts/"), "{route}");
        respond(&mut socket, json!({"revision": 0, "body": null})).await;
        drop(socket);
        wait_for_session(composer.session.as_ref().unwrap(), DraftSession::is_loaded).await;
        composer.sync(&core);

        hand_over_pasted_image(composer.session.as_ref().unwrap(), client);
        let (mut socket, route, _) = draft_or_blob_request(&listener, &acceptor).await;
        assert_eq!(route, "PUT /blobs HTTP/1.1");
        let failure = artisan_api::Failure::Resource(artisan_api::ResourceFailure::TooManyPixels {
            width: 40_000,
            height: 40_000,
            limit: 100_000_000,
        });
        let message = failure.to_string();
        respond_status(
            &mut socket,
            "400 Bad Request",
            serde_json::to_value(artisan_api::ApiError::new(failure, message)).unwrap(),
        )
        .await;
        drop(socket);
        wait_for_session(composer.session.as_ref().unwrap(), |state| !state.pending()).await;
        let session = composer.session.as_ref().unwrap();
        assert!(!session.read(DraftSession::halted));
        assert_eq!(
            session.read(DraftSession::error),
            None,
            "an image Forge will never take is not retried behind a draft error"
        );
        composer.sync(&core);
        assert_eq!(
            composer.attachments.as_ref().unwrap().items.len(),
            1,
            "the refused image is shown in the composer so it can be removed"
        );

        composer.editor.borrow_mut().set_text("still saved");
        composer.edited();
        let (mut socket, route, body) = composer_request(&listener, &acceptor).await;
        assert!(route.starts_with("PUT /drafts/"), "{route}");
        assert_eq!(body["body"]["text"], "still saved");
        respond(
            &mut socket,
            json!({"saved": true, "current": {"revision": 1, "body": body["body"]}}),
        )
        .await;
        drop(socket);
    });
}

async fn answer_empty_draft(listener: &TcpListener, acceptor: &TlsAcceptor, revision: u64) {
    let (mut socket, route, _) = composer_request(listener, acceptor).await;
    assert!(route.starts_with("GET /drafts/"), "{route}");
    respond(&mut socket, json!({"revision": revision, "body": null})).await;
}

fn accepted_message(id: &str, text: &str) -> Value {
    json!({"id": id, "thread_id": "thread", "position": 1, "seq": 1, "run_id": null, "parent_id": null, "status": "done", "created_at": 0, "updated_at": 0, "kind": {"type": "user_message", "text": text}})
}

#[test]
fn a_send_left_unanswered_at_closing_is_finished_instead_of_coming_back_as_a_draft() {
    use artisan_api::{ItemId, SendMessage, ThreadId};
    use editor_aqua::composer::controller::{Composer, ComposerTarget};
    use editor_aqua::composer::draft_session::{DraftSession, Submission};
    use editor_aqua::composer::persistence::{ShutdownOutcome, drain_pending};
    use editor_aqua::composer::spool::DraftSpool;

    if !child_process(
        "persistence::a_send_left_unanswered_at_closing_is_finished_instead_of_coming_back_as_a_draft",
    ) {
        return;
    }
    let (directory, mut core, _config, _client) = fixture();
    let spool = DraftSpool::new(directory.path());
    run(async {
        let (listener, acceptor, pin) = pinned_server().await;
        let host = connect_host(&mut core, &listener, &pin, "sending");
        let instance = core.host(&host).unwrap().instance();
        let client = core.host(&host).unwrap().client.clone();
        let thread = ThreadId::from("thread");
        let target = ComposerTarget::Thread {
            host: host.clone(),
            thread: thread.clone(),
        };
        let scope = target.scope();

        let mut sessions = DraftSessions::with_spool(spool.clone());
        let lease = sessions
            .open(host.clone(), instance, scope.clone(), client)
            .unwrap();
        answer_empty_draft(&listener, &acceptor, 0).await;
        wait_for_session(&lease, DraftSession::is_loaded).await;
        type_into(&lease, typed("send me"));
        lease.submit(Submission {
            thread: thread.clone(),
            create: None,
            message: SendMessage {
                id: ItemId::from("message"),
                text: "send me".into(),
                images: Vec::new(),
                config: None,
                draft: None,
            },
        });
        let (mut socket, route, body) = composer_request(&listener, &acceptor).await;
        assert!(route.starts_with("PUT /drafts/"), "{route}");
        respond(
            &mut socket,
            json!({"saved": true, "current": {"revision": 1, "body": body["body"]}}),
        )
        .await;
        drop(socket);
        let (unanswered, route, body) = composer_request(&listener, &acceptor).await;
        assert_eq!(route, "POST /threads/thread/messages HTTP/1.1");
        assert_eq!(body["id"], "message");
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
        drop(unanswered);

        let mut sessions = DraftSessions::with_spool(spool.clone());
        let mut composer = Composer::new(&core, &mut sessions, target);
        answer_empty_draft(&listener, &acceptor, 2).await;
        let (mut socket, route, body) = composer_request(&listener, &acceptor).await;
        assert_eq!(
            route, "POST /threads/thread/messages HTTP/1.1",
            "the send left unanswered is finished rather than written back as a draft"
        );
        assert_eq!(
            body["id"], "message",
            "it is the same message, so Forge does not take it twice"
        );
        composer.sync(&core);
        assert_eq!(
            composer.editor.borrow().text(),
            "",
            "the message being sent is not offered as a draft"
        );
        assert_eq!(
            body["draft"],
            Value::Null,
            "Forge already cleared the draft that held it"
        );
        respond(&mut socket, accepted_message("message", "send me")).await;
        drop(socket);
        wait_for_session(composer.session.as_ref().unwrap(), |state| {
            state.is_loaded() && !state.pending()
        })
        .await;
        composer.sync(&core);
        assert_eq!(composer.editor.borrow().text(), "");
        assert!(
            spool.take(&host, &scope).unwrap().is_none(),
            "the kept draft is forgotten once Forge has it"
        );
    });
}

#[test]
fn typing_offline_after_a_saved_edit_does_not_ask_which_draft_to_keep() {
    use editor_aqua::composer::draft_session::DraftSession;
    use editor_aqua::composer::persistence::{ShutdownOutcome, drain_pending};
    use editor_aqua::composer::spool::DraftSpool;

    if !child_process(
        "persistence::typing_offline_after_a_saved_edit_does_not_ask_which_draft_to_keep",
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
        type_into(&lease, typed("first"));
        let (mut socket, route, body) = request(&listener, &acceptor).await;
        assert_eq!(route, "PUT /drafts/new HTTP/1.1");
        respond(
            &mut socket,
            json!({"saved": true, "current": {"revision": 1, "body": body["body"]}}),
        )
        .await;
        drop(socket);
        wait_for_session(&lease, |state| !state.pending()).await;
        type_into(&lease, typed("first, then more"));
        let (offline, route, _) = request(&listener, &acceptor).await;
        assert_eq!(route, "PUT /drafts/new HTTP/1.1");
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
        drop(offline);

        let mut sessions = DraftSessions::with_spool(spool.clone());
        let lease = sessions
            .open(config.id.clone(), instance, "new".into(), client)
            .unwrap();
        let (mut socket, route, _) = request(&listener, &acceptor).await;
        assert_eq!(route, "GET /drafts/new HTTP/1.1");
        respond(
            &mut socket,
            json!({"revision": 1, "body": {"text": "first"}}),
        )
        .await;
        drop(socket);
        wait_for_session(&lease, DraftSession::is_loaded).await;
        assert!(
            !lease.read(DraftSession::conflict),
            "the draft Forge holds is the one this client's edit started from"
        );
        let (mut socket, route, body) = request(&listener, &acceptor).await;
        assert_eq!(route, "PUT /drafts/new HTTP/1.1");
        assert_eq!(
            body,
            json!({"expected_revision": 1, "body": {"text": "first, then more"}})
        );
        respond(
            &mut socket,
            json!({"saved": true, "current": {"revision": 2, "body": body["body"]}}),
        )
        .await;
        drop(socket);
        wait_for_session(&lease, |state| !state.pending()).await;
        assert!(
            spool.take(&config.id, "new").unwrap().is_none(),
            "the kept draft is forgotten once Forge has it"
        );
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
        while lease.read(DraftSession::error).is_none() {
            changed.changed().await.unwrap();
        }
        assert!(
            lease.read(DraftSession::pending),
            "the image is still waiting for Forge after its upload failed"
        );
        assert!(
            !lease.read(DraftSession::halted),
            "the failed image does not hold up the draft's own saves"
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
        assert!(
            spool.take(&config.id, "new").unwrap().is_none(),
            "the kept draft is forgotten once Forge has it"
        );
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
        type_into(&lease, typed("mine"));
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
        type_into(&lease, typed("mine, typed after the upload failed"));
        assert!(
            lease.read(|state| state.conflict() && state.halted()),
            "the next keystroke does not write over the other client's draft"
        );
        assert!(
            lease
                .read(editor_aqua::composer::draft_session::DraftSession::error)
                .is_some_and(|error| error.headline().contains("another client")),
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

fn too_many_pixels() -> Value {
    let failure = artisan_api::Failure::Resource(artisan_api::ResourceFailure::TooManyPixels {
        width: 40_000,
        height: 40_000,
        limit: 100_000_000,
    });
    let message = failure.to_string();
    serde_json::to_value(artisan_api::ApiError::new(failure, message)).unwrap()
}

#[test]
fn a_paragraph_shows_on_return_although_the_image_handed_over_with_it_was_refused_first() {
    use editor_aqua::composer::controller::{Composer, ComposerTarget};
    use editor_aqua::composer::draft_session::DraftSession;

    let (directory, mut core, _config, _client) = fixture();
    run(async {
        let (listener, acceptor, pin) = pinned_server().await;
        let host = connect_host(&mut core, &listener, &pin, "handing-over");
        let here = ComposerTarget::NewThread {
            host: host.clone(),
            project: Some(artisan_api::ProjectId::from("here")),
        };
        let elsewhere = ComposerTarget::NewThread {
            host: host.clone(),
            project: Some(artisan_api::ProjectId::from("elsewhere")),
        };
        let mut sessions = DraftSessions::default();
        let mut composer = Composer::new(&core, &mut sessions, here.clone());
        answer_empty_draft(&listener, &acceptor, 0).await;
        let session = DraftHandle::clone(composer.session.as_ref().unwrap());
        wait_for_session(&session, DraftSession::is_loaded).await;
        composer.sync(&core);
        composer.editor.borrow_mut().set_text("a paragraph");
        composer.edited();
        let pasted = directory.path().join("pasted.png");
        std::fs::write(&pasted, pasted_png()).unwrap();
        composer.add_paths(vec![pasted]);

        composer.set_target(&core, &mut sessions, elsewhere);
        for _ in 0..3 {
            let (mut socket, route, body) = draft_or_blob_request(&listener, &acceptor).await;
            if route.starts_with("PUT /blobs") {
                respond_status(&mut socket, "400 Bad Request", too_many_pixels()).await;
            } else if route.starts_with("GET /drafts/") {
                respond(&mut socket, json!({"revision": 0, "body": null})).await;
            } else {
                assert!(route.starts_with("PUT /drafts/"), "{route}");
                let draft: Value = serde_json::from_slice(&body).unwrap();
                respond(
                    &mut socket,
                    json!({"saved": true, "current": {"revision": 1, "body": draft["body"]}}),
                )
                .await;
            }
        }
        wait_for_session(&session, |state| !state.pending()).await;

        composer.set_target(&core, &mut sessions, here);
        composer.sync(&core);
        assert_eq!(
            composer.editor.borrow().text(),
            "a paragraph",
            "the paragraph shows beside the image Forge refused"
        );
        let items = &composer.attachments.as_ref().unwrap().items;
        assert_eq!(
            items.len(),
            1,
            "the refused image shows so it can be removed"
        );
        composer
            .editor
            .borrow_mut()
            .set_text("a paragraph, then a word");
        composer.edited();
        assert_eq!(
            session.read(|state| state.body().text.clone()),
            "a paragraph, then a word",
            "typing adds to the paragraph instead of replacing it"
        );
    });
}

#[expect(
    clippy::future_not_send,
    reason = "the composer is UI-thread state, driven here on the test's one thread"
)]
async fn close_while_sending(
    core: &Core,
    spool: &editor_aqua::composer::spool::DraftSpool,
    target: &editor_aqua::composer::controller::ComposerTarget,
    listener: &TcpListener,
    acceptor: &TlsAcceptor,
) -> (Value, Value) {
    use editor_aqua::composer::controller::Composer;
    use editor_aqua::composer::draft_session::DraftSession;
    use editor_aqua::composer::persistence::{ShutdownOutcome, drain_pending};

    let mut sessions = DraftSessions::with_spool(spool.clone());
    let mut composer = Composer::new(core, &mut sessions, target.clone());
    answer_empty_draft(listener, acceptor, 0).await;
    wait_for_session(composer.session.as_ref().unwrap(), DraftSession::is_loaded).await;
    composer.sync(core);
    composer.config = Some(artisan_api::EngineConfig::new(artisan_api::EngineId::Codex));
    composer.editor.borrow_mut().set_text("send me");
    composer.edited();
    assert!(composer.send(core.host(target.host()).unwrap()));
    let (mut socket, route, body) = composer_request(listener, acceptor).await;
    assert!(route.starts_with("PUT /drafts/"), "{route}");
    let sent = body["body"].clone();
    respond(
        &mut socket,
        json!({"saved": true, "current": {"revision": 1, "body": sent}}),
    )
    .await;
    drop(socket);
    let (unanswered, route, body) = composer_request(listener, acceptor).await;
    assert_eq!(route, "POST /threads HTTP/1.1");
    let thread = body["id"].clone();
    composer.editor.borrow_mut().set_text("typed after sending");
    composer.edited();
    drop(composer);
    drop(sessions);
    assert_eq!(
        drain_pending(Duration::ZERO).await,
        ShutdownOutcome::Incomplete {
            failed: 0,
            timed_out: 1,
            kept_locally: 1,
        }
    );
    drop(unanswered);

    (sent, thread)
}

#[test]
fn text_typed_after_an_unanswered_send_shows_on_reopening_and_survives_the_resend_failing() {
    use editor_aqua::composer::controller::{Composer, ComposerTarget};
    use editor_aqua::composer::draft_session::DraftSession;
    use editor_aqua::composer::spool::DraftSpool;

    if !child_process(
        "persistence::text_typed_after_an_unanswered_send_shows_on_reopening_and_survives_the_resend_failing",
    ) {
        return;
    }
    let (directory, mut core, _config, _client) = fixture();
    let spool = DraftSpool::new(directory.path());
    run(async {
        let (listener, acceptor, pin) = pinned_server().await;
        let host = connect_host(&mut core, &listener, &pin, "resending");
        let target = ComposerTarget::NewThread {
            host: host.clone(),
            project: Some(artisan_api::ProjectId::from("project")),
        };

        let (sent, thread) =
            close_while_sending(&core, &spool, &target, &listener, &acceptor).await;

        let mut sessions = DraftSessions::with_spool(spool.clone());
        let mut composer = Composer::new(&core, &mut sessions, target);
        composer.sync(&core);
        assert_eq!(
            composer.editor.borrow().text(),
            "typed after sending",
            "what was typed after sending shows while the message is still on its way"
        );
        let (mut socket, route, _) = composer_request(&listener, &acceptor).await;
        assert!(route.starts_with("GET /drafts/"), "{route}");
        respond(&mut socket, json!({"revision": 1, "body": sent})).await;
        drop(socket);
        let (mut socket, route, body) = composer_request(&listener, &acceptor).await;
        assert_eq!(route, "POST /threads HTTP/1.1");
        assert_eq!(body["id"], thread, "the unanswered send is finished");
        respond_status(
            &mut socket,
            "500 Internal Server Error",
            json!({"error": {"code": "internal", "message": "send rejected"}}),
        )
        .await;
        drop(socket);
        wait_for_session(
            composer.session.as_ref().unwrap(),
            DraftSession::send_failed,
        )
        .await;
        composer.sync(&core);
        assert_eq!(
            composer.editor.borrow().text(),
            "send me",
            "the message that failed comes back to the composer"
        );
        assert_eq!(
            composer.session.as_ref().unwrap().read(|state| state
                .body()
                .follow_ups
                .first()
                .map(|body| body.text.clone())),
            Some("typed after sending".to_owned()),
            "the later text is kept beside it"
        );

        assert!(composer.send(core.host(&host).unwrap()));
        loop {
            let (mut socket, route, body) = composer_request(&listener, &acceptor).await;
            if route.starts_with("PUT /drafts/") {
                respond(
                    &mut socket,
                    json!({"saved": true, "current": {"revision": 2, "body": body["body"]}}),
                )
                .await;
                continue;
            }
            assert_eq!(route, "POST /threads HTTP/1.1");
            assert_eq!(
                body["id"], thread,
                "sending it again is the same message, so Forge doesn't take it twice"
            );
            break;
        }
    });
}

#[expect(
    clippy::future_not_send,
    reason = "the composer is UI-thread state, driven here on the test's one thread"
)]
async fn send_and_type_over(
    core: &Core,
    composer: &mut editor_aqua::composer::controller::Composer,
    host: &HostId,
    listener: &TcpListener,
    acceptor: &TlsAcceptor,
) -> (TlsStream<TcpStream>, Value) {
    use editor_aqua::composer::draft_session::DraftSession;

    answer_empty_draft(listener, acceptor, 0).await;
    wait_for_session(composer.session.as_ref().unwrap(), DraftSession::is_loaded).await;
    composer.sync(core);
    composer.config = Some(artisan_api::EngineConfig::new(artisan_api::EngineId::Codex));
    composer.editor.borrow_mut().set_text("send me");
    composer.edited();
    assert!(composer.send(core.host(host).unwrap()));
    composer.editor.borrow_mut().set_text("typed meanwhile");
    composer.edited();
    let (mut socket, route, body) = composer_request(listener, acceptor).await;
    assert!(route.starts_with("PUT /drafts/"), "{route}");
    respond(
        &mut socket,
        json!({"saved": true, "current": {"revision": 1, "body": body["body"]}}),
    )
    .await;
    drop(socket);
    let (posting, route, body) = composer_request(listener, acceptor).await;
    assert_eq!(route, "POST /threads HTTP/1.1");
    (posting, body["id"].clone())
}

async fn refuse(mut socket: TlsStream<TcpStream>) {
    respond_status(
        &mut socket,
        "500 Internal Server Error",
        json!({"error": {"code": "internal", "message": "send rejected"}}),
    )
    .await;
}

#[expect(
    clippy::future_not_send,
    reason = "the composer is UI-thread state, driven here on the test's one thread"
)]
async fn resent_thread(
    core: &Core,
    composer: &mut editor_aqua::composer::controller::Composer,
    host: &HostId,
    listener: &TcpListener,
    acceptor: &TlsAcceptor,
    follow_up: &str,
) -> Value {
    assert_eq!(
        composer.editor.borrow().text(),
        "send me",
        "the message that failed is back in the composer"
    );
    assert_eq!(
        composer.session.as_ref().unwrap().read(|state| state
            .body()
            .follow_ups
            .iter()
            .map(|body| body.text.clone())
            .collect::<Vec<_>>()),
        [follow_up],
        "what was typed after sending is kept beside it"
    );
    assert!(composer.send(core.host(host).unwrap()));
    loop {
        let (mut socket, route, body) = composer_request(listener, acceptor).await;
        if route.starts_with("PUT /drafts/") {
            respond(
                &mut socket,
                json!({"saved": true, "current": {"revision": 2, "body": body["body"]}}),
            )
            .await;
            continue;
        }
        if route.starts_with("GET /drafts/") {
            respond(&mut socket, json!({"revision": 1, "body": null})).await;
            continue;
        }
        assert_eq!(route, "POST /threads HTTP/1.1");
        return body["id"].clone();
    }
}

#[test]
fn a_send_that_fails_while_another_thread_is_open_comes_back_and_resends_as_the_same_message() {
    use editor_aqua::composer::controller::{Composer, ComposerTarget};
    use editor_aqua::composer::draft_session::DraftSession;

    let (directory, mut core, _config, _client) = fixture();
    run(async {
        let (listener, acceptor, pin) = pinned_server().await;
        let host = connect_host(&mut core, &listener, &pin, "detached");
        let target = ComposerTarget::NewThread {
            host: host.clone(),
            project: Some(artisan_api::ProjectId::from("project")),
        };
        let mut sessions = DraftSessions::default();
        let mut composer = Composer::new(&core, &mut sessions, target.clone());
        let (posting, thread) =
            send_and_type_over(&core, &mut composer, &host, &listener, &acceptor).await;
        let sending = DraftHandle::clone(composer.session.as_ref().unwrap());

        composer.set_target(
            &core,
            &mut sessions,
            ComposerTarget::NewThread {
                host: host.clone(),
                project: Some(artisan_api::ProjectId::from("elsewhere")),
            },
        );
        refuse(posting).await;
        wait_for_session(&sending, DraftSession::send_failed).await;
        composer.set_target(&core, &mut sessions, target);
        composer.sync(&core);

        assert_eq!(
            resent_thread(
                &core,
                &mut composer,
                &host,
                &listener,
                &acceptor,
                "typed meanwhile"
            )
            .await,
            thread,
            "trying again is the same message, so Forge doesn't take it twice"
        );
        drop(directory);
    });
}

#[expect(
    clippy::future_not_send,
    reason = "the composer is UI-thread state, driven here on the test's one thread"
)]
async fn composer_whose_send_failed(
    core: &Core,
    host: &HostId,
    sessions: &mut DraftSessions,
    listener: &TcpListener,
    acceptor: &TlsAcceptor,
) -> (editor_aqua::composer::view::ComposerOwner, DraftHandle) {
    use editor_aqua::composer::controller::ComposerTarget;
    use editor_aqua::composer::draft_session::DraftSession;

    let owner = editor_aqua::composer::view::ComposerOwner::new(
        core,
        sessions,
        ComposerTarget::NewThread {
            host: host.clone(),
            project: Some(artisan_api::ProjectId::from("project")),
        },
        std::rc::Rc::new(|_| {}),
    );
    let sending = DraftHandle::clone(owner.controller.borrow().session.as_ref().unwrap());
    answer_empty_draft(listener, acceptor, 0).await;
    wait_for_session(&sending, DraftSession::is_loaded).await;
    {
        let mut composer = owner.controller.borrow_mut();
        composer.sync(core);
        composer.config = Some(artisan_api::EngineConfig::new(artisan_api::EngineId::Codex));
        composer.editor.borrow_mut().set_text("send me");
        composer.edited();
        assert!(composer.send(core.host(host).unwrap()));
    }
    let (mut socket, route, body) = composer_request(listener, acceptor).await;
    assert!(route.starts_with("PUT /drafts/"), "{route}");
    respond(
        &mut socket,
        json!({"saved": true, "current": {"revision": 1, "body": body["body"]}}),
    )
    .await;
    drop(socket);
    let (posting, route, _) = composer_request(listener, acceptor).await;
    assert_eq!(route, "POST /threads HTTP/1.1");
    refuse(posting).await;
    wait_for_session(&sending, DraftSession::send_failed).await;
    (owner, sending)
}

#[test]
fn an_image_pasted_after_a_send_fails_but_before_the_composer_catches_up_stays() {
    use editor_aqua::composer::view::ComposerAction;
    use editor_aqua::ui::text::mount::EditorEvent;

    let (directory, mut core, _config, _client) = fixture();
    run(async {
        let (listener, acceptor, pin) = pinned_server().await;
        let host = connect_host(&mut core, &listener, &pin, "pasting-after-failure");
        let mut sessions = DraftSessions::default();
        let (owner, _) =
            composer_whose_send_failed(&core, &host, &mut sessions, &listener, &acceptor).await;

        let image = directory.path().join("pasted.png");
        std::fs::write(&image, pasted_png()).unwrap();
        owner.dispatch_with_change(
            &mut core,
            ComposerAction::Editor(EditorEvent::PastedFiles {
                images: Vec::new(),
                paths: vec![image],
            }),
            None,
        );
        let mut composer = owner.controller.borrow_mut();
        composer.sync(&core);

        assert_eq!(composer.editor.borrow().text(), "send me");
        assert_eq!(
            composer
                .attachments
                .as_ref()
                .map(|attachments| attachments.items.len()),
            Some(1),
            "the image pasted after the failure is still in the composer"
        );
    });
}

#[test]
fn trying_again_before_the_composer_catches_up_with_the_failure_sends_again() {
    use editor_aqua::composer::draft_session::DraftSession;
    use editor_aqua::composer::view::ComposerAction;

    let (_directory, mut core, _config, _client) = fixture();
    run(async {
        let (listener, acceptor, pin) = pinned_server().await;
        let host = connect_host(&mut core, &listener, &pin, "retrying-before-sync");
        let mut sessions = DraftSessions::default();
        let (owner, sending) =
            composer_whose_send_failed(&core, &host, &mut sessions, &listener, &acceptor).await;

        owner.dispatch_with_change(&mut core, ComposerAction::RetryDraft, None);

        assert!(
            sending.read(DraftSession::sending),
            "trying again sends the message instead of doing nothing"
        );
    });
}

#[test]
fn a_send_that_fails_before_the_reopened_composer_first_shows_comes_back_with_its_identity() {
    use editor_aqua::composer::controller::{Composer, ComposerTarget};
    use editor_aqua::composer::draft_session::DraftSession;
    use editor_aqua::composer::spool::DraftSpool;

    if !child_process(
        "persistence::a_send_that_fails_before_the_reopened_composer_first_shows_comes_back_with_its_identity",
    ) {
        return;
    }
    let (directory, mut core, _config, _client) = fixture();
    let spool = DraftSpool::new(directory.path());
    run(async {
        let (listener, acceptor, pin) = pinned_server().await;
        let host = connect_host(&mut core, &listener, &pin, "unshown");
        let target = ComposerTarget::NewThread {
            host: host.clone(),
            project: Some(artisan_api::ProjectId::from("project")),
        };
        let (sent, thread) =
            close_while_sending(&core, &spool, &target, &listener, &acceptor).await;

        let mut sessions = DraftSessions::with_spool(spool.clone());
        let mut composer = Composer::new(&core, &mut sessions, target);
        let (mut socket, route, _) = composer_request(&listener, &acceptor).await;
        assert!(route.starts_with("GET /drafts/"), "{route}");
        respond(&mut socket, json!({"revision": 1, "body": sent})).await;
        drop(socket);
        let (posting, route, body) = composer_request(&listener, &acceptor).await;
        assert_eq!(route, "POST /threads HTTP/1.1");
        assert_eq!(body["id"], thread, "the unanswered send is finished");
        refuse(posting).await;
        wait_for_session(
            composer.session.as_ref().unwrap(),
            DraftSession::send_failed,
        )
        .await;
        composer.sync(&core);
        assert_eq!(
            resent_thread(
                &core,
                &mut composer,
                &host,
                &listener,
                &acceptor,
                "typed after sending"
            )
            .await,
            thread,
            "trying again is the same message"
        );
    });
}

#[test]
fn a_send_that_failed_unseen_before_closing_comes_back_on_reopening_with_its_identity() {
    use editor_aqua::composer::controller::{Composer, ComposerTarget};
    use editor_aqua::composer::draft_session::DraftSession;
    use editor_aqua::composer::persistence::{ShutdownOutcome, drain_pending};
    use editor_aqua::composer::spool::DraftSpool;

    if !child_process(
        "persistence::a_send_that_failed_unseen_before_closing_comes_back_on_reopening_with_its_identity",
    ) {
        return;
    }
    let (directory, mut core, _config, _client) = fixture();
    let spool = DraftSpool::new(directory.path());
    run(async {
        let (listener, acceptor, pin) = pinned_server().await;
        let host = connect_host(&mut core, &listener, &pin, "released");
        let target = ComposerTarget::NewThread {
            host: host.clone(),
            project: Some(artisan_api::ProjectId::from("project")),
        };
        let mut sessions = DraftSessions::with_spool(spool.clone());
        let mut composer = Composer::new(&core, &mut sessions, target.clone());
        let (posting, thread) =
            send_and_type_over(&core, &mut composer, &host, &listener, &acceptor).await;
        let sending = DraftHandle::clone(composer.session.as_ref().unwrap());
        drop(composer);
        refuse(posting).await;
        wait_for_session(&sending, DraftSession::send_failed).await;
        drop(sending);
        drop(sessions);
        assert_eq!(
            drain_pending(Duration::ZERO).await,
            ShutdownOutcome::Incomplete {
                failed: 1,
                timed_out: 0,
                kept_locally: 1,
            }
        );

        let mut sessions = DraftSessions::with_spool(spool.clone());
        let mut composer = Composer::new(&core, &mut sessions, target);
        composer.sync(&core);
        assert_eq!(
            resent_thread(
                &core,
                &mut composer,
                &host,
                &listener,
                &acceptor,
                "typed meanwhile"
            )
            .await,
            thread,
            "trying again after reopening is the same message"
        );
    });
}

#[test]
fn an_image_pasted_while_another_clients_draft_loads_stays_and_asks_which_to_keep() {
    use editor_aqua::composer::controller::{Composer, ComposerTarget};
    use editor_aqua::composer::draft_session::DraftSession;

    let (directory, mut core, _config, _client) = fixture();
    run(async {
        let (listener, acceptor, pin) = pinned_server().await;
        let host = connect_host(&mut core, &listener, &pin, "pasting");
        let mut sessions = DraftSessions::default();
        let mut composer = Composer::new(
            &core,
            &mut sessions,
            ComposerTarget::NewThread {
                host,
                project: Some(artisan_api::ProjectId::from("project")),
            },
        );
        let image = directory.path().join("pasted.png");
        std::fs::write(&image, pasted_png()).unwrap();
        composer.add_paths(vec![image]);

        let (mut socket, route, _) = composer_request(&listener, &acceptor).await;
        assert!(route.starts_with("GET /drafts/"), "{route}");
        respond(
            &mut socket,
            json!({"revision": 3, "body": {"text": "theirs"}}),
        )
        .await;
        drop(socket);
        wait_for_session(composer.session.as_ref().unwrap(), DraftSession::is_loaded).await;
        composer.sync(&core);

        assert_eq!(
            composer
                .attachments
                .as_ref()
                .map(|attachments| attachments.items.len()),
            Some(1),
            "the image being uploaded is still in the composer"
        );
        assert!(
            composer
                .session
                .as_ref()
                .unwrap()
                .read(DraftSession::conflict),
            "the user chooses which draft to keep"
        );
    });
}

#[test]
fn the_notice_about_an_unreadable_kept_draft_shows_once() {
    use editor_aqua::composer::controller::{Composer, ComposerTarget};
    use editor_aqua::composer::spool::DraftSpool;

    let (directory, mut core, _config, _client) = fixture();
    let spool = DraftSpool::new(directory.path());
    run(async {
        let (listener, _acceptor, pin) = pinned_server().await;
        let host = connect_host(&mut core, &listener, &pin, "notice");
        let drafts = directory.path().join("drafts");
        std::fs::create_dir_all(&drafts).unwrap();
        std::fs::write(drafts.join("notice-new_3aproject.json"), b"{ not json").unwrap();
        let target = ComposerTarget::NewThread {
            host: host.clone(),
            project: Some(artisan_api::ProjectId::from("project")),
        };
        let mut sessions = DraftSessions::with_spool(spool);
        let mut composer = Composer::new(&core, &mut sessions, target.clone());
        composer.sync(&core);
        assert!(
            composer
                .notice
                .as_ref()
                .is_some_and(|notice| notice.headline().contains("couldn't be read")),
            "{:?}",
            composer.notice
        );

        composer.set_target(
            &core,
            &mut sessions,
            ComposerTarget::NewThread {
                host,
                project: Some(artisan_api::ProjectId::from("elsewhere")),
            },
        );
        composer.set_target(&core, &mut sessions, target);
        composer.sync(&core);
        assert_eq!(
            composer.notice, None,
            "coming back to the draft doesn't repeat the notice"
        );
    });
}
