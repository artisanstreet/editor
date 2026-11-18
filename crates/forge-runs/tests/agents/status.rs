use super::*;

#[tokio::test(flavor = "multi_thread")]
async fn an_agent_whose_run_failed_before_taking_its_prompt_reports_failed() {
    let fixture = fixture(Arc::new(|_, _| Box::pin(async { Outcome::done() }))).await;
    let store = fixture.hub.store();
    let parent = store.thread(&fixture.thread_id).await.unwrap().unwrap();
    let agent = ItemId::new();
    let child = ThreadId::new();
    let (_, _, prompt) = store
        .spawn_agent(
            forge_store::NewItem {
                id: agent.clone(),
                thread_id: parent.id.clone(),
                run_id: None,
                parent_id: None,
                native_id: None,
                status: Status::Running,
                kind: ItemKind::Agent {
                    name: None,
                    description: "Review the diff".into(),
                    prompt: None,
                    agent_id: None,
                    context_used: None,
                    model: None,
                    background: false,
                    result: None,
                    label: None,
                    engine: Some(EngineId::Claude),
                    thread: Some(child.clone()),
                },
            },
            CreateThread {
                id: child.clone(),
                project_id: parent.project_id.clone(),
                config: parent.config.clone(),
                title: Some("Review the diff".into()),
                parent_thread_id: Some(parent.id.clone()),
                spawned_by: Some(agent.clone()),
            },
            message("review it"),
            forge_store::Acceptance::default(),
        )
        .await
        .unwrap();
    store
        .upsert_run(artisan_api::Run {
            config: None,
            id: RunId::new(),
            thread_id: child.clone(),
            engine: EngineId::Claude,
            model: None,
            input_cutoff: prompt.seq,
            status: RunStatus::Failed,
            usage: artisan_api::Usage::default(),
            session_id: None,
            error: Some("the working directory is gone".into()),
            seq: 0,
            started_at: artisan_api::now(),
            ended_at: Some(artisan_api::now()),
            continuation: None,
        })
        .await
        .unwrap();

    let status = fixture
        .hub
        .agent_status(&parent.id, agent.as_str())
        .await
        .unwrap();

    assert_eq!(status.state, agents::AgentState::Failed);
    assert_eq!(
        status.error.as_deref(),
        Some("the working directory is gone")
    );
    fixture.hub.shutdown();
}
