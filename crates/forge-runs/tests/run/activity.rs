use super::*;

#[tokio::test(flavor = "multi_thread")]
async fn a_native_worker_resumed_in_the_same_turn_returns_to_the_inspector() {
    let ready = Arc::new(tokio::sync::Notify::new());
    let finish = Arc::new(tokio::sync::Notify::new());
    let script_ready = Arc::clone(&ready);
    let script_finish = Arc::clone(&finish);
    let fixture = fixture(Arc::new(move |_, io| {
        let ready = Arc::clone(&script_ready);
        let finish = Arc::clone(&script_finish);
        Box::pin(async move {
            for (status, description) in [
                (Status::Running, "Initial work"),
                (Status::Done, "Initial report"),
                (Status::Running, "Followup"),
            ] {
                io.emit(EngineEvent::Item {
                    key: "native-worker".into(),
                    parent: None,
                    status,
                    kind: ItemKind::Agent {
                        engine: None,
                        thread: None,
                        name: Some("/root/worker".into()),
                        description: description.into(),
                        prompt: None,
                        agent_id: Some("native-worker".into()),
                        context_used: None,
                        model: None,
                        background: false,
                        result: None,
                        label: None,
                    },
                })
                .await;
            }
            ready.notify_one();
            finish.notified().await;
            Outcome::done()
        })
    }))
    .await;
    fixture
        .hub
        .send_message(&fixture.thread_id, message("work"))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), ready.notified())
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let summary = fixture
                .hub
                .store()
                .inspector(&fixture.thread_id)
                .await
                .unwrap();
            if summary.agents.len() == 1
                && summary.agents[0].status == Status::Running
                && summary.agents[0].description.inline() == Some("Followup")
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    finish.notify_one();
    fixture.settled().await;
    let agents: Vec<_> = fixture
        .items()
        .await
        .into_iter()
        .filter(|item| matches!(item.kind, ItemKind::Agent { .. }))
        .collect();
    assert_eq!(agents.len(), 1);
    assert_eq!(agents[0].status, Status::Done);
}
