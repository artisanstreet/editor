use aqua::images::ImageAssets;
use artisan_api::{BlobHash, Event, GlobalPrompt};
use axum::http::StatusCode;
use editor_aqua::core::{
    BlobImage, Core, CoreEvent, EditorFiles, HostEvent, HostId, runtime::runtime,
};
use editor_sources::SourceProviderExt;
use std::sync::Arc;
use std::time::{Duration, Instant};
use test_support::editor::account::{email_of, fail_syncs, hold_syncs};
use test_support::editor::core::{connect, stub_host, test_core};
use test_support::editor::forge::{StubForge, empty_state, png};

const SETTLED_WITHIN: Duration = Duration::from_secs(10);

fn host(core: &mut Core, id: &str) -> (HostId, StubForge) {
    let forge = runtime().block_on(StubForge::serve(empty_state(), axum::Router::new()));
    let config = stub_host(id, id, &forge);
    connect(core, &config);
    wait_live(core, &config.id);
    (config.id, forge)
}

fn wait_live(core: &mut Core, id: &HostId) {
    let deadline = Instant::now() + SETTLED_WITHIN;
    while !core
        .host(id)
        .is_some_and(editor_aqua::core::HostData::is_live)
    {
        assert!(Instant::now() < deadline, "host {id} never went live");
        core.drain_with(|_, _| {});
        std::thread::yield_now();
    }
}

fn core() -> (tempfile::TempDir, Core) {
    let directory = tempfile::tempdir().expect("a temporary directory is created");
    let core = test_core(directory.path());
    (directory, core)
}

fn account(core: &Core) -> String {
    core.account()
        .profile()
        .expect("the test account is signed in")
        .account_id
        .clone()
}

fn flush_initial(core: &mut Core) {
    let deadline = Instant::now() + SETTLED_WITHIN;
    while core.pending_work() > 0 {
        assert!(Instant::now() < deadline, "Core startup did not settle");
        core.drain_with(|_, _| {});
        std::thread::yield_now();
    }
}

#[track_caller]
fn admitted(core: &Core, count: usize) {
    let deadline = Instant::now() + SETTLED_WITHIN;
    while core.pending_work() < count {
        assert!(
            Instant::now() < deadline,
            "only {} of {count} transitions were admitted",
            core.pending_work()
        );
        std::thread::yield_now();
    }
}

fn setting(key: &str, value: serde_json::Value) -> Event {
    Event::Setting {
        key: key.into(),
        value,
    }
}

fn prompt(text: &str) -> GlobalPrompt {
    GlobalPrompt {
        version: 1,
        text: text.into(),
        candidates: Vec::new(),
        engines: Vec::new(),
    }
}

fn load_projections(core: &mut Core, id: &HostId) {
    let host = core.host_mut(id).expect("the test host is connected");
    host.load_prompt(false);
    host.load_skill_catalog(false);
}

#[test]
fn admitted_stream_and_projection_results_apply_in_order_before_invalidation() {
    let (_directory, mut core) = core();
    let (id, forge) = host(&mut core, "first");
    load_projections(&mut core, &id);
    flush_initial(&mut core);
    let prompt_request = forge.take("GET", "/prompt");
    let catalog_request = forge.take("GET", "/skills/catalog");
    prompt_request.respond(prompt("before"));
    admitted(&core, 1);
    forge.publish(setting("prompt_revision", 2.into()));
    admitted(&core, 2);
    catalog_request.respond(Vec::<artisan_api::CatalogSkill>::new());
    admitted(&core, 3);
    forge.publish(setting("skills_revision", 2.into()));
    admitted(&core, 4);
    let mut states = Vec::new();
    for _ in 0..4 {
        core.drain_with(|core, event| {
            if matches!(
                event,
                CoreEvent::Host {
                    event: HostEvent::Changed,
                    ..
                }
            ) {
                let host = core.host(&id).unwrap();
                states.push((host.global_prompt.is_some(), host.skill_catalog.is_some()));
            }
        });
    }
    assert_eq!(
        states,
        [(true, false), (false, false), (false, true), (false, false)]
    );
}

