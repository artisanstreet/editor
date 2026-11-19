use super::*;

#[tokio::test(flavor = "multi_thread")]
async fn a_stop_is_published_at_once_while_the_adapter_cleans_up_and_drains_its_final_events() {
    for engine in [EngineId::Codex, EngineId::Claude, EngineId::OpenCode] {
        let entered = Arc::new(Notify::new());
        let ready = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let cleaned = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let script_entered = Arc::clone(&entered);
        let script_ready = Arc::clone(&ready);
        let script_release = Arc::clone(&release);
        let script_cleaned = Arc::clone(&cleaned);
        let adapter: Arc<dyn Harness> = Arc::new(Scripted(Arc::new(move |_, io| {
            let entered = Arc::clone(&script_entered);
            let ready = Arc::clone(&script_ready);
            let release = Arc::clone(&script_release);
            let cleaned = Arc::clone(&script_cleaned);
            Box::pin(async move {
                ready.notify_one();
                io.cancel.cancelled().await;
                entered.notify_one();
                release.notified().await;
                for index in 0..300 {
                    io.emit(reply(&format!("cleanup-{index}"), "stopped")).await;
                }
                cleaned.store(true, Ordering::SeqCst);
                Outcome::cancelled()
            })
        })));
        let fixture =
            fixture_with_engine_harness(engine, Arc::new(move |_| Arc::clone(&adapter))).await;
        fixture
            .hub
            .send_message(&fixture.thread_id, message_on(engine, "work"))
            .await
            .unwrap();
        tokio::time::timeout(test_support::wait::LIVENESS, ready.notified())
            .await
            .unwrap();
        fixture.hub.cancel(&fixture.thread_id).await.unwrap();
        tokio::time::timeout(test_support::wait::LIVENESS, entered.notified())
            .await
            .unwrap();
        assert!(fixture.hub.is_live(&fixture.thread_id));
        assert_eq!(
            fixture
                .hub
                .store()
                .latest_run(&fixture.thread_id)
                .await
                .unwrap()
                .unwrap()
                .status(),
            RunStatus::Cancelled
        );
        release.notify_one();
        let runs = fixture.settled().await;
        assert!(cleaned.load(Ordering::SeqCst));
        assert_eq!(runs.last().unwrap().status(), RunStatus::Cancelled);
        assert_eq!(fixture.items().await.iter().filter(|item| matches!(&item.kind, ItemKind::AssistantMessage { text, .. } if text.inline() == Some("stopped"))).count(), 300);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stop_the_engine_fails_to_clean_up_stays_a_continuable_stop() {
    let ready = Arc::new(Notify::new());
    let script_ready = Arc::clone(&ready);
    let fixture = fixture(Arc::new(move |_, io| {
        let ready = Arc::clone(&script_ready);
        Box::pin(async move {
            ready.notify_one();
            io.cancel.cancelled().await;
            Outcome::retryable(
                "provider refused to stop".to_owned(),
                forge_engines::Retry::Overloaded,
            )
        })
    }))
    .await;
    fixture
        .hub
        .send_message(&fixture.thread_id, message("work"))
        .await
        .unwrap();
    tokio::time::timeout(test_support::wait::LIVENESS, ready.notified())
        .await
        .unwrap();
    fixture.hub.cancel(&fixture.thread_id).await.unwrap();
    let runs = fixture.settled().await;
    assert_eq!(
        runs.last().unwrap().status(),
        RunStatus::Cancelled,
        "{runs:#?}"
    );
    assert_eq!(
        runs.last().unwrap().error(),
        Some("provider refused to stop")
    );
    assert!(
        !fixture
            .hub
            .store()
            .continue_pending(&fixture.thread_id)
            .await
            .unwrap(),
        "a stopped turn never resumes by itself"
    );
    fixture.hub.continue_turn(&fixture.thread_id).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stop_the_engine_never_confirms_stays_a_stop_that_says_so() {
    let ready = Arc::new(Notify::new());
    let script_ready = Arc::clone(&ready);
    let fixture = fixture(Arc::new(move |_, _io| {
        let ready = Arc::clone(&script_ready);
        Box::pin(async move {
            ready.notify_one();
            std::future::pending().await
        })
    }))
    .await;
    fixture
        .hub
        .send_message(&fixture.thread_id, message("work"))
        .await
        .unwrap();
    tokio::time::timeout(test_support::wait::LIVENESS, ready.notified())
        .await
        .unwrap();
    fixture.hub.cancel(&fixture.thread_id).await.unwrap();
    tokio::time::timeout(Duration::from_secs(120), async {
        while fixture.hub.is_live(&fixture.thread_id) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let run = fixture
        .hub
        .store()
        .latest_run(&fixture.thread_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(run.status(), RunStatus::Cancelled);
    assert!(run.error().unwrap().contains("did not confirm it stopped"));
}

#[tokio::test(flavor = "multi_thread")]
#[expect(
    clippy::too_many_lines,
    reason = "one cancellation told from request to cleanup"
)]
async fn cancellation_rejects_answers_and_goal_updates_while_cleanup_is_pending() {
    let release = Arc::new(Notify::new());
    let script_release = Arc::clone(&release);
    let adapter: Arc<dyn Harness> = Arc::new(Scripted(Arc::new(move |_, io| {
        let release = Arc::clone(&script_release);
        Box::pin(async move {
            io.emit(EngineEvent::Item {
                key: "approval".into(),
                parent: None,
                status: Status::AwaitingUser,
                kind: ItemKind::Tool {
                    tool: ToolKind::Shell {
                        command: "work".into(),
                        cwd: None,
                        background: false,
                        watch: false,
                    },
                    title: None,
                    output: None,
                    exit_code: None,
                    approval: Some(artisan_api::Approval {
                        request: ApprovalRequest::Command {
                            command: "work".into(),
                            cwd: None,
                        },
                        reason: None,
                        options: vec![Decision::AllowOnce, Decision::DenyOnce],
                        decision: None,
                    }),
                },
            })
            .await;
            io.emit(EngineEvent::Item {
                key: "goal".into(),
                parent: None,
                status: Status::Done,
                kind: ItemKind::Notice {
                    kind: artisan_api::NoticeKind::Goal {
                        state: "active".into(),
                    },
                    text: "finish work".into(),
                },
            })
            .await;
            io.cancel.cancelled().await;
            io.emit(reply("cleanup-started", "stopping")).await;
            release.notified().await;
            Outcome::cancelled()
        })
    })));
    let fixture =
        fixture_with_engine_harness(EngineId::Codex, Arc::new(move |_| Arc::clone(&adapter))).await;
    fixture
        .hub
        .send_message(&fixture.thread_id, message_on(EngineId::Codex, "work"))
        .await
        .unwrap();
    fixture
        .wait_for(|items| items.iter().any(|item| item.status == Status::AwaitingUser))
        .await;
    let approval = fixture
        .items()
        .await
        .into_iter()
        .find(|item| item.status == Status::AwaitingUser)
        .unwrap();
    fixture.hub.cancel(&fixture.thread_id).await.unwrap();
    fixture
        .wait_for(|items| texts(items).contains(&("stopping".into(), Status::Done)))
        .await;
    assert!(
        fixture.hub.is_live(&fixture.thread_id),
        "cleanup must still hold the run slot"
    );
    let answer = fixture
        .hub
        .answer(
            &approval.id,
            Answer::Approval {
                decision: Decision::AllowOnce,
            },
        )
        .await;
    assert!(
        answer.is_err(),
        "answers must be rejected once cancellation is draining"
    );
    let goal = fixture.hub.clear_goal(&fixture.thread_id).await;
    assert!(
        goal.is_err(),
        "goal updates must be rejected once cancellation is draining"
    );
    assert_eq!(
        fixture
            .hub
            .store()
            .inspector(&fixture.thread_id)
            .await
            .unwrap()
            .goal
            .unwrap()
            .objective
            .inline(),
        Some("finish work"),
        "a rejected clear must preserve the saved goal"
    );
    release.notify_one();
    assert_eq!(
        fixture.settled().await.last().unwrap().status(),
        RunStatus::Cancelled,
        "cleanup must still complete cancellation after rejecting inputs"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_input_the_engine_acknowledges_while_stopping_is_not_sent_again() {
    let asked = Asked::default();
    let started = Arc::new(Notify::new());
    let script: Script = {
        let asked = Arc::clone(&asked);
        let started = Arc::clone(&started);
        Arc::new(move |turn, io| {
            let asked = Arc::clone(&asked);
            let started = Arc::clone(&started);
            Box::pin(async move {
                let first = {
                    let mut asked = asked.lock().unwrap();
                    asked.push((turn.input.text.clone(), turn.session_id.clone()));
                    asked.len() == 1
                };
                io.emit(EngineEvent::Session {
                    id: "session".to_owned(),
                })
                .await;
                if first {
                    started.notify_one();
                    io.cancel.cancelled().await;
                    io.emit(EngineEvent::InputAccepted {
                        id: turn.input.id.clone(),
                    })
                    .await;
                    return Outcome::cancelled();
                }
                io.emit(EngineEvent::InputAccepted {
                    id: turn.input.id.clone(),
                })
                .await;
                io.emit(reply("done", "carried on")).await;
                Outcome::done()
            })
        })
    };
    let harness: Arc<dyn Harness> = Arc::new(Unscripted(script));
    let fixture = fixture_with_harness(Arc::new(move |_| Arc::clone(&harness))).await;
    fixture
        .hub
        .send_message(&fixture.thread_id, message("rename the module"))
        .await
        .unwrap();
    tokio::time::timeout(test_support::wait::LIVENESS, started.notified())
        .await
        .unwrap();
    fixture.hub.cancel(&fixture.thread_id).await.unwrap();
    assert_eq!(fixture.settled().await[0].status(), RunStatus::Cancelled);

    fixture.hub.continue_turn(&fixture.thread_id).await.unwrap();
    let continued = fixture.settled().await.remove(0);

    assert_eq!(
        continued.continuation,
        Some(artisan_api::Continuation::Requested)
    );
    let asked = asked.lock().unwrap().clone();
    assert_eq!(asked.len(), 2);
    assert!(
        !asked[1].0.contains("rename the module"),
        "the engine got the message twice: {asked:?}"
    );
    let items = fixture.items().await;
    assert!(items.iter().any(
        |item| matches!(item.kind, ItemKind::UserMessage { .. }) && item.status == Status::Done
    ));
}

struct WindingDown {
    asked: Asked,
    entered: Arc<Notify>,
    release: Arc<Notify>,
}

impl WindingDown {
    fn new() -> Self {
        Self {
            asked: Asked::default(),
            entered: Arc::new(Notify::new()),
            release: Arc::new(Notify::new()),
        }
    }

    fn script(&self) -> Script {
        let asked = Arc::clone(&self.asked);
        let entered = Arc::clone(&self.entered);
        let release = Arc::clone(&self.release);
        Arc::new(move |turn, io| {
            let asked = Arc::clone(&asked);
            let entered = Arc::clone(&entered);
            let release = Arc::clone(&release);
            Box::pin(async move {
                let first = {
                    let mut asked = asked.lock().unwrap();
                    asked.push((turn.input.text.clone(), turn.session_id.clone()));
                    asked.len() == 1
                };
                if first {
                    io.cancel.cancelled().await;
                    entered.notify_one();
                    release.notified().await;
                    return Outcome::cancelled();
                }
                io.emit(reply("done", "picked it back up")).await;
                Outcome::done()
            })
        })
    }

    fn turns(&self) -> usize {
        self.asked.lock().unwrap().len()
    }
}

async fn stopping_fixture(winding: &WindingDown) -> Fixture {
    let fixture = fixture(winding.script()).await;
    fixture
        .hub
        .send_message(&fixture.thread_id, message("refactor the parser"))
        .await
        .unwrap();
    fixture
        .wait_for(|items| {
            items.iter().any(|item| {
                matches!(item.kind, ItemKind::UserMessage { .. }) && item.status == Status::Done
            })
        })
        .await;
    fixture
}

#[tokio::test(flavor = "multi_thread")]
async fn continue_pressed_while_a_stop_winds_down_carries_the_turn_on_once_it_has() {
    let winding = WindingDown::new();
    let fixture = stopping_fixture(&winding).await;
    fixture.hub.cancel(&fixture.thread_id).await.unwrap();
    tokio::time::timeout(test_support::wait::LIVENESS, winding.entered.notified())
        .await
        .unwrap();
    test_support::wait::eventually(
        "the stop to be shown before the engine confirms it",
        test_support::wait::LIVENESS,
        || async {
            fixture
                .hub
                .store()
                .latest_run(&fixture.thread_id)
                .await
                .unwrap()
                .is_some_and(|run| run.status() == RunStatus::Cancelled)
        },
    )
    .await;
    assert!(fixture.hub.is_live(&fixture.thread_id));

    fixture
        .hub
        .continue_turn(&fixture.thread_id)
        .await
        .expect("Continue on a stopped turn is accepted while the engine winds down");
    fixture
        .hub
        .continue_turn(&fixture.thread_id)
        .await
        .expect("pressing Continue twice queues it once");
    winding.release.notify_one();

    let continued = fixture.settled().await.remove(0);
    assert_eq!(continued.status(), RunStatus::Done);
    assert_eq!(
        continued.continuation,
        Some(artisan_api::Continuation::Requested)
    );
    assert_eq!(winding.turns(), 2);
    assert!(
        texts(&fixture.items().await)
            .iter()
            .any(|(text, _)| text == "picked it back up")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn continue_is_accepted_while_a_stop_winds_down_before_the_stop_is_saved() {
    let winding = WindingDown::new();
    let fixture = stopping_fixture(&winding).await;
    let disk = durability::DiskFull::arming(&fixture, "runs");
    disk.strike();
    fixture.hub.cancel(&fixture.thread_id).await.unwrap();
    tokio::time::timeout(test_support::wait::LIVENESS, winding.entered.notified())
        .await
        .unwrap();
    fixture
        .hub
        .continue_turn(&fixture.thread_id)
        .await
        .expect("Continue after Stop is not refused as busy while the stop is still unsaved");
    disk.clear();
    winding.release.notify_one();
    fixture.settled().await;
    let history = fixture.runs().await;
    let runs: Vec<_> = history
        .iter()
        .map(|run| (run.status(), run.continuation))
        .collect();
    assert_eq!(
        runs,
        [
            (RunStatus::Cancelled, None),
            (RunStatus::Done, Some(artisan_api::Continuation::Requested)),
        ],
        "the stop is saved once the disk recovers and the continue carries the turn on: {history:#?} {:#?}",
        fixture.items().await
    );
    assert_eq!(winding.turns(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn stop_then_continue_then_stop_leaves_the_turn_stopped() {
    let winding = WindingDown::new();
    let fixture = stopping_fixture(&winding).await;
    fixture.hub.cancel(&fixture.thread_id).await.unwrap();
    tokio::time::timeout(test_support::wait::LIVENESS, winding.entered.notified())
        .await
        .unwrap();
    fixture.hub.continue_turn(&fixture.thread_id).await.unwrap();
    fixture.hub.cancel(&fixture.thread_id).await.unwrap();
    winding.release.notify_one();

    let runs = fixture.settled().await;
    assert_eq!(winding.turns(), 1, "the last Stop wins over the Continue");
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].status(), RunStatus::Cancelled);
    assert!(
        !fixture
            .hub
            .store()
            .continue_pending(&fixture.thread_id)
            .await
            .unwrap()
    );
}

async fn stays_stopped_after_restart(fixture: &Fixture) {
    let run = fixture
        .hub
        .store()
        .latest_run(&fixture.thread_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(run.status(), RunStatus::Cancelled);
    assert!(
        !fixture
            .hub
            .store()
            .continue_pending(&fixture.thread_id)
            .await
            .unwrap(),
        "a turn the user stopped never resumes when Forge starts again"
    );
    assert!(
        !fixture
            .hub
            .store()
            .pending_threads(8, Vec::new())
            .await
            .unwrap()
            .contains(&fixture.thread_id)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stop_forge_shuts_down_during_is_not_resumed_on_restart() {
    let winding = WindingDown::new();
    let fixture = stopping_fixture(&winding).await;
    fixture.hub.cancel(&fixture.thread_id).await.unwrap();
    tokio::time::timeout(test_support::wait::LIVENESS, winding.entered.notified())
        .await
        .unwrap();
    fixture.hub.shutdown();
    winding.release.notify_one();
    fixture.hub.stop(test_support::wait::LIVENESS).await;

    stays_stopped_after_restart(&fixture).await;
    assert_eq!(winding.turns(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stop_pressed_while_forge_shuts_down_is_not_resumed_on_restart() {
    let winding = WindingDown::new();
    let fixture = stopping_fixture(&winding).await;
    fixture.hub.shutdown();
    tokio::time::timeout(test_support::wait::LIVENESS, winding.entered.notified())
        .await
        .unwrap();
    fixture.hub.cancel(&fixture.thread_id).await.unwrap();
    winding.release.notify_one();
    fixture.hub.stop(test_support::wait::LIVENESS).await;

    stays_stopped_after_restart(&fixture).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_continue_acknowledged_during_a_stop_survives_a_storage_failure() {
    let winding = WindingDown::new();
    let fixture = stopping_fixture(&winding).await;
    fixture.hub.cancel(&fixture.thread_id).await.unwrap();
    tokio::time::timeout(test_support::wait::LIVENESS, winding.entered.notified())
        .await
        .unwrap();
    fixture.hub.continue_turn(&fixture.thread_id).await.unwrap();
    let disk = durability::DiskFull::arming(&fixture, "continues");
    disk.strike();
    winding.release.notify_one();
    test_support::wait::within(
        "the stopped run to end",
        test_support::wait::LIVENESS,
        async {
            while fixture.hub.is_live(&fixture.thread_id) {
                tokio::task::yield_now().await;
            }
        },
    )
    .await;
    assert_eq!(winding.turns(), 1);

    disk.clear();
    test_support::wait::eventually(
        "the acknowledged continue to run once storage recovers",
        test_support::wait::LIVENESS,
        || async { winding.turns() == 2 },
    )
    .await;
    let continued = fixture.settled().await;
    assert_eq!(
        continued
            .iter()
            .find(|run| run.continuation.is_some())
            .map(|run| (run.status(), run.continuation)),
        Some((RunStatus::Done, Some(artisan_api::Continuation::Requested)))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stop_that_lands_as_the_engine_returns_is_still_a_stop() {
    let stopper: Arc<std::sync::OnceLock<(Hub, ThreadId)>> = Arc::default();
    let script_stopper = Arc::clone(&stopper);
    let fixture = fixture(Arc::new(move |_, io| {
        let stopper = Arc::clone(&script_stopper);
        Box::pin(async move {
            let (hub, thread) = stopper
                .get()
                .expect("the test names the run to stop")
                .clone();
            let runtime = tokio::runtime::Handle::current();
            drop(std::thread::spawn(move || {
                runtime
                    .block_on(hub.cancel(&thread))
                    .expect("the stop is accepted");
            }));
            test_support::wait::wait_until(
                "the stop to reach the engine while it is still answering",
                test_support::wait::LIVENESS,
                || io.cancel.is_cancelled(),
            );
            Outcome::retryable(
                "provider refused to stop".to_owned(),
                forge_engines::Retry::Overloaded,
            )
        })
    }))
    .await;
    stopper
        .set((fixture.hub.clone(), fixture.thread_id.clone()))
        .expect("the stopper is set once");
    fixture
        .hub
        .send_message(&fixture.thread_id, message("work"))
        .await
        .unwrap();

    let runs = fixture.settled().await;
    assert_eq!(runs[0].status(), RunStatus::Cancelled, "{runs:#?}");
    assert_eq!(runs[0].error(), Some("provider refused to stop"));
    assert_eq!(
        fixture.thread().await.parked_until,
        None,
        "a stop is not retried"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_continue_pressed_while_a_failed_save_winds_down_still_carries_the_turn_on() {
    let go = Arc::new(Notify::new());
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let turns = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let fixture = fixture(Arc::new({
        let (go, entered, release, turns) = (
            Arc::clone(&go),
            Arc::clone(&entered),
            Arc::clone(&release),
            Arc::clone(&turns),
        );
        move |_, io| {
            let (go, entered, release, turns) = (
                Arc::clone(&go),
                Arc::clone(&entered),
                Arc::clone(&release),
                Arc::clone(&turns),
            );
            Box::pin(async move {
                if turns.fetch_add(1, Ordering::SeqCst) > 0 {
                    io.emit(reply("done", "picked it back up")).await;
                    return Outcome::done();
                }
                go.notified().await;
                io.emit(EngineEvent::Session {
                    id: "resumed".to_owned(),
                })
                .await;
                io.cancel.cancelled().await;
                entered.notify_one();
                release.notified().await;
                Outcome::cancelled()
            })
        }
    }))
    .await;
    fixture
        .hub
        .send_message(&fixture.thread_id, message("refactor the parser"))
        .await
        .unwrap();
    fixture
        .wait_for(|items| {
            items.iter().any(|item| {
                matches!(item.kind, ItemKind::UserMessage { .. }) && item.status == Status::Done
            })
        })
        .await;
    let disk = durability::DiskFull::arming(&fixture, "runs");
    disk.strike();
    go.notify_one();
    tokio::time::timeout(test_support::wait::LIVENESS, entered.notified())
        .await
        .unwrap();

    fixture.hub.cancel(&fixture.thread_id).await.unwrap();
    fixture
        .hub
        .continue_turn(&fixture.thread_id)
        .await
        .expect("Continue is accepted while the stop winds down");
    disk.clear();
    release.notify_one();

    fixture.settled().await;
    let history = fixture.runs().await;
    let runs: Vec<_> = history
        .iter()
        .map(|run| (run.status(), run.continuation))
        .collect();
    assert_eq!(
        runs,
        [
            (RunStatus::Cancelled, None),
            (RunStatus::Done, Some(artisan_api::Continuation::Requested)),
        ],
        "{history:#?}"
    );
    assert!(
        history[0]
            .error()
            .is_some_and(|error| error.contains("database or disk is full")),
        "the stop keeps why the run could not be saved: {history:#?}"
    );
}

async fn ended(fixture: &Fixture) {
    test_support::wait::within("the run to end", test_support::wait::LIVENESS, async {
        while fixture.hub.is_live(&fixture.thread_id) {
            tokio::task::yield_now().await;
        }
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_second_stop_cancels_a_continue_still_waiting_on_an_unsaved_stop() {
    let winding = WindingDown::new();
    let fixture = stopping_fixture(&winding).await;
    let disk = durability::DiskFull::arming(&fixture, "runs");
    disk.strike();
    fixture.hub.cancel(&fixture.thread_id).await.unwrap();
    tokio::time::timeout(test_support::wait::LIVENESS, winding.entered.notified())
        .await
        .unwrap();
    fixture.hub.continue_turn(&fixture.thread_id).await.unwrap();
    winding.release.notify_one();
    ended(&fixture).await;

    fixture.hub.cancel(&fixture.thread_id).await.unwrap();
    disk.clear();
    fixture.hub.settle_unsettled().await;
    fixture.hub.continue_owed().await;

    let runs = fixture.settled().await;
    assert_eq!(winding.turns(), 1, "the second Stop wins over the Continue");
    assert_eq!(
        runs.iter()
            .map(artisan_api::Run::status)
            .collect::<Vec<_>>(),
        [RunStatus::Cancelled]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_continue_pressed_while_the_stop_is_unsaved_brings_back_the_unread_message() {
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let asked = Asked::default();
    let script: Script = Arc::new({
        let (entered, release, asked) = (
            Arc::clone(&entered),
            Arc::clone(&release),
            Arc::clone(&asked),
        );
        move |turn, io| {
            let (entered, release, asked) = (
                Arc::clone(&entered),
                Arc::clone(&release),
                Arc::clone(&asked),
            );
            Box::pin(async move {
                let first = {
                    let mut asked = asked.lock().unwrap();
                    asked.push((turn.input.text.clone(), turn.session_id.clone()));
                    asked.len() == 1
                };
                if first {
                    io.cancel.cancelled().await;
                    entered.notify_one();
                    release.notified().await;
                    return Outcome::cancelled();
                }
                io.emit(EngineEvent::InputAccepted {
                    id: turn.input.id.clone(),
                })
                .await;
                io.emit(reply("done", "the wheel goes left")).await;
                Outcome::done()
            })
        }
    });
    let harness: Arc<dyn Harness> = Arc::new(Unscripted(script));
    let fixture = fixture_with_harness(Arc::new(move |_| Arc::clone(&harness))).await;
    fixture
        .hub
        .send_message(&fixture.thread_id, message("where should the wheel go?"))
        .await
        .unwrap();
    test_support::wait::eventually(
        "the engine to start the turn",
        test_support::wait::LIVENESS,
        || async { !asked.lock().unwrap().is_empty() },
    )
    .await;
    let disk = durability::DiskFull::arming(&fixture, "runs");
    disk.strike();
    fixture.hub.cancel(&fixture.thread_id).await.unwrap();
    tokio::time::timeout(test_support::wait::LIVENESS, entered.notified())
        .await
        .unwrap();
    release.notify_one();
    ended(&fixture).await;

    fixture
        .hub
        .continue_turn(&fixture.thread_id)
        .await
        .expect("Continue on the stopped turn is accepted while its end is unsaved");
    disk.clear();
    fixture.hub.resume_pending();

    let runs = fixture.settled().await;
    assert_eq!(runs[0].status(), RunStatus::Done, "{runs:#?}");
    let asked = asked.lock().unwrap().clone();
    assert_eq!(asked.len(), 2, "{asked:?}");
    assert!(
        asked[1].0.contains("where should the wheel go?"),
        "the unread message reaches the engine: {asked:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stop_pressed_while_forge_shuts_down_is_saved_before_the_engine_confirms_it() {
    let winding = WindingDown::new();
    let fixture = stopping_fixture(&winding).await;
    fixture.hub.shutdown();
    tokio::time::timeout(test_support::wait::LIVENESS, winding.entered.notified())
        .await
        .unwrap();

    fixture.hub.cancel(&fixture.thread_id).await.unwrap();
    let saved = fixture
        .hub
        .store()
        .latest_run(&fixture.thread_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        saved.status(),
        RunStatus::Cancelled,
        "the stop is saved when Stop returns, though the engine has not confirmed it"
    );
    fixture
        .hub
        .store()
        .recover_thread(fixture.thread_id.clone())
        .await
        .unwrap();
    stays_stopped_after_restart(&fixture).await;

    winding.release.notify_one();
    fixture.hub.stop(test_support::wait::LIVENESS).await;
    assert_eq!(winding.turns(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_continue_pressed_as_a_finished_turn_ends_adds_no_error() {
    let fixture = fixture(Arc::new(|_, io| {
        Box::pin(async move {
            io.emit(reply("done", "all done")).await;
            Outcome::done()
        })
    }))
    .await;
    fixture
        .hub
        .send_message(&fixture.thread_id, message("finish it"))
        .await
        .unwrap();
    assert_eq!(fixture.settled().await[0].status(), RunStatus::Done);
    let (inbox, _receiver) = mpsc::channel(1);
    let through = fixture.hub.store().head();
    fixture
        .hub
        .lock()
        .claim(fixture.thread_id.clone(), |retries| LiveRun {
            id: RunId::new(),
            inbox,
            cancel: CancellationToken::new(),
            engine: None,
            launched: None,
            live: std::collections::BTreeMap::new(),
            foreground_bytes: 0,
            terminal_bytes: 0,
            phase: turns::Phase::Stopping {
                through,
                then: turns::AfterStop::Continue,
            },
            stop: turns::StopSignal::default(),
            retries,
        });

    fixture.hub.end_live(&fixture.thread_id, false).await;

    assert!(
        fixture
            .items()
            .await
            .iter()
            .all(|item| !matches!(item.kind, ItemKind::Error { .. })),
        "a Continue that finds the turn finished settles quietly"
    );
    assert!(
        !fixture
            .hub
            .store()
            .continue_pending(&fixture.thread_id)
            .await
            .unwrap()
    );
    assert!(fixture.hub.active_thread_ids().is_empty());
}
