use super::*;

async fn background_agent(fixture: &Fixture, parent: &artisan_api::Thread) -> (ItemId, ThreadId) {
    let store = fixture.hub.store();
    let agent = ItemId::new();
    let child = ThreadId::new();
    store
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
                    background: true,
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
    (agent, child)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_background_result_that_could_not_be_delivered_is_delivered_after_recovery() {
    let release = Arc::new(Notify::new());
    let fixture = fixture(Arc::new({
        let release = Arc::clone(&release);
        move |turn, io| {
            let release = Arc::clone(&release);
            Box::pin(async move {
                if turn.input.text == "review it" {
                    release.notified().await;
                    io.emit(reply("verdict", "looks fine")).await;
                }
                Outcome::done()
            })
        }
    }))
    .await;
    let store = fixture.hub.store();
    let parent = store.thread(&fixture.thread_id).await.unwrap().unwrap();
    let (agent, child) = background_agent(&fixture, &parent).await;
    fixture.hub.resume_pending();
    test_support::wait::eventually(
        "the agent to start",
        test_support::wait::LIVENESS,
        || async { fixture.delivered(&child).await },
    )
    .await;
    let disk =
        rusqlite::Connection::open(fixture.dir.path().join("forge").join("forge.sqlite3")).unwrap();
    disk.busy_timeout(test_support::wait::LIVENESS).unwrap();
    disk.execute(
        &include_str!("fill_disk_for_thread.sql").replace("__THREAD__", &parent.id.to_string()),
        [],
    )
    .unwrap();

    release.notify_one();
    test_support::wait::eventually(
        "the agent's run to end",
        test_support::wait::LIVENESS,
        || async {
            !fixture.hub.is_live(&child)
                && store
                    .latest_run(&child)
                    .await
                    .unwrap()
                    .is_some_and(|run| run.status() == RunStatus::Done)
        },
    )
    .await;
    fixture.quiescent().await;
    disk.execute_batch(include_str!("drop_disk_full.sql"))
        .unwrap();
    fixture.hub.reconcile_agents().await;

    let reports: Vec<Item> = fixture
        .items()
        .await
        .into_iter()
        .filter(|item| {
            matches!(&item.kind, ItemKind::UserMessage { from_agent: Some(from), .. } if *from == agent)
        })
        .collect();
    assert_eq!(
        reports.len(),
        1,
        "the parent hears of its agent exactly once"
    );
    assert!(
        reports[0]
            .preview()
            .is_some_and(|text| text.contains("looks fine"))
    );
    fixture.hub.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_finished_background_agent_reports_to_its_parent_in_a_managed_generation() {
    let fixture = fixture_in_generation(Arc::new(|turn, io| {
        Box::pin(async move {
            if turn.input.text == "review it" {
                io.emit(reply("verdict", "looks fine")).await;
            }
            Outcome::done()
        })
    }))
    .await;
    let store = fixture.hub.store();
    let parent = store.thread(&fixture.thread_id).await.unwrap().unwrap();
    let (agent, child) = background_agent(&fixture, &parent).await;
    fixture.hub.resume_pending();
    test_support::wait::eventually(
        "the parent to hear its agent finished",
        test_support::wait::LIVENESS,
        || async {
            fixture
                .items()
                .await
                .iter()
                .any(|item| {
                    matches!(&item.kind, ItemKind::UserMessage { from_agent: Some(from), .. } if *from == agent)
                })
        },
    )
    .await;
    assert_eq!(
        store.item(&agent).await.unwrap().unwrap().status,
        Status::Done,
        "the agent row of {child} stops running"
    );
    fixture.hub.shutdown();
}

async fn reports_from(fixture: &Fixture, agent: &ItemId) -> usize {
    fixture
        .items()
        .await
        .iter()
        .filter(|item| {
            matches!(&item.kind, ItemKind::UserMessage { from_agent: Some(from), .. } if from == agent)
        })
        .count()
}

async fn reported(fixture: &Fixture, agent: &ItemId, count: usize) {
    test_support::wait::eventually(
        "the parent to hear of its agent",
        test_support::wait::LIVENESS,
        || async { reports_from(fixture, agent).await == count },
    )
    .await;
}

async fn agent_reaches(fixture: &Fixture, agent: &ItemId, status: Status) {
    test_support::wait::eventually(
        "the agent row to reach its status",
        test_support::wait::LIVENESS,
        || async {
            fixture
                .hub
                .store()
                .item(agent)
                .await
                .unwrap()
                .unwrap()
                .status
                == status
        },
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_settled_agent_that_is_woken_works_again_and_reports_again() {
    let release = Arc::new(Notify::new());
    let fixture = fixture_in_generation(Arc::new({
        let release = Arc::clone(&release);
        move |turn, io| {
            let release = Arc::clone(&release);
            Box::pin(async move {
                match turn.input.text.as_str() {
                    "review it" => io.emit(reply("verdict", "looks fine")).await,
                    "Your reviewers finished." => {
                        release.notified().await;
                        io.emit(reply("final", "all reviewed")).await;
                    }
                    _ => {}
                }
                Outcome::done()
            })
        }
    }))
    .await;
    let store = fixture.hub.store();
    let parent = store.thread(&fixture.thread_id).await.unwrap().unwrap();
    let (agent, child) = background_agent(&fixture, &parent).await;
    fixture.hub.resume_pending();
    reported(&fixture, &agent, 1).await;
    agent_reaches(&fixture, &agent, Status::Done).await;
    fixture
        .hub
        .send_message(&child, message("Your reviewers finished."))
        .await
        .unwrap();
    agent_reaches(&fixture, &agent, Status::Running).await;
    release.notify_one();
    reported(&fixture, &agent, 2).await;
    agent_reaches(&fixture, &agent, Status::Done).await;
    fixture.hub.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_follow_up_cancelled_before_it_runs_settles_without_reporting_again() {
    let fixture = fixture_in_generation(Arc::new(|turn, io| {
        Box::pin(async move {
            if turn.input.text == "review it" {
                io.emit(reply("verdict", "looks fine")).await;
            }
            Outcome::done()
        })
    }))
    .await;
    let store = fixture.hub.store();
    let parent = store.thread(&fixture.thread_id).await.unwrap().unwrap();
    let (agent, child) = background_agent(&fixture, &parent).await;
    fixture.hub.resume_pending();
    reported(&fixture, &agent, 1).await;
    agent_reaches(&fixture, &agent, Status::Done).await;
    fixture.quiescent().await;
    fixture
        .hub
        .generation()
        .unwrap()
        .set_phase(runtime::Phase::Standby);
    let info = fixture
        .hub
        .message_agent(&fixture.thread_id, agent.as_str(), "One more thing.".into())
        .await
        .unwrap();
    assert_eq!(info.state, AgentState::Working);
    fixture.hub.cancel(&child).await.unwrap();
    agent_reaches(&fixture, &agent, Status::Done).await;
    assert_eq!(
        reports_from(&fixture, &agent).await,
        1,
        "the parent hears the agent's old result only once"
    );
    fixture.hub.shutdown();
}

async fn overview_agents(fixture: &Fixture) -> Vec<ItemId> {
    fixture
        .hub
        .store()
        .inspector(&fixture.thread_id)
        .await
        .unwrap()
        .agents
        .into_iter()
        .map(|agent| agent.id)
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_finished_agent_leaves_the_thread_overview_without_its_parent_asking() {
    let release = Arc::new(Notify::new());
    let fixture = fixture_in_generation(Arc::new({
        let release = Arc::clone(&release);
        move |turn, io| {
            let release = Arc::clone(&release);
            Box::pin(async move {
                if turn.input.text == "review it" {
                    release.notified().await;
                    io.emit(reply("verdict", "looks fine")).await;
                }
                Outcome::done()
            })
        }
    }))
    .await;
    let store = fixture.hub.store();
    let parent = store.thread(&fixture.thread_id).await.unwrap().unwrap();
    let (agent, child) = background_agent(&fixture, &parent).await;
    fixture.hub.resume_pending();
    test_support::wait::eventually(
        "the agent to start",
        test_support::wait::LIVENESS,
        || async { fixture.delivered(&child).await },
    )
    .await;
    assert_eq!(overview_agents(&fixture).await, vec![agent.clone()]);
    release.notify_one();
    test_support::wait::eventually(
        "the finished agent to leave the overview",
        test_support::wait::LIVENESS,
        || async { overview_agents(&fixture).await.is_empty() },
    )
    .await;
    assert_eq!(
        store.item(&agent).await.unwrap().unwrap().status,
        Status::Done
    );
    fixture.hub.shutdown();
}
