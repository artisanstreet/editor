use super::*;

async fn spawned_agent(fixture: &Fixture) -> (ThreadId, ItemId, ThreadId, Item) {
    described_agent(fixture, "Review the diff").await
}

async fn described_agent(
    fixture: &Fixture,
    description: &str,
) -> (ThreadId, ItemId, ThreadId, Item) {
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
                    description: description.to_owned().into(),
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
    (parent.id, agent, child, prompt)
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agent_whose_run_failed_before_taking_its_prompt_reports_failed() {
    let fixture = fixture(Arc::new(|_, _| Box::pin(async { Outcome::done() }))).await;
    let (parent, agent, child, prompt) = spawned_agent(&fixture).await;
    let store = fixture.hub.store();
    store
        .upsert_run(artisan_api::Run {
            config: None,
            id: RunId::new(),
            thread_id: child.clone(),
            engine: EngineId::Claude,
            model: None,
            input_cutoff: prompt.seq,
            state: artisan_api::RunState::Finished {
                outcome: artisan_api::RunOutcome::Failed,
                error: Some("the working directory is gone".into()),
                ended_at: artisan_api::now(),
            },
            usage: artisan_api::Usage::default(),
            session_id: None,
            seq: 0,
            started_at: artisan_api::now(),
            continuation: None,
        })
        .await
        .unwrap();

    let status = fixture
        .hub
        .agent_status(&parent, agent.as_str())
        .await
        .unwrap();

    assert_eq!(status.state, AgentState::Failed);
    assert_eq!(
        status.error.as_deref(),
        Some("the working directory is gone")
    );
    fixture.hub.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agent_stopped_before_its_first_run_reports_cancelled_to_its_waiter() {
    let fixture = fixture(Arc::new(|_, _| Box::pin(async { Outcome::done() }))).await;
    let _every_slot = Arc::clone(&fixture.hub.inner.harness_slots)
        .acquire_many_owned(u32::try_from(RUN_CAPACITY).unwrap())
        .await
        .unwrap();
    let (parent, agent, child, _) = spawned_agent(&fixture).await;
    let waiting = {
        let hub = fixture.hub.clone();
        let parent = parent.clone();
        let agent = agent.to_string();
        tokio::spawn(async move {
            hub.wait_agents(&parent, &[agent], false, Duration::from_secs(600))
                .await
                .unwrap()
        })
    };
    test_support::wait::eventually(
        "the waiter to register",
        test_support::wait::LIVENESS,
        || async { !fixture.hub.inner.agent_waiters.lock().unwrap().is_empty() },
    )
    .await;

    fixture.hub.cancel(&child).await.unwrap();

    let agents = tokio::time::timeout(test_support::wait::LIVENESS, waiting)
        .await
        .expect("the waiter hears that the agent was stopped")
        .unwrap();
    assert_eq!(agents[0].state, AgentState::Cancelled);
    let status = fixture
        .hub
        .agent_status(&parent, agent.as_str())
        .await
        .unwrap();
    assert_eq!(status.state, AgentState::Cancelled);
    fixture.hub.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unlabelled_agent_with_a_long_task_is_listed_by_its_whole_task() {
    let fixture = fixture(Arc::new(|_, _| Box::pin(async { Outcome::done() }))).await;
    let task = "Review every file in the diff. ".repeat(artisan_api::INLINE_TEXT_BYTES / 16);
    let (parent, agent, _, _) = described_agent(&fixture, &task).await;
    let disk =
        rusqlite::Connection::open(fixture.dir.path().join("forge").join("forge.sqlite3")).unwrap();
    disk.busy_timeout(test_support::wait::LIVENESS).unwrap();
    disk.execute(include_str!("forget_label.sql"), [agent.as_str()])
        .unwrap();

    let status = fixture
        .hub
        .agent_status(&parent, agent.as_str())
        .await
        .unwrap();

    assert_eq!(status.task, task);
    fixture.hub.shutdown();
}