#[test]
fn invalidation_discards_already_admitted_stale_projection_results() {
    let (_directory, mut core) = core();
    let (id, forge) = host(&mut core, "first");
    load_projections(&mut core, &id);
    flush_initial(&mut core);
    let prompt_request = forge.take("GET", "/prompt");
    let catalog_request = forge.take("GET", "/skills/catalog");
    forge.publish(setting("prompt_revision", 2.into()));
    admitted(&core, 1);
    prompt_request.respond(prompt("stale"));
    admitted(&core, 2);
    forge.publish(setting("skills_revision", 2.into()));
    admitted(&core, 3);
    catalog_request.respond(Vec::<artisan_api::CatalogSkill>::new());
    admitted(&core, 4);
    for _ in 0..4 {
        core.drain_with(|_, _| {});
    }
    let host = core.host(&id).unwrap();
    assert!(host.global_prompt.is_none());
    assert!(host.skill_catalog.is_none());
}

#[test]
fn hosts_and_image_completions_expose_each_admitted_state() {
    let (_directory, mut core) = core();
    let (first, first_forge) = host(&mut core, "first");
    let (second, second_forge) = host(&mut core, "second");
    let a = BlobHash("a".into());
    let b = BlobHash("b".into());
    core.host_mut(&first).unwrap().load_image(a.clone(), false);
    core.host_mut(&first).unwrap().load_image(b.clone(), false);
    flush_initial(&mut core);
    let a_request = first_forge.take("GET", "/blobs/a");
    let b_request = first_forge.take("GET", "/blobs/b");
    fail_syncs(&account(&core), StatusCode::SERVICE_UNAVAILABLE);
    second_forge.publish(setting("order", 1.into()));
    admitted(&core, 1);
    core.account_mut().refresh();
    admitted(&core, 2);
    a_request.fail(StatusCode::NOT_FOUND, "not_found", "no such blob");
    admitted(&core, 3);
    first_forge.publish(setting("order", 2.into()));
    admitted(&core, 4);
    b_request.fail(StatusCode::NOT_FOUND, "not_found", "no such blob");
    admitted(&core, 5);
    second_forge.publish(setting("order", 3.into()));
    admitted(&core, 6);
    let mut snapshots = Vec::new();
    let mut order = Vec::new();
    let deadline = Instant::now() + SETTLED_WITHIN;
    while order.len() < 6 {
        assert!(
            Instant::now() < deadline,
            "admitted transitions did not arrive"
        );
        core.drain_with(|core, event| {
            if matches!(&event, CoreEvent::Account(_)) {
                order.push("account".to_owned());
            }
            if let CoreEvent::Host { host, .. } = event {
                order.push(host.0.clone());
                let image_host = core.host(&first).unwrap();
                snapshots.push((
                    host,
                    matches!(image_host.image(&a), Some(BlobImage::Failed(_))),
                    matches!(image_host.image(&b), Some(BlobImage::Failed(_))),
                ));
            }
        });
    }
    assert_eq!(
        order,
        ["second", "account", "first", "first", "first", "second"]
    );
    assert_eq!(
        snapshots,
        [
            (second.clone(), false, false),
            (first.clone(), true, false),
            (first.clone(), true, false),
            (first, true, true),
            (second, true, true)
        ]
    );
}

fn decode(core: &mut Core, forge: &StubForge, id: &HostId, hash: &BlobHash, shade: u8) {
    core.host_mut(id)
        .expect("the test host is connected")
        .load_image(hash.clone(), false);
    forge
        .take("GET", &format!("/blobs/{hash}"))
        .respond_bytes("image/png", png(shade));
    let deadline = Instant::now() + SETTLED_WITHIN;
    while !matches!(
        core.host(id)
            .expect("the test host is connected")
            .image(hash),
        Some(BlobImage::Ready(_))
    ) {
        assert!(Instant::now() < deadline, "image {hash} never decoded");
        core.drain_with(|_, _| {});
        std::thread::yield_now();
    }
}

