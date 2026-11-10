use editor_sources::SourceProviderExt;
use aqua::images::ImageAssets;
use aqua_media::decode::DecodedImage;
use artisan_api::{BlobHash, Event, GlobalPrompt};
use editor_aqua::core::{
    BlobImage, Core, CoreEvent, EditorFiles, ForgeClient, HostConfig, HostEvent, HostId, HostKind,
    runtime::runtime,
};
use std::sync::Arc;

fn host(core: &mut Core, id: &str) -> HostId {
    let config = HostConfig {
        id: HostId(id.into()),
        name: id.into(),
        address: "127.0.0.1:1".into(),
        fingerprint: "0".repeat(64),
        kind: HostKind::Remote,
    };
    let client = ForgeClient::new(
        &config.address,
        &config.fingerprint,
        core.identity.as_ref().unwrap(),
    )
    .unwrap();
    core.add_test_host(&config, client);
    core.host_mut(&config.id).unwrap().test_stop_connection();
    config.id
}

fn core() -> (tempfile::TempDir, Core) {
    let directory = tempfile::tempdir().unwrap();
    let core = Core::for_tests(EditorFiles {
        dir: directory.path().to_owned(),
    })
    .unwrap();
    (directory, core)
}

fn flush_initial(core: &mut Core) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while core.test_startup_pending() {
        assert!(
            std::time::Instant::now() < deadline,
            "Core startup did not settle"
        );
        core.drain_with(|_, _| {});
        std::thread::yield_now();
    }
}

fn setting(key: &str, value: serde_json::Value) -> Event {
    Event::Setting {
        key: key.into(),
        value,
    }
}

