use super::*;

#[tokio::test(flavor = "multi_thread")]
async fn cancellation_waits_for_adapter_cleanup_and_drains_its_final_events() {
    for engine in [EngineId::Codex, EngineId::Claude, EngineId::OpenCode] {
        let entered = Arc::new(tokio::sync::Notify::new());
        let ready = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
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
                .status,
            RunStatus::Running
        );
        release.notify_one();
        let runs = fixture.settled().await;
        assert!(cleaned.load(Ordering::SeqCst));
        assert_eq!(runs.last().unwrap().status, RunStatus::Cancelled);
        assert_eq!(fixture.items().await.iter().filter(|item| matches!(&item.kind, ItemKind::AssistantMessage { text, .. } if text.inline() == Some("stopped"))).count(), 300);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn cancellation_preserves_an_adapter_stop_failure() {
    let ready = Arc::new(tokio::sync::Notify::new());
    let script_ready = Arc::clone(&ready);
    let fixture = fixture(Arc::new(move |_, io| {
        let ready = Arc::clone(&script_ready);
        Box::pin(async move {
            ready.notify_one();
            io.cancel.cancelled().await;
            Outcome::failed("provider refused to stop".to_owned())
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
    assert_eq!(runs.last().unwrap().status, RunStatus::Failed);
    assert_eq!(
        runs.last().unwrap().error.as_deref(),
        Some("provider refused to stop")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn cancellation_timeout_reports_unconfirmed_work_as_failed() {
    let ready = Arc::new(tokio::sync::Notify::new());
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
    tokio::time::timeout(Duration::from_secs(50), async {
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
    assert_eq!(run.status, RunStatus::Failed);
    assert!(run.error.unwrap().contains("did not confirm it stopped"));
}

#[tokio::test(flavor = "multi_thread")]
async fn cancellation_rejects_answers_and_goal_updates_while_cleanup_is_pending() {
    let release = Arc::new(tokio::sync::Notify::new());
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
        fixture.settled().await.last().unwrap().status,
        RunStatus::Cancelled,
        "cleanup must still complete cancellation after rejecting inputs"
    );
}
