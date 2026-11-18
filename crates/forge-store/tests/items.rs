use artisan_api::{
    CreateProject, CreateThread, EngineConfig, EngineId, ItemId, ItemKind, Phase, ProjectId,
    Status, ThreadId,
};
use forge_store::{NewItem, Store, WAITING_ITEMS};

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