#[test]
fn image_facade_observes_current_results_without_retaining_host_or_evicted_images() {
    let (_directory, mut core) = core();
    core.set_image_assets(ImageAssets::default());
    let (id, forge) = host(&mut core, "images");
    flush_initial(&mut core);
    let facade = core.host(&id).unwrap().blob_images();
    let first = BlobHash("first".into());
    assert!(facade.lookup(&first).is_none());
    core.host_mut(&id).unwrap().load_image(first.clone(), false);
    assert!(matches!(facade.lookup(&first), Some(BlobImage::Loading)));
    forge
        .take("GET", "/blobs/first")
        .respond_bytes("image/png", png(10));
    let deadline = Instant::now() + SETTLED_WITHIN;
    while matches!(facade.lookup(&first), Some(BlobImage::Loading)) {
        assert!(Instant::now() < deadline, "first image did not arrive");
        core.drain_with(|_, _| {});
        std::thread::yield_now();
    }
    let Some(BlobImage::Ready(first_image)) = facade.lookup(&first) else {
        panic!("the first image decoded");
    };
    for index in 0..32 {
        decode(
            &mut core,
            &forge,
            &id,
            &BlobHash(format!("other-{index}")),
            30,
        );
    }
    assert!(matches!(facade.lookup(&first), Some(BlobImage::Ready(_))));
    drop(first_image);
    assert!(facade.lookup(&first).is_none());
    let last = BlobHash("other-31".into());
    assert!(matches!(facade.lookup(&last), Some(BlobImage::Ready(_))));
    core.remove_host(&id).unwrap();
    assert!(facade.lookup(&last).is_none());
    let (_, _replacement_forge) = host(&mut core, "images");
    assert!(facade.lookup(&last).is_none());
    let replacement = core.host(&id).unwrap().blob_images();
    assert!(replacement.lookup(&last).is_none());
}

fn wait_gate(core: &mut Core, ready: bool, what: &str) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while matches!(
        core.entry_gate(),
        editor_aqua::core::EntryGate::Ready { .. }
    ) != ready
    {
        assert!(Instant::now() < deadline, "{what}");
        core.drain_with(|_, _| {});
        std::thread::yield_now();
    }
}

fn sign_in_again(core: &mut Core, account: &str) {
    core.account_mut()
        .request_link(&email_of(account), "Test".into());
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        assert!(Instant::now() < deadline, "the magic link never signed in");
        core.drain_with(|_, _| {});
        if let editor_aqua::core::AccountPhase::Error { message } = core.account().phase() {
            panic!("signing in again failed: {message}");
        }
        if matches!(
            core.account().phase(),
            editor_aqua::core::AccountPhase::EmailApproved { .. }
        ) {
            break;
        }
        std::thread::yield_now();
    }
    core.account_mut().finish_sign_in(None);
    wait_gate(core, true, "the gate did not reopen");
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one gate and instance scenario walked step by step"
)]
fn source_provider_tracks_gate_and_host_instance_without_reviving() {
    let (_directory, mut core) = core();
    let (id, _forge) = host(&mut core, "sources");
    flush_initial(&mut core);
    let instance = core.host(&id).unwrap().instance();
    let provider = core.source_provider(&id, instance).unwrap();
    let source = artisan_api::ContentRef::from_text("stored source");
    let collection = artisan_api::CollectionRef {
        source: source.clone(),
        count: 1,
    };
    assert!(provider.open_content(source.clone()).is_some());
    assert!(
        provider
            .index_range(
                source.clone(),
                artisan_api::SourceIndexFormat::Markdown,
                None,
                64
            )
            .is_some()
    );
    assert!(provider.content_range(source.clone(), 0, 65536).is_some());
    assert!(
        provider
            .open_collection::<String>(collection.clone())
            .is_some()
    );
    let account = account(&core);
    core.account_mut().sign_out();
    let deadline = Instant::now() + SETTLED_WITHIN;
    while matches!(
        core.entry_gate(),
        editor_aqua::core::EntryGate::Ready { .. }
    ) {
        assert!(Instant::now() < deadline, "gate did not close");
        core.drain_with(|core, event| {
            if matches!(event, CoreEvent::AccessChanged)
                && !matches!(
                    core.entry_gate(),
                    editor_aqua::core::EntryGate::Ready { .. }
                )
            {
                assert!(!provider.is_available());
            }
        });
    }
    assert!(!provider.is_available());
    assert!(
        provider
            .index_range(
                source.clone(),
                artisan_api::SourceIndexFormat::Markdown,
                None,
                64
            )
            .is_none()
    );
    assert!(provider.content_range(source.clone(), 0, 65536).is_none());
    assert!(provider.open_content(source.clone()).is_none());
    assert!(
        provider
            .open_collection::<String>(collection.clone())
            .is_none()
    );
    sign_in_again(&mut core, &account);
    assert!(
        core.source_provider(&id, instance).is_none(),
        "signing back in must not revive the signed-out host instance"
    );
    assert!(!provider.is_available());
    wait_live(&mut core, &id);
    let replacement_instance = core.host(&id).unwrap().instance();
    assert_ne!(
        replacement_instance, instance,
        "the host restored after sign-in must have a new lifetime"
    );
    let refreshed = core.source_provider(&id, replacement_instance).unwrap();
    assert!(refreshed.open_content(source.clone()).is_some());
    core.remove_host(&id).unwrap();
    assert!(!refreshed.is_available());
    assert!(
        refreshed
            .index_range(
                source.clone(),
                artisan_api::SourceIndexFormat::Literal,
                None,
                64
            )
            .is_none()
    );
    assert!(refreshed.content_range(source.clone(), 0, 65536).is_none());
    let (_, _replacement_forge) = host(&mut core, "sources");
    assert!(
        core.source_provider(&id, replacement_instance).is_none(),
        "recreating the same host must not revive its removed lifetime"
    );
    assert!(refreshed.open_collection::<String>(collection).is_none());
    assert!(
        core.source_provider(&id, core.host(&id).unwrap().instance())
            .unwrap()
            .open_content(source)
            .is_some()
    );
}

