use artisan_api::{ItemId, LiveTarget, ThreadId};
use editor_aqua::core::{CoreEvent, HostEvent, HostId};
use editor_aqua::shell::live_batch::LiveBatch;

fn live(host: &str, thread: &str, item: &ItemId) -> CoreEvent {
    CoreEvent::Host {
        host: HostId(host.into()),
        event: HostEvent::Live {
            thread_id: ThreadId(thread.into()),
            target: LiveTarget::Text {
                item_id: item.clone(),
            },
        },
    }
}

#[test]
fn a_burst_notifies_each_live_target_once_without_mixing_hosts_or_threads() {
    let item = ItemId::new();
    let other = ItemId::new();
    let mut batch = LiveBatch::default();
    for _ in 0..256 {
        assert!(batch.push(live("host", "thread", &item)).is_none());
    }
    assert!(batch.push(live("other-host", "thread", &item)).is_none());
    assert!(batch.push(live("host", "other-thread", &item)).is_none());
    assert!(batch.push(live("host", "thread", &other)).is_none());
    let events = batch.take();
    assert_eq!(events.len(), 4);
    assert!(
        matches!(&events[0], CoreEvent::Host { host, event: HostEvent::Live { thread_id, target: LiveTarget::Text { item_id } } }
        if host.0 == "host" && thread_id.0 == "thread" && item_id == &item)
    );
    assert!(batch.take().is_empty());
}

#[test]
fn lifecycle_barriers_flush_before_terminal_or_navigation_events() {
    let item = ItemId::new();
    for barrier in [
        CoreEvent::ScreenChanged,
        CoreEvent::HostsChanged,
        CoreEvent::Host {
            host: HostId("host".into()),
            event: HostEvent::Reset,
        },
    ] {
        let mut batch = LiveBatch::default();
        assert!(batch.push(live("host", "thread", &item)).is_none());
        let barrier = batch.push(barrier).unwrap();
        let mut delivered = batch.take();
        delivered.push(barrier);
        assert_eq!(delivered.len(), 2);
        assert!(matches!(
            &delivered[0],
            CoreEvent::Host {
                event: HostEvent::Live { .. },
                ..
            }
        ));
        assert!(!matches!(
            &delivered[1],
            CoreEvent::Host {
                event: HostEvent::Live { .. },
                ..
            }
        ));
        assert!(batch.push(live("host", "thread", &item)).is_none());
        assert_eq!(batch.take().len(), 1);
    }
}

#[test]
fn coalescing_notifications_never_drops_the_underlying_text_appends() {
    use artisan_api::{Event, LiveCoverage};
    use editor_aqua::core::{
        Core, EditorFiles, ForgeClient, HostConfig, HostKind, runtime::runtime,
    };
    let directory = tempfile::tempdir().unwrap();
    let mut core = Core::for_tests(EditorFiles {
        dir: directory.path().into(),
    })
    .unwrap();
    let config = HostConfig {
        id: HostId("burst".into()),
        name: "Burst".into(),
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
    drop(core.drain());
    let sender = core.host(&config.id).unwrap().test_transition_sender();
    let thread = ThreadId("thread".into());
    let target = LiveTarget::Text {
        item_id: ItemId::new(),
    };
    let chunks: Vec<String> = (0..32).map(|index| format!("{index}:héllo ")).collect();
    runtime().block_on(async {
        let mut total = 0;
        for (index, chunk) in chunks.iter().enumerate() {
            total += chunk.len() as u64;
            sender
                .event(Event::Live {
                    thread_id: thread.clone(),
                    target: target.clone(),
                    text: chunk.clone(),
                    append: index > 0,
                    coverage: LiveCoverage { start: 0, total },
                })
                .await;
        }
    });
    let mut batch = LiveBatch::default();
    while core.drain_step(|_, event| {
        if let Some(event) = batch.push(event) {
            assert!(!matches!(
                event,
                CoreEvent::Host {
                    event: HostEvent::Live { .. },
                    ..
                }
            ));
        }
    }) {}
    let events = batch.take();
    assert_eq!(events.len(), 1);
    let snapshot = core
        .host(&config.id)
        .unwrap()
        .live_snapshot(&thread, &target)
        .unwrap();
    assert_eq!(snapshot.text.to_string(), chunks.concat());
    assert!(snapshot.coverage.is_complete(snapshot.text.len_bytes()));
}
