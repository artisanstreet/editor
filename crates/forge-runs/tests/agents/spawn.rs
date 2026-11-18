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