#[test]
fn peeked_head_keeps_capacity_until_delivery_and_cancelled_senders_release_it() {
    use editor_aqua::core::delivery::{AdmissionOrder, Delivery};
    use std::{
        num::NonZeroUsize,
        sync::atomic::{AtomicUsize, Ordering},
        time::Duration,
    };
    use test_support::poll::poll_once;
    let mut wake = aqua::mailbox::Mailbox::new(NonZeroUsize::MIN);
    let wakes = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&wakes);
    wake.set_waker(Arc::new(move || {
        observed.fetch_add(1, Ordering::SeqCst);
    }));
    let mut delivery =
        Delivery::with_order(NonZeroUsize::MIN, wake.sender(), AdmissionOrder::default());
    let sender = delivery.sender();
    runtime().block_on(async {
        sender.send(1).await.unwrap();
        assert!(delivery.peek_order().is_some());
        assert_eq!(wake.drain(), vec![()]);
        let mut blocked = Box::pin(sender.send(2));
        assert!(
            poll_once(blocked.as_mut()).is_pending(),
            "a send beyond capacity waits for the peeked head"
        );
        drop(blocked);
        assert_eq!(delivery.drain_one(), Some(1));
        assert!(wake.drain().is_empty());
        tokio::time::timeout(Duration::from_secs(1), sender.send(3))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(delivery.drain_one(), Some(3));
        assert!(wakes.load(Ordering::SeqCst) >= 2);
        wake.drain();
        sender.send(4).await.unwrap();
        assert!(delivery.peek_order().is_some());
        let mut waiting = Box::pin(sender.send(5));
        assert!(
            poll_once(waiting.as_mut()).is_pending(),
            "a send beyond capacity waits for the peeked head"
        );
        drop(delivery);
        let error = tokio::time::timeout(Duration::from_secs(1), waiting)
            .await
            .unwrap()
            .unwrap_err();
        assert_eq!(error.0, 5);
    });
}

#[test]
fn account_retry_waits_for_ui_delivery_and_stale_acknowledgement_closes() {
    let (_directory, mut core) = core();
    flush_initial(&mut core);
    let syncs = hold_syncs(&account(&core));
    core.account_mut().refresh();
    syncs
        .take()
        .fail(StatusCode::SERVICE_UNAVAILABLE, "internal", "Core is down");
    admitted(&core, 1);
    assert!(
        syncs.wait(Duration::from_millis(2500)).is_none(),
        "the account retried before the failure reached the UI"
    );
    let mut observed = false;
    while core.pending_work() > 0 {
        core.drain_with(|_, event| {
            if matches!(event, CoreEvent::Account(_)) {
                observed = true;
            }
        });
    }
    assert!(observed);
    let drained_at = Instant::now();
    let retry = syncs.take();
    assert!(
        drained_at.elapsed() >= Duration::from_secs(2),
        "the retry waits its delay after the UI saw the failure"
    );
    core.account_mut().refresh();
    retry.fail(StatusCode::SERVICE_UNAVAILABLE, "internal", "Core is down");
    syncs
        .take()
        .fail(StatusCode::SERVICE_UNAVAILABLE, "internal", "Core is down");
    admitted(&core, 1);
    let mut delivered = 0;
    let deadline = Instant::now() + SETTLED_WITHIN;
    while core.pending_work() > 0 {
        assert!(Instant::now() < deadline, "the current sync never settled");
        core.drain_with(|_, event| {
            if matches!(event, CoreEvent::Account(_)) {
                delivered += 1;
            }
        });
    }
    assert_eq!(
        delivered, 1,
        "only the current sync reaches the UI; the superseded one closed"
    );
}

