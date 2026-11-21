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
    use test_support::editor::core::StubbedHost;
    use test_support::editor::forge::empty_state;
    let mut stubbed = StubbedHost::start(empty_state(), axum::Router::new());
    stubbed.apply_published();
    let thread = ThreadId("thread".into());
    let target = LiveTarget::Text {
        item_id: ItemId::new(),
    };
    let chunks: Vec<String> = (0..32).map(|index| format!("{index}:héllo ")).collect();
    let mut total = 0;
    for (index, chunk) in chunks.iter().enumerate() {
        total += chunk.len() as u64;
        stubbed.forge.publish(Event::Live {
            thread_id: thread.clone(),
            target: target.clone(),
            text: chunk.clone(),
            append: index > 0,
            coverage: LiveCoverage { start: 0, total },
        });
    }
    let complete = chunks.concat();
    let mut batch = LiveBatch::default();
    test_support::wait::wait_until(
        "every published append reaches the host",
        std::time::Duration::from_secs(10),
        || {
            while stubbed.core.drain_step(|_, event| {
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
            stubbed
                .host()
                .live_snapshot(&thread, &target)
                .is_some_and(|snapshot| snapshot.text.len_bytes() == complete.len())
        },
    );
    let events = batch.take();
    assert_eq!(events.len(), 1);
    let snapshot = stubbed.host().live_snapshot(&thread, &target).unwrap();
    assert_eq!(snapshot.text.to_string(), complete);
    assert!(snapshot.coverage.is_complete(snapshot.text.len_bytes()));
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one notify-then-sign-out scenario walked step by step"
)]
fn signing_out_clears_the_badge_and_withdraws_the_notifications_it_posted() {
    use artisan_api::{
        EngineConfig, EngineId, Event, ProjectId, Run, RunId, RunOutcome, RunState, Thread, Usage,
    };
    use editor_aqua::core::{Attention, EntryGate, Screen};
    use editor_aqua::shell::native_notifications::activation;
    use editor_aqua::shell::notifications::tag;
    use test_support::editor::forge::{StubForge, empty_state};
    use test_support::editor::shell::{Motion, ThreadShell};

    let thread_id = ThreadId("notified-thread".into());
    let thread = Thread {
        id: thread_id.clone(),
        project_id: ProjectId::scratch(),
        title: Some("Fix the build".into()),
        config: EngineConfig::new(EngineId::Claude),
        activity_seq: 2,
        activity_at: 0,
        read_seq: 1,
        marked_unread: false,
        settled_seq: 0,
        seq: 2,
        created_at: 0,
        updated_at: 0,
        archived: false,
        parent_thread_id: None,
        spawned_by: None,
        parked_until: None,
    };
    let run = Run {
        config: None,
        id: RunId("notified-run".into()),
        thread_id: thread_id.clone(),
        engine: EngineId::Claude,
        model: None,
        input_cutoff: 0,
        state: RunState::Running,
        usage: Usage::default(),
        session_id: None,
        seq: 2,
        started_at: 0,
        continuation: None,
    };
    let mut state = empty_state();
    state.threads.push(thread.clone());
    state.runs.push(run.clone());
    state.head = 2;
    let forge = editor_aqua::core::runtime::runtime()
        .block_on(StubForge::serve(state, axum::Router::new()));
    let ThreadShell {
        mut shell, host, ..
    } = ThreadShell::open_stubbed(&forge, "notified-host", "notified-thread", Motion::Reduced);
    let workspace = shell
        .entry
        .workspace_mut()
        .expect("a signed-in shell shows a workspace");
    workspace.handle_event(&shell.core, &CoreEvent::HostsChanged, false);

    forge.publish(Event::Thread {
        thread: Thread {
            activity_seq: 3,
            seq: 3,
            ..thread
        },
    });
    forge.publish(Event::Run {
        run: Run {
            state: RunState::Finished {
                outcome: RunOutcome::Done,
                error: None,
                ended_at: 0,
            },
            seq: 4,
            ..run
        },
    });
    let mut shown = Vec::new();
    test_support::wait::wait_until(
        "the finished thread to be notified",
        std::time::Duration::from_secs(10),
        || {
            let workspace = shell
                .entry
                .workspace_mut()
                .expect("a signed-in shell shows a workspace");
            shell.core.drain_with(|core, event| {
                shown.extend(
                    workspace
                        .handle_event(core, &event, false)
                        .show
                        .into_iter()
                        .map(|notification| notification.tag),
                );
            });
            !shown.is_empty()
        },
    );
    let posted = tag(&host.id, &thread_id);
    assert_eq!(shown, std::slice::from_ref(&posted));
    assert_eq!(
        activation(&shell, &posted),
        Some(Screen::Thread {
            host: host.id,
            thread: thread_id,
        })
    );

    shell.core.account_mut().sign_out();
    test_support::wait::wait_until(
        "the account to sign out",
        std::time::Duration::from_secs(10),
        || {
            shell.core.drain();
            !matches!(shell.core.entry_gate(), EntryGate::Ready { .. })
        },
    );
    let effects = shell
        .entry
        .sync(&shell.core)
        .expect("leaving the account reports what to clean up");
    assert_eq!(
        effects.withdraw,
        std::slice::from_ref(&posted),
        "the signed-out session's notification is taken down"
    );
    assert_eq!(effects.badge, Some(Attention::default()));
    assert!(effects.show.is_empty());
    assert_eq!(
        activation(&shell, &posted),
        None,
        "a notification from the signed-out session opens nothing"
    );
}