#[test]
fn admitted_stream_and_projection_results_apply_in_order_before_invalidation() {
    let (_directory, mut core) = core();
    let id = host(&mut core, "first");
    flush_initial(&mut core);
    let (prompt_generation, catalog_generation) =
        core.host_mut(&id).unwrap().test_begin_projections();
    let sender = core.host(&id).unwrap().test_transition_sender();
    let prompt = GlobalPrompt {
        version: 1,
        text: "before".into(),
        candidates: Vec::new(),
        engines: Vec::new(),
    };
    runtime().block_on(async {
        sender.prompt(prompt_generation, prompt).await;
        sender.event(setting("prompt_revision", 2.into())).await;
        sender.catalog(catalog_generation, Vec::new()).await;
        sender.event(setting("skills_revision", 2.into())).await;
    });
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
    let id = host(&mut core, "first");
    flush_initial(&mut core);
    let (prompt_generation, catalog_generation) =
        core.host_mut(&id).unwrap().test_begin_projections();
    let sender = core.host(&id).unwrap().test_transition_sender();
    runtime().block_on(async {
        sender.event(setting("prompt_revision", 2.into())).await;
        sender
            .prompt(
                prompt_generation,
                GlobalPrompt {
                    version: 1,
                    text: "stale".into(),
                    candidates: Vec::new(),
                    engines: Vec::new(),
                },
            )
            .await;
        sender.event(setting("skills_revision", 2.into())).await;
        sender.catalog(catalog_generation, Vec::new()).await;
    });
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
    let first = host(&mut core, "first");
    let second = host(&mut core, "second");
    flush_initial(&mut core);
    let first_sender = core.host(&first).unwrap().test_transition_sender();
    let second_sender = core.host(&second).unwrap().test_transition_sender();
    let account_sender = core.account_mut().test_sync_sender();
    let a = BlobHash("a".into());
    let b = BlobHash("b".into());
    let a_generation = core.host_mut(&first).unwrap().test_begin_image(a.clone());
    let b_generation = core.host_mut(&first).unwrap().test_begin_image(b.clone());
    let assets = ImageAssets::default();
    let image = Arc::new(
        assets
            .insert(DecodedImage::from_rgba(1, 1, vec![0, 0, 0, 255]).unwrap())
            .unwrap(),
    );
    runtime().block_on(async {
        second_sender.event(setting("order", 1.into())).await;
        account_sender.transport_failure().await;
        first_sender
            .image(a.clone(), a_generation, image.clone())
            .await;
        first_sender.event(setting("order", 2.into())).await;
        first_sender.image(b.clone(), b_generation, image).await;
        second_sender.event(setting("order", 3.into())).await;
    });
    let mut snapshots = Vec::new();
    let mut order = Vec::new();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while order.len() < 6 {
        assert!(
            std::time::Instant::now() < deadline,
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
                    matches!(image_host.image(&a), Some(BlobImage::Ready(_))),
                    matches!(image_host.image(&b), Some(BlobImage::Ready(_))),
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

#[test]
fn image_facade_observes_current_results_without_retaining_host_or_evicted_images() {
    let (_directory, mut core) = core();
    let id = host(&mut core, "images");
    flush_initial(&mut core);
    let facade = core.host(&id).unwrap().blob_images();
    let first = BlobHash("first".into());
    assert!(facade.lookup(&first).is_none());
    let generation = core.host_mut(&id).unwrap().test_begin_image(first.clone());
    assert!(matches!(facade.lookup(&first), Some(BlobImage::Loading)));
    let assets = ImageAssets::default();
    let first_image = Arc::new(
        assets
            .insert(DecodedImage::from_rgba(1, 1, vec![10, 20, 30, 255]).unwrap())
            .unwrap(),
    );
    let sender = core.host(&id).unwrap().test_transition_sender();
    runtime().block_on(sender.image(first.clone(), generation, first_image.clone()));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while matches!(facade.lookup(&first), Some(BlobImage::Loading)) {
        assert!(
            std::time::Instant::now() < deadline,
            "first image did not arrive"
        );
        core.drain_with(|_, _| {});
    }
    assert!(matches!(facade.lookup(&first), Some(BlobImage::Ready(_))));
    let other = Arc::new(
        assets
            .insert(DecodedImage::from_rgba(1, 1, vec![30, 20, 10, 255]).unwrap())
            .unwrap(),
    );
    for index in 0..32 {
        let hash = BlobHash(format!("other-{index}"));
        let generation = core.host_mut(&id).unwrap().test_begin_image(hash.clone());
        runtime().block_on(sender.image(hash.clone(), generation, other.clone()));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while matches!(facade.lookup(&hash), Some(BlobImage::Loading)) {
            assert!(
                std::time::Instant::now() < deadline,
                "image completion did not arrive"
            );
            core.drain_with(|_, _| {});
        }
        assert!(matches!(facade.lookup(&hash), Some(BlobImage::Ready(_))));
    }
    assert!(matches!(facade.lookup(&first), Some(BlobImage::Ready(_))));
    drop(first_image);
    assert!(facade.lookup(&first).is_none());
    let last = BlobHash("other-31".into());
    assert!(matches!(facade.lookup(&last), Some(BlobImage::Ready(_))));
    core.remove_host(&id).unwrap();
    assert!(facade.lookup(&last).is_none());
    host(&mut core, "images");
    assert!(facade.lookup(&last).is_none());
    let replacement = core.host(&id).unwrap().blob_images();
    assert!(replacement.lookup(&last).is_none());
}

#[test]
fn source_provider_tracks_gate_and_host_instance_without_reviving() {
    let (_directory, mut core) = core();
    let id = host(&mut core, "sources");
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
    let original = core.account().profile().unwrap().clone();
    let mut no_machine = original.clone();
    no_machine.machines.clear();
    let sender = core.account_mut().test_sync_sender();
    runtime().block_on(sender.profile(no_machine));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while matches!(
        core.entry_gate(),
        editor_aqua::core::EntryGate::Ready { .. }
    ) {
        assert!(std::time::Instant::now() < deadline, "gate did not close");
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
    runtime().block_on(sender.profile(original));
    while !matches!(
        core.entry_gate(),
        editor_aqua::core::EntryGate::Ready { .. }
    ) {
        assert!(std::time::Instant::now() < deadline, "gate did not reopen");
        core.drain_with(|_, _| {});
    }
    assert_eq!(core.host(&id).unwrap().instance(), instance);
    assert!(!provider.is_available());
    let refreshed = core.source_provider(&id, instance).unwrap();
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
    host(&mut core, "sources");
    assert!(core.source_provider(&id, instance).is_none());
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
        let (started, running) = tokio::sync::oneshot::channel();
        let blocked_sender = sender.clone();
        let blocked = tokio::spawn(async move {
            let _ = started.send(());
            blocked_sender.send(2).await
        });
        running.await.unwrap();
        assert!(!blocked.is_finished());
        blocked.abort();
        assert!(blocked.await.unwrap_err().is_cancelled());
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
        let (started, running) = tokio::sync::oneshot::channel();
        let waiting_sender = sender.clone();
        let waiting = tokio::spawn(async move {
            let _ = started.send(());
            waiting_sender.send(5).await
        });
        running.await.unwrap();
        assert!(!waiting.is_finished());
        drop(delivery);
        let error = tokio::time::timeout(Duration::from_secs(1), waiting)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert_eq!(error.0, 5);
    });
}

#[test]
fn account_retry_waits_for_ui_delivery_and_stale_acknowledgement_closes() {
    use tokio::sync::oneshot::error::TryRecvError;
    let (_directory, mut core) = core();
    flush_initial(&mut core);
    let sender = core.account_mut().test_sync_sender();
    let mut acknowledgement = runtime().block_on(sender.transport_failure_ack());
    assert!(matches!(
        acknowledgement.try_recv(),
        Err(TryRecvError::Empty)
    ));
    let mut observed = false;
    core.drain_with(|_, event| {
        if matches!(event, CoreEvent::Account(_)) {
            observed = true;
            assert!(matches!(
                acknowledgement.try_recv(),
                Err(TryRecvError::Empty)
            ));
        }
    });
    assert!(observed);
    assert_eq!(acknowledgement.try_recv(), Ok(()));
    let stale = core.account_mut().test_sync_sender();
    let mut stale_acknowledgement = runtime().block_on(stale.transport_failure_ack());
    let _replacement = core.account_mut().test_sync_sender();
    core.drain_with(|_, event| assert!(!matches!(event, CoreEvent::Account(_))));
    assert!(matches!(
        stale_acknowledgement.try_recv(),
        Err(TryRecvError::Closed)
    ));
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
            seen.send(path).unwrap();
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
    flush_initial(&mut core);
    core.account_mut()
        .request_link("test@example.invalid", "Test".into());
    core.drain_with(|_, _| {});
    assert_eq!(
        requests.recv_timeout(Duration::from_secs(5)).unwrap(),
        "/v1/auth/magic/request"
    );
    assert!(requests.recv_timeout(Duration::from_millis(3200)).is_err());
    let mut pending_seen = false;
    let deadline = Instant::now() + Duration::from_secs(5);
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
    assert!(requests.recv_timeout(Duration::from_millis(2700)).is_err());
    assert_eq!(
        requests.recv_timeout(Duration::from_secs(3)).unwrap(),
        "/v1/auth/magic/status"
    );
}

#[test]
fn observer_mutations_dispatch_before_next_transition_and_nested_drain_is_deferred() {
    let (_directory, mut core) = core();
    let id = host(&mut core, "first");
    flush_initial(&mut core);
    let sender = core.host(&id).unwrap().test_transition_sender();
    runtime().block_on(async {
        sender.event(setting("first", 1.into())).await;
        sender.event(setting("second", 2.into())).await;
    });
    let mut events = Vec::new();
    let mut nested = false;
    core.drain_with(|core, event| match event {
        CoreEvent::Host {
            event: HostEvent::Changed,
            ..
        } => {
            events.push("host-first");
            core.set_setting("observer-test", true.into());
            core.drain_with(|_, _| nested = true);
        }
        CoreEvent::SettingsChanged => events.push("settings"),
        _ => {}
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
    let id = host(&mut core, "first");
    flush_initial(&mut core);
    let sender = core.host(&id).unwrap().test_transition_sender();
    runtime().block_on(sender.event(Event::ThreadRemoved {
        thread_id: "removed".into(),
    }));
    let mut events = Vec::new();
    core.drain_with(|core, event| match event {
        CoreEvent::Host {
            event: HostEvent::ThreadChanged { .. },
            ..
        } => {
            events.push("thread");
            core.set_setting("observer-test", true.into());
        }
        CoreEvent::Host {
            event: HostEvent::Changed,
            ..
        } => events.push("host"),
        CoreEvent::SettingsChanged => events.push("settings"),
        _ => {}
    });
    assert_eq!(events, ["thread", "host", "settings"]);
}

#[test]
fn observer_cross_domain_effects_follow_existing_siblings_and_see_final_state() {
    let (_directory, mut core) = core();
    let id = host(&mut core, "first");
    flush_initial(&mut core);
    let sender = core.host(&id).unwrap().test_transition_sender();
    runtime().block_on(sender.event(Event::ThreadRemoved {
        thread_id: "removed".into(),
    }));
    let mut events = Vec::new();
    core.drain_with(|core, event| match event {
        CoreEvent::Host {
            event: HostEvent::ThreadChanged { .. },
            ..
        } => {
            events.push("thread");
            core.host_mut(&id).unwrap().load_prompt(false);
            core.set_setting("observer-test", true.into());
        }
        CoreEvent::Host {
            event: HostEvent::Changed,
            ..
        } => {
            assert!(core.host(&id).unwrap().prompt_load().loading);
            events.push("host");
        }
        CoreEvent::SettingsChanged => events.push("settings"),
        _ => {}
    });
    assert_eq!(events, ["thread", "host", "host", "settings"]);
}

#[test]
fn host_list_changes_only_when_a_host_crosses_the_live_boundary() {
    use editor_aqua::core::ConnectionStatus;

    let (_directory, mut core) = core();
    let id = host(&mut core, "first");
    flush_initial(&mut core);
    let sender = core.host(&id).unwrap().test_transition_sender();
    runtime().block_on(sender.event(setting("initial", true.into())));
    core.drain_with(|_, _| {});
    runtime().block_on(async {
        sender
            .status(ConnectionStatus::Offline {
                attempt: 1,
                reason: "first".into(),
                code: None,
            })
            .await;
        sender
            .status(ConnectionStatus::Offline {
                attempt: 2,
                reason: "second".into(),
                code: None,
            })
            .await;
        sender.status(ConnectionStatus::Live).await;
    });
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
    let id = host(&mut core, "first");
    flush_initial(&mut core);
    let original = core.host(&id).unwrap().instance();
    let sender = core.host(&id).unwrap().test_transition_sender();
    runtime().block_on(sender.event(Event::ThreadRemoved {
        thread_id: "removed".into(),
    }));
    let mut delivered = 0;
    core.drain_with(|core, event| {
        if let CoreEvent::Host {
            event: HostEvent::ThreadChanged { .. },
            ..
        } = event
        {
            delivered += 1;
            core.remove_host(&id).unwrap();
            host(core, "first");
        } else if matches!(event, CoreEvent::Host { .. }) {
            delivered += 1;
        }
    });
    assert_eq!(delivered, 1);
    assert_ne!(core.host(&id).unwrap().instance(), original);
}