#[test]
fn magic_link_poll_interval_starts_when_pending_state_reaches_ui() {
    use editor_aqua::core::{AccountPhase, Services};
    use std::{
        io::{BufRead, BufReader, Write},
        net::TcpListener,
        sync::mpsc,
        time::{Duration, Instant},
    };
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let (seen, requests) = mpsc::channel();
    std::thread::spawn(move || {
        for stream in listener.incoming().take(2) {
            let mut stream = stream.unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let path = line.split_whitespace().nth(1).unwrap().to_owned();
            loop {
                line.clear();
                if reader.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                    break;
                }
            }
            let body = match path.as_str() {
                "/v1/auth/magic/request" => {
                    r#"{"request_id":"logreq_1","request_secret":"SECRET","expires_at":"2026-10-28T01:00:00.000Z"}"#
                }
                "/v1/auth/magic/status" => {
                    r#"{"request_id":"logreq_1","status":"pending","totp_required":false,"email":"test@example.invalid","expires_at":"2026-10-28T01:00:00.000Z"}"#
                }
                _ => panic!("unexpected request {path}"),
            };
            write!(stream, "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}", body.len(), body).unwrap();
            stream.flush().unwrap();
            seen.send((path, Instant::now())).unwrap();
        }
    });
    let directory = tempfile::tempdir().unwrap();
    let services = Services::new(
        &url,
        editor_aqua::core::Ae::new(test_support::editor::account::ScriptedAe::new(|args| {
            Err(editor_aqua::core::ae::AeError::Failed {
                command: args.join(" "),
                detail: "no ae".to_owned(),
            })
        })),
        editor_aqua::core::RelayRoute::Off,
        editor_aqua::core::Clock::system(),
    );
    let mut core = Core::with_services(
        EditorFiles {
            dir: directory.path().to_owned(),
        },
        services,
    )
    .unwrap();
    flush_initial(&mut core);
    core.account_mut()
        .request_link("test@example.invalid", "Test".into());
    assert_eq!(
        requests.recv_timeout(Duration::from_secs(5)).unwrap().0,
        "/v1/auth/magic/request"
    );
    requests
        .recv_timeout(Duration::from_millis(3200))
        .unwrap_err();
    let mut pending_seen = false;
    let drained_at = Instant::now();
    let deadline = drained_at + Duration::from_secs(5);
    while !pending_seen && Instant::now() < deadline {
        core.drain_with(|core, event| {
            if matches!(event, CoreEvent::Account(_))
                && matches!(core.account().phase(), AccountPhase::Pending { .. })
            {
                pending_seen = true;
            }
        });
        if !pending_seen {
            std::thread::yield_now();
        }
    }
    assert!(pending_seen);
    let (path, polled_at) = requests.recv_timeout(Duration::from_secs(15)).unwrap();
    assert_eq!(path, "/v1/auth/magic/status");
    assert!(polled_at.duration_since(drained_at) >= Duration::from_secs(3));
}

#[test]
fn observer_mutations_dispatch_before_next_transition_and_nested_drain_is_deferred() {
    let (_directory, mut core) = core();
    let (id, forge) = host(&mut core, "first");
    flush_initial(&mut core);
    forge.publish(setting("first", 1.into()));
    forge.publish(setting("second", 2.into()));
    admitted(&core, 2);
    let mut events = Vec::new();
    let mut nested = false;
    core.drain_with(|core, event| match event {
        CoreEvent::Host {
            event: HostEvent::Changed,
            ..
        } => {
            events.push("host-first");
            core.set_setting("browser", "Helium".into())
                .expect("the browser setting saves");
            core.drain_with(|_, _| nested = true);
        }
        CoreEvent::SettingsChanged => events.push("settings"),
        CoreEvent::ScreenChanged
        | CoreEvent::HostsChanged
        | CoreEvent::AccessChanged
        | CoreEvent::McpSignInChanged
        | CoreEvent::OpenUrl(_)
        | CoreEvent::Account(_)
        | CoreEvent::Host { .. } => {}
    });
    assert!(!nested);
    assert_eq!(events, ["host-first", "settings"]);
    assert_eq!(core.host(&id).unwrap().setting("second"), None);
    core.drain_with(|_, event| {
        if matches!(
            event,
            CoreEvent::Host {
                event: HostEvent::Changed,
                ..
            }
        ) {
            events.push("host-second");
        }
    });
    assert_eq!(events, ["host-first", "settings", "host-second"]);
}