mod withdrawal {
    use artisan_api::{
        EngineConfig, EngineId, ProjectId, Run, RunId, RunOutcome, RunState, Thread, ThreadId,
        Usage,
    };
    use editor_aqua::core::{HostId, HostSwitch};
    use editor_aqua::shell::notifications::{NotificationHost, Notifications, tag};
    use std::collections::{HashMap, HashSet};

    struct FakeHost {
        id: HostId,
        threads: HashMap<ThreadId, Thread>,
        runs: HashMap<ThreadId, Run>,
        working: HashSet<ThreadId>,
        deleted: HashSet<ThreadId>,
        continue_after_restart: bool,
    }

    impl NotificationHost for FakeHost {
        fn id(&self) -> &HostId {
            &self.id
        }
        fn is_live(&self) -> bool {
            true
        }
        fn threads(&self) -> &HashMap<ThreadId, Thread> {
            &self.threads
        }
        fn run(&self, thread: &ThreadId) -> Option<&Run> {
            self.runs.get(thread)
        }
        fn is_working(&self, thread: &ThreadId) -> bool {
            self.working.contains(thread)
        }
        fn is_waiting(&self, _: &ThreadId) -> bool {
            false
        }
        fn is_deleted(&self, thread: &ThreadId) -> bool {
            self.deleted.contains(thread)
        }
        fn switch(&self, switch: HostSwitch) -> bool {
            switch != HostSwitch::ContinueAfterRestart || self.continue_after_restart
        }
    }

    fn thread(id: &ThreadId) -> Thread {
        Thread {
            id: id.clone(),
            project_id: ProjectId("project".into()),
            title: Some("Fix the build".into()),
            config: EngineConfig::new(EngineId::Claude),
            activity_seq: 2,
            activity_at: 0,
            read_seq: 1,
            marked_unread: false,
            settled_seq: 0,
            seq: 2,
            created_at: 0,
            updated_at: 0,
            archived: false,
            parent_thread_id: None,
            spawned_by: None,
            parked_until: None,
        }
    }

    fn finished(thread: &ThreadId) -> Run {
        ended(thread, RunOutcome::Done)
    }

    fn ended(thread: &ThreadId, outcome: RunOutcome) -> Run {
        Run {
            config: None,
            id: RunId("run".into()),
            thread_id: thread.clone(),
            engine: EngineId::Claude,
            model: None,
            input_cutoff: 0,
            state: RunState::Finished {
                outcome,
                error: None,
                ended_at: 0,
            },
            usage: Usage::default(),
            session_id: None,
            seq: 1,
            started_at: 0,
            continuation: None,
        }
    }

    #[test]
    fn deleting_a_notified_thread_withdraws_its_notification() {
        let id = ThreadId("thread".into());
        let mut host = FakeHost {
            id: HostId("host".into()),
            threads: HashMap::from([(id.clone(), thread(&id))]),
            runs: HashMap::new(),
            working: HashSet::from([id.clone()]),
            deleted: HashSet::new(),
            continue_after_restart: true,
        };
        let mut notifications = Notifications::default();
        notifications.reset(&host);
        host.working.clear();
        host.runs.insert(id.clone(), finished(&id));
        let posted = notifications.thread_changed(&host, &id, None, true);
        assert_eq!(
            posted.show.map(|shown| shown.body).as_deref(),
            Some("Finished.")
        );

        host.threads.remove(&id);
        host.runs.remove(&id);
        host.deleted.insert(id.clone());
        let changes = notifications.thread_changed(&host, &id, None, true);
        assert_eq!(changes.withdraw, vec![tag(&host.id, &id)]);
        assert!(notifications.host_reset(&host).is_empty());
    }

    fn shut_down(continue_after_restart: bool) -> Option<String> {
        let id = ThreadId("thread".into());
        let mut host = FakeHost {
            id: HostId("host".into()),
            threads: HashMap::from([(id.clone(), thread(&id))]),
            runs: HashMap::new(),
            working: HashSet::from([id.clone()]),
            deleted: HashSet::new(),
            continue_after_restart,
        };
        let mut notifications = Notifications::default();
        notifications.reset(&host);
        host.working.clear();
        host.runs
            .insert(id.clone(), ended(&id, RunOutcome::Interrupted));
        notifications
            .thread_changed(&host, &id, None, true)
            .show
            .map(|shown| shown.body)
    }

    #[test]
    fn a_turn_stopped_by_a_shutdown_is_announced_only_when_it_will_not_continue() {
        assert_eq!(shut_down(true), None);
        assert_eq!(
            shut_down(false).as_deref(),
            Some("Stopped because Forge shut down.")
        );
    }
}
