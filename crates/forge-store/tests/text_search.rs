use artisan_api::{
    CreateProject, CreateThread, EngineConfig, EngineId, ItemId, ItemKind, Phase, ProjectId,
    Status, TextSearchQuery, ThreadId,
};
use forge_store::{NewItem, Store};
use std::path::PathBuf;

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("forge-text-search-test-{}", ItemId::new()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

async fn thread(store: &Store, title: &str, parent: Option<ThreadId>) -> ThreadId {
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
            title: Some(title.into()),
            parent_thread_id: parent,
            spawned_by: None,
        })
        .await
        .unwrap()
        .id
}

fn said(thread: &ThreadId, text: &str, phase: Phase) -> NewItem {
    NewItem {
        id: ItemId::new(),
        thread_id: thread.clone(),
        run_id: None,
        parent_id: None,
        native_id: None,
        status: Status::Done,
        kind: ItemKind::AssistantMessage {
            text: text.into(),
            phase,
        },
    }
}

#[tokio::test]
async fn a_search_finds_the_message_the_line_and_the_exact_match() {
    let dir = TempDir::new();
    let store = Store::open(&dir.0).unwrap();
    let fixed = thread(&store, "Scene bound fix", None).await;
    let child = thread(&store, "Worker", Some(fixed.clone())).await;
    let reply = "The thread froze.\nRaising the Scene Bound to 2048 items fixed the freeze for good.";
    let answer = store
        .upsert_item(said(&fixed, reply, Phase::Final))
        .await
        .unwrap();
    store
        .upsert_item(said(&fixed, "Checking the scene bound first.", Phase::Commentary))
        .await
        .unwrap();
    store
        .upsert_item(said(&child, "The scene bound is 512.", Phase::Final))
        .await
        .unwrap();

    let hits = store
        .text_search(TextSearchQuery {
            query: " scene bound ".into(),
            limit: None,
        })
        .await
        .unwrap();

    assert_eq!(hits.len(), 1);
    let hit = &hits[0];
    assert_eq!((&hit.thread_id, &hit.item_id), (&fixed, &answer.id));
    assert_eq!(hit.thread_title.as_deref(), Some("Scene bound fix"));
    assert_eq!(hit.position, answer.position);
    let (start, end) = (hit.offset.0 as usize, hit.offset.1 as usize);
    assert_eq!(&reply[start..end], "Scene Bound");
    let (from, to) = (hit.matched.0 as usize, hit.matched.1 as usize);
    assert_eq!(&hit.snippet[from..to], "Scene Bound");
    assert_eq!(
        hit.snippet,
        "Raising the Scene Bound to 2048 items fixed the freeze for good."
    );
    assert!(
        store
            .text_search(TextSearchQuery {
                query: "no such words".into(),
                limit: None,
            })
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn a_long_line_is_cut_around_the_match() {
    let dir = TempDir::new();
    let store = Store::open(&dir.0).unwrap();
    let home = thread(&store, "Long lines", None).await;
    let line = format!("{} the needle sits here {}", "before ".repeat(30), "after ".repeat(40));
    store
        .upsert_item(said(&home, &line, Phase::Final))
        .await
        .unwrap();

    let hits = store
        .text_search(TextSearchQuery {
            query: "needle".into(),
            limit: None,
        })
        .await
        .unwrap();

    let hit = &hits[0];
    assert!(hit.snippet.starts_with('…') && hit.snippet.ends_with('…'));
    assert!(hit.snippet.chars().count() <= 122);
    assert_eq!(
        &hit.snippet[hit.matched.0 as usize..hit.matched.1 as usize],
        "needle"
    );
}