#[test]
fn observer_effect_follows_sibling_effects_from_the_same_transition() {
    let (_directory, mut core) = core();
    let (_id, forge) = host(&mut core, "first");
    flush_initial(&mut core);
    forge.publish(Event::ThreadRemoved {
        thread_id: "removed".into(),
    });
    admitted(&core, 1);
    let mut events = Vec::new();
    core.drain_with(|core, event| match event {
        CoreEvent::Host {
            event: HostEvent::ThreadChanged { .. },
            ..
        } => {
            events.push("thread");
            core.set_setting("browser", "Helium".into())
                .expect("the browser setting saves");
        }
        CoreEvent::Host {
            event: HostEvent::Changed,
            ..
        } => events.push("host"),
        CoreEvent::SettingsChanged => events.push("settings"),
        CoreEvent::ScreenChanged
        | CoreEvent::HostsChanged
        | CoreEvent::AccessChanged
        | CoreEvent::McpSignInChanged
        | CoreEvent::OpenUrl(_)
        | CoreEvent::Account(_)
        | CoreEvent::Host { .. } => {}
    });
    assert_eq!(events, ["thread", "host", "settings"]);
}

#[test]
fn observer_cross_domain_effects_follow_existing_siblings_and_see_final_state() {
    let (_directory, mut core) = core();
    let (id, forge) = host(&mut core, "first");
    flush_initial(&mut core);
    forge.publish(Event::ThreadRemoved {
        thread_id: "removed".into(),
    });
    admitted(&core, 1);
    let mut events = Vec::new();
    core.drain_with(|core, event| match event {
        CoreEvent::Host {
            event: HostEvent::ThreadChanged { .. },
            ..
        } => {
            events.push("thread");
            core.host_mut(&id).unwrap().load_prompt(false);
            core.set_setting("browser", "Helium".into())
                .expect("the browser setting saves");
        }
        CoreEvent::Host {
            event: HostEvent::Changed,
            ..
        } => {
            assert!(core.host(&id).unwrap().prompt_load().loading);
            events.push("host");
        }
        CoreEvent::SettingsChanged => events.push("settings"),
        CoreEvent::ScreenChanged
        | CoreEvent::HostsChanged
        | CoreEvent::AccessChanged
        | CoreEvent::McpSignInChanged
        | CoreEvent::OpenUrl(_)
        | CoreEvent::Account(_)
        | CoreEvent::Host { .. } => {}
    });
    assert_eq!(events, ["thread", "host", "host", "settings"]);
}

#[test]
fn host_list_changes_only_when_a_host_crosses_the_live_boundary() {
    let (_directory, mut core) = core();
    let (_id, forge) = host(&mut core, "first");
    flush_initial(&mut core);
    forge.publish(setting("initial", true.into()));
    admitted(&core, 1);
    core.drain_with(|_, _| {});
    forge.close_streams();
    admitted(&core, 3);
    let mut counts = Vec::new();
    for _ in 0..3 {
        let mut count = 0;
        core.drain_with(|_, event| {
            if matches!(event, CoreEvent::HostsChanged) {
                count += 1;
            }
        });
        counts.push(count);
    }
    assert_eq!(counts, [1, 0, 1]);
}

#[test]
fn replacing_host_in_observer_stops_remaining_events_from_old_instance() {
    let (_directory, mut core) = core();
    let (id, forge) = host(&mut core, "first");
    flush_initial(&mut core);
    let original = core.host(&id).unwrap().instance();
    forge.publish(Event::ThreadRemoved {
        thread_id: "removed".into(),
    });
    admitted(&core, 1);
    let mut delivered = 0;
    core.drain_with(|core, event| {
        if let CoreEvent::Host {
            event: HostEvent::ThreadChanged { .. },
            ..
        } = event
        {
            delivered += 1;
            core.remove_host(&id).unwrap();
            core.connect_machine(&id.0);
        } else if matches!(event, CoreEvent::Host { .. }) {
            delivered += 1;
        }
    });
    assert_eq!(delivered, 1);
    wait_live(&mut core, &id);
    assert_ne!(core.host(&id).unwrap().instance(), original);
}
