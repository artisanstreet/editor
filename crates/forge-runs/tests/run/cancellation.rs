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
        tokio::time::timeout(Duration::from_secs(5), ready.notified())
            .await
            .unwrap();
        fixture.hub.cancel(&fixture.thread_id).await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), entered.notified())
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
    tokio::time::timeout(Duration::from_secs(5), ready.notified())
        .await
        .unwrap();
    fixture.hub.cancel(&fixture.thread_id).await.unwrap();
    let runs = fixture.settled().await;
    assert_eq!(runs.last().unwrap().status(), RunStatus::Cancelled);
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
    tokio::time::timeout(Duration::from_secs(5), ready.notified())
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
    tokio::time::timeout(Duration::from_secs(10), started.notified())
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

#[tokio::test(flavor = "multi_thread")]
async fn continue_pressed_while_a_stop_winds_down_carries_the_turn_on_once_it_has() {
    let asked = Asked::default();
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let script: Script = {
        let asked = Arc::clone(&asked);
        let entered = Arc::clone(&entered);
        let release = Arc::clone(&release);
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
    };
    let fixture = fixture(script).await;
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
    fixture.hub.cancel(&fixture.thread_id).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), entered.notified())
        .await
        .unwrap();
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
    release.notify_one();

    let continued = fixture.settled().await.remove(0);
    assert_eq!(continued.status(), RunStatus::Done);
    assert_eq!(
        continued.continuation,
        Some(artisan_api::Continuation::Requested)
    );
    assert_eq!(asked.lock().unwrap().len(), 2);
    assert!(
        texts(&fixture.items().await)
            .iter()
            .any(|(text, _)| text == "picked it back up")
    );
}
