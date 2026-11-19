use super::*;

#[tokio::test(flavor = "multi_thread")]
async fn a_spawn_whose_prompt_is_refused_leaves_no_working_agent_behind() {
    let fixture = fixture(Arc::new(|_, _| Box::pin(async { Outcome::done() }))).await;
    let oversized = "x".repeat(33 * 1024 * 1024);

    let spawned = fixture
        .hub
        .spawn_agent(
            &fixture.thread_id,
            SpawnAgent {
                task: "Read everything".to_owned(),
                prompt: oversized,
                ..SpawnAgent::default()
            },
        )
        .await;

    spawned.unwrap_err();
    assert!(
        fixture
            .hub
            .agents(&fixture.thread_id)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        fixture
            .items()
            .await
            .iter()
            .all(|item| !matches!(item.kind, ItemKind::Agent { .. })),
        "the parent shows no agent that never got its prompt"
    );
    assert!(
        fixture
            .hub
            .store()
            .agent_threads(&fixture.thread_id)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_run_launched_in_auto_cannot_start_a_full_access_agent_after_its_thread_switches() {
    let fixture = fixture(Arc::new(|turn, mut io| {
        Box::pin(async move {
            if turn.input.text != "work" {
                return Outcome::done();
            }
            io.emit(reply("m1", "working")).await;
            while let Some(input) = io.inbox.recv().await {
                if matches!(input, Input::SetPermissions(Permissions::Full)) {
                    break;
                }
            }
            io.emit(reply("m2", "switched")).await;
            io.inbox.recv().await;
            Outcome::done()
        })
    }))
    .await;
    fixture
        .hub
        .send_message(&fixture.thread_id, message("work"))
        .await
        .unwrap();
    fixture
        .wait_for(|items| texts(items).iter().any(|(text, _)| text == "working"))
        .await;
    let mut config = fixture
        .hub
        .store()
        .thread(&fixture.thread_id)
        .await
        .unwrap()
        .unwrap()
        .config;
    config.permissions = Permissions::Full;
    fixture
        .hub
        .update_thread(
            &fixture.thread_id,
            artisan_api::UpdateThread {
                config: Some(config),
                ..artisan_api::UpdateThread::default()
            },
        )
        .await
        .unwrap();
    fixture
        .wait_for(|items| texts(items).iter().any(|(text, _)| text == "switched"))
        .await;

    let agent = fixture
        .hub
        .spawn_agent(
            &fixture.thread_id,
            SpawnAgent {
                task: "Escape".to_owned(),
                prompt: "agent prompt".to_owned(),
                permissions: Some(Permissions::Full),
                ..SpawnAgent::default()
            },
        )
        .await
        .unwrap();

    fixture
        .hub
        .send_message(&fixture.thread_id, message("finish"))
        .await
        .unwrap();
    fixture.settled().await;
    assert_eq!(
        agent.permissions,
        Permissions::Auto,
        "a still-sandboxed run started an agent with Full access"
    );
}
