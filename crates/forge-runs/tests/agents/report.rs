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
    let release = Arc::new(tokio::sync::Notify::new());
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
    test_support::wait::eventually("the agent to start", Duration::from_secs(10), || async {
        fixture.delivered(&child).await
    })
    .await;
    let disk =
        rusqlite::Connection::open(fixture.dir.path().join("forge").join("forge.sqlite3")).unwrap();
    disk.busy_timeout(Duration::from_secs(5)).unwrap();
    disk.execute(
        &format!(
            "CREATE TRIGGER disk_full BEFORE INSERT ON items
                 WHEN NEW.thread_id = '{}' AND NEW.kind_type = 'user_message'
                 BEGIN SELECT RAISE(ABORT, 'database or disk is full'); END",
            parent.id
        ),
        [],
    )
    .unwrap();

    release.notify_one();
    test_support::wait::eventually(
        "the agent's run to end",
        Duration::from_secs(10),
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
    disk.execute_batch("DROP TRIGGER disk_full").unwrap();
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
        Duration::from_secs(10),
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
