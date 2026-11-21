use artisan_api::{
    CreateProject, CreateThread, EngineConfig, EngineId, ItemId, ItemKind, ProjectId, RunId,
    Status, ThreadId,
};
use forge_store::{NewItem, Store};

const SENT_PROMPTS: usize = 1024;

#[expect(
    clippy::unwrap_used,
    reason = "test helper; a failure fails the calling test"
)]
async fn thread(store: &Store) -> ThreadId {
    let project = store
        .create_project(CreateProject {
            id: ProjectId::new(),
            path: "/work/outline".into(),
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

fn prompt(thread: &ThreadId, id: &str, run: Option<&str>, steer: bool) -> NewItem {
    NewItem {
        id: ItemId::from(id),
        thread_id: thread.clone(),
        run_id: run.map(RunId::from),
        parent_id: None,
        native_id: None,
        status: Status::Done,
        kind: ItemKind::UserMessage {
            text: format!("Prompt {id}").into(),
            images: Vec::new(),
            steer,
            from_agent: None,
            sender: None,
        },
    }
}

fn reply(thread: &ThreadId, id: &str) -> NewItem {
    NewItem {
        id: ItemId::from(id),
        thread_id: thread.clone(),
        run_id: Some(RunId::from("run")),
        parent_id: None,
        native_id: None,
        status: Status::Done,
        kind: ItemKind::AssistantMessage {
            text: "Done".into(),
            phase: artisan_api::Phase::Final,
        },
    }
}

fn labels(summary: &artisan_api::InspectorSummary) -> Vec<&str> {
    summary
        .outline
        .iter()
        .map(|entry| entry.label.as_str())
        .collect()
}

#[tokio::test]
async fn an_inspector_refresh_resends_the_outline_only_after_a_prompt_changes() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let thread = thread(&store).await;
    for id in ["a", "b"] {
        store
            .upsert_item(prompt(&thread, id, None, false))
            .await
            .unwrap();
    }
    let first = store.inspector_since(&thread, None).await.unwrap();
    assert_eq!(labels(&first), ["Prompt a", "Prompt b"]);
    let known = first.outline_revision;
    store.upsert_item(reply(&thread, "answer")).await.unwrap();
    let quiet = store.inspector_since(&thread, Some(known)).await.unwrap();
    assert_eq!(quiet.outline_revision, known);
    assert!(
        quiet.outline.is_empty(),
        "an unchanged outline is not resent"
    );
    assert!(quiet.revision > first.revision);

    store
        .upsert_item(prompt(&thread, "steer", None, true))
        .await
        .unwrap();
    let steering = store.inspector_since(&thread, Some(known)).await.unwrap();
    assert_eq!(labels(&steering), ["Prompt a", "Prompt b", "Prompt steer"]);
    assert!(steering.outline_revision > known);

    store
        .upsert_item(prompt(&thread, "steer", Some("run"), true))
        .await
        .unwrap();
    let accepted = store
        .inspector_since(&thread, Some(steering.outline_revision))
        .await
        .unwrap();
    assert_eq!(
        labels(&accepted),
        ["Prompt a", "Prompt b"],
        "a steer that joined the turn leaves the outline"
    );
    assert!(accepted.outline_revision > steering.outline_revision);
}

#[tokio::test]
async fn the_outline_sends_the_newest_prompts_of_a_thread_with_more_than_it_carries() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let thread = thread(&store).await;
    let prompts = SENT_PROMPTS + 6;
    for index in 0..prompts {
        store
            .upsert_item(prompt(&thread, &format!("{index:05}"), None, false))
            .await
            .unwrap();
    }
    let summary = store.inspector(&thread).await.unwrap();
    assert_eq!(summary.outline.len(), prompts - 6);
    assert_eq!(summary.outline[0].label, "Prompt 00006");
    assert_eq!(
        summary.outline.last().map(|entry| entry.label.as_str()),
        Some(format!("Prompt {:05}", prompts - 1).as_str())
    );
    assert!(
        summary
            .outline
            .windows(2)
            .all(|pair| pair[0].position < pair[1].position)
    );
}
