use artisan_api::AGENT_NAMES;
use artisan_api::{
    CreateProject, CreateThread, EngineConfig, EngineId, ItemId, ItemKind, Phase, ProjectId,
    Status, ThreadId,
};
use forge_store::{NewItem, Store, WAITING_ITEMS};
use std::collections::HashSet;

#[tokio::test]
async fn the_items_waiting_on_the_user_stop_at_the_limit() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let project = store
        .create_project(CreateProject {
            id: ProjectId::new(),
            path: "/work/app".into(),
            name: None,
        })
        .await
        .unwrap();
    let thread = store
        .create_thread(CreateThread {
            id: ThreadId::new(),
            project_id: project.id,
            config: EngineConfig::new(EngineId::Claude),
            title: None,
            parent_thread_id: None,
            spawned_by: None,
        })
        .await
        .unwrap()
        .id;
    for index in 0..WAITING_ITEMS + 5 {
        store
            .upsert_item(NewItem {
                id: ItemId::from(format!("waiting-{index}").as_str()),
                thread_id: thread.clone(),
                run_id: None,
                parent_id: None,
                native_id: None,
                status: Status::AwaitingUser,
                kind: ItemKind::AssistantMessage {
                    text: "Allow?".into(),
                    phase: Phase::Final,
                },
            })
            .await
            .unwrap();
    }
    let waiting = store.items_awaiting_user().await.unwrap();
    assert_eq!(waiting.len(), WAITING_ITEMS);
    assert_eq!(
        waiting[0].id.as_str(),
        "waiting-0",
        "the oldest wait comes first"
    );
}

#[expect(
    clippy::unwrap_used,
    reason = "test helper; a failure fails the calling test"
)]
async fn thread_in(store: &Store) -> ThreadId {
    let project = store
        .create_project(CreateProject {
            id: ProjectId::new(),
            path: "/work/app".into(),
            name: None,
        })
        .await
        .unwrap();
    store
        .create_thread(CreateThread {
            id: ThreadId::new(),
            project_id: project.id,
            config: EngineConfig::new(EngineId::Claude),
            title: None,
            parent_thread_id: None,
            spawned_by: None,
        })
        .await
        .unwrap()
        .id
}

fn agent(thread: &ThreadId, id: &str, status: Status, label: Option<String>) -> NewItem {
    NewItem {
        id: ItemId::from(id),
        thread_id: thread.clone(),
        run_id: None,
        parent_id: None,
        native_id: None,
        status,
        kind: ItemKind::Agent {
            name: None,
            description: "Review the diff".into(),
            prompt: None,
            agent_id: None,
            context_used: None,
            model: None,
            background: true,
            result: None,
            label,
            engine: Some(EngineId::Claude),
            thread: None,
        },
    }
}

#[expect(
    clippy::unwrap_used,
    reason = "test helper; a failure fails the calling test"
)]
async fn spawn_named(store: &Store, thread: &ThreadId, id: &str) -> String {
    store
        .upsert_item(agent(thread, id, Status::Running, None))
        .await
        .unwrap()
        .agent_label()
        .unwrap()
}

#[expect(
    clippy::unwrap_used,
    reason = "test helper; a failure fails the calling test"
)]
async fn live_names(store: &Store, ids: &[String]) -> Vec<String> {
    let mut names = Vec::new();
    for id in ids {
        let item = store
            .item(&ItemId::from(id.as_str()))
            .await
            .unwrap()
            .unwrap();
        if !item.status.is_terminal() {
            names.push(item.agent_label().unwrap().to_lowercase());
        }
    }
    names
}

#[tokio::test]
async fn agents_started_after_others_finished_get_plain_names() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let thread = thread_in(&store).await;
    for round in 0..2 {
        for index in 0..AGENT_NAMES.len() {
            let id = format!("round-{round}-agent-{index}");
            let name = spawn_named(&store, &thread, &id).await;
            assert!(
                AGENT_NAMES.contains(&name.as_str()),
                "agent {index} of round {round} is named {name}"
            );
            store
                .upsert_item(agent(&thread, &id, Status::Done, Some(name)))
                .await
                .unwrap();
        }
    }
}

#[tokio::test]
async fn a_finished_agent_woken_while_another_holds_its_name_is_renamed() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let thread = thread_in(&store).await;
    let first = spawn_named(&store, &thread, "first").await;
    store
        .upsert_item(agent(&thread, "first", Status::Done, Some(first.clone())))
        .await
        .unwrap();
    let mut ids = vec!["first".to_owned()];
    for index in 0..AGENT_NAMES.len() {
        let id = format!("next-{index}");
        spawn_named(&store, &thread, &id).await;
        ids.push(id);
    }
    let woken = store
        .upsert_item(agent(
            &thread,
            "first",
            Status::Running,
            Some(first.clone()),
        ))
        .await
        .unwrap()
        .agent_label()
        .unwrap();
    assert_ne!(woken, first, "two live agents are both called {first}");
    let names = live_names(&store, &ids).await;
    let unique: HashSet<&String> = names.iter().collect();
    assert_eq!(
        unique.len(),
        names.len(),
        "live agents share a name: {names:?}"
    );
    let still = store
        .upsert_item(agent(&thread, "first", Status::AwaitingUser, None))
        .await
        .unwrap()
        .agent_label();
    assert_eq!(still, Some(woken), "a live agent keeps its name");
}

#[tokio::test(flavor = "multi_thread")]
async fn agents_started_at_once_never_share_a_name() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let thread = thread_in(&store).await;
    let ids: Vec<String> = (0..AGENT_NAMES.len() + 20)
        .map(|index| format!("agent-{index}"))
        .collect();
    let mut spawns = tokio::task::JoinSet::new();
    for id in ids.clone() {
        let store = store.clone();
        let thread = thread.clone();
        spawns.spawn(async move { spawn_named(&store, &thread, &id).await });
    }
    while let Some(spawned) = spawns.join_next().await {
        spawned.unwrap();
    }
    let names = live_names(&store, &ids).await;
    let unique: HashSet<&String> = names.iter().collect();
    assert_eq!(names.len(), ids.len());
    assert_eq!(
        unique.len(),
        names.len(),
        "live agents share a name: {names:?}"
    );
}
