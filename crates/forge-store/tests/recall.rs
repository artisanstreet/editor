//! Recall finds what was said and done in other threads.

use artisan_api::{
    Collection, CreateProject, CreateThread, EngineConfig, EngineId, FileChange, FileOp, ItemId,
    ItemKind, Output, Phase, ProjectId, Status, ThreadId, ToolKind,
};
use forge_store::{GrepQuery, NewItem, RecallScope, Store};
use std::path::PathBuf;

struct TempDir(PathBuf);

impl TempDir {
    #[expect(
        clippy::unwrap_used,
        reason = "test helper; a failure fails the calling test"
    )]
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("forge-recall-test-{}", ItemId::new()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        // Best effort: a leftover temp dir is harmless.
        std::fs::remove_dir_all(&self.0).ok();
    }
}

#[expect(
    clippy::unwrap_used,
    reason = "test helper; a failure fails the calling test"
)]
async fn thread(store: &Store, engine: EngineId, title: &str) -> ThreadId {
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
            config: EngineConfig::new(engine),
            title: Some(title.into()),
            parent_thread_id: None,
            spawned_by: None,
        })
        .await
        .unwrap()
        .id
}

fn item(thread: &ThreadId, status: Status, kind: ItemKind) -> NewItem {
    NewItem {
        id: ItemId::new(),
        thread_id: thread.clone(),
        run_id: None,
        parent_id: None,
        native_id: None,
        status,
        kind,
    }
}

fn said(thread: &ThreadId, text: &str) -> NewItem {
    item(
        thread,
        Status::Done,
        ItemKind::AssistantMessage {
            text: text.into(),
            phase: Phase::Final,
        },
    )
}

#[tokio::test]
#[expect(clippy::too_many_lines, reason = "one scenario walked step by step")]
async fn an_agent_finds_what_another_thread_said_and_where() {
    let dir = TempDir::new();
    let store = Store::open(&dir.0).unwrap();
    let fixed = thread(&store, EngineId::Claude, "Scene bound fix").await;
    let other = thread(&store, EngineId::Codex, "Sidebar polish").await;
    store
        .upsert_item(said(
            &fixed,
            "The thread froze because the scene bound stops at 512 items.\nRaising it fixed the freeze.",
        ))
        .await
        .unwrap();
    store
        .upsert_item(said(&other, "The sidebar now groups unread threads."))
        .await
        .unwrap();
    store
        .upsert_item(item(
            &fixed,
            Status::Done,
            ItemKind::Tool {
                tool: ToolKind::Edit {
                    changes: Collection::Inline(vec![FileChange {
                        path: "crates/editor-conversation/src/scene.rs".into(),
                        op: FileOp::Modify,
                        diff: None,
                    }]),
                },
                title: None,
                output: None,
                exit_code: None,
                approval: None,
            },
        ))
        .await
        .unwrap();
    store
        .upsert_item(item(
            &other,
            Status::Done,
            ItemKind::Tool {
                tool: ToolKind::Shell {
                    command: "cargo test -p editor-sidebar".into(),
                    cwd: None,
                    background: false,
                    watch: false,
                },
                title: None,
                output: Some(Output::from_text("test result: ok. 41 passed; 0 failed")),
                exit_code: Some(0),
                approval: None,
            },
        ))
        .await
        .unwrap();

    let found = store
        .recall_search(RecallScope::default(), "scene bound".into(), false, 10, 0)
        .await
        .unwrap();
    assert_eq!(found.hits.len(), 1, "{found:?}");
    assert_eq!(found.hits[0].thread_id, fixed);
    assert_eq!(
        found.hits[0].thread_title.as_deref(),
        Some("Scene bound fix")
    );
    assert!(
        found.hits[0].snippet.contains("**sce"),
        "{}",
        found.hits[0].snippet
    );

    let codex_only = RecallScope {
        engine: Some(EngineId::Codex),
        ..RecallScope::default()
    };
    let none = store
        .recall_search(codex_only, "scene bound".into(), false, 10, 0)
        .await
        .unwrap();
    assert!(none.hits.is_empty());

    // Output is searched only when asked for.
    let quiet = store
        .recall_search(RecallScope::default(), "41 passed".into(), false, 10, 0)
        .await
        .unwrap();
    assert!(quiet.hits.is_empty());
    let loud = store
        .recall_search(RecallScope::default(), "41 passed".into(), true, 10, 0)
        .await
        .unwrap();
    assert_eq!(loud.hits[0].thread_id, other);

    let grep = store
        .recall_grep(
            RecallScope::default(),
            GrepQuery {
                pattern: r"scene bound stops at \d+".into(),
                ignore_case: false,
                output: false,
                context: 1,
                max_matches: None,
                cursor: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(grep.matches.len(), 1);
    assert_eq!(grep.matches[0].line, 1);
    assert_eq!(grep.matches[0].after, vec!["Raising it fixed the freeze."]);
    assert_eq!(grep.next, None);

    let touched = store
        .recall_touching(RecallScope::default(), "scene.rs".into(), 10)
        .await
        .unwrap();
    assert_eq!(touched.len(), 1);
    assert_eq!(touched[0].thread_id, fixed);
    assert_eq!(touched[0].op, "edit");
}

#[tokio::test]
async fn an_edited_item_is_found_by_its_new_text_only() {
    let dir = TempDir::new();
    let store = Store::open(&dir.0).unwrap();
    let thread = thread(&store, EngineId::Claude, "Drafts").await;
    let mut draft = said(&thread, "first wording about pelicans");
    store.upsert_item(draft.clone()).await.unwrap();
    draft.kind = ItemKind::AssistantMessage {
        text: "second wording about flamingos".into(),
        phase: Phase::Final,
    };
    store.upsert_item(draft).await.unwrap();
    let search =
        |query: &str| store.recall_search(RecallScope::default(), query.into(), false, 10, 0);
    assert!(search("pelicans").await.unwrap().hits.is_empty());
    assert_eq!(search("flamingos").await.unwrap().hits.len(), 1);
}

#[tokio::test]
async fn a_pattern_that_does_not_compile_says_so() {
    let dir = TempDir::new();
    let store = Store::open(&dir.0).unwrap();
    let error = store
        .recall_grep(
            RecallScope::default(),
            GrepQuery {
                pattern: "(unclosed".into(),
                ignore_case: false,
                output: false,
                context: 0,
                max_matches: None,
                cursor: None,
            },
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("doesn't compile"), "{error:#}");
}

/// Migrates a copy of a real database and times the recall backfill:
/// `FORGE_RECALL_DB=/path/to/forge.sqlite3 cargo test -- --ignored`.
#[tokio::test]
#[ignore = "needs a copy of a real database"]
async fn backfill_on_a_real_database() {
    let Ok(source) = std::env::var("FORGE_RECALL_DB") else {
        return;
    };
    let dir = TempDir::new();
    std::fs::copy(&source, dir.0.join("forge.sqlite3")).unwrap();
    let started = std::time::Instant::now();
    let store = Store::open(&dir.0).unwrap();
    eprintln!("opened and migrated in {:?}", started.elapsed());
    let started = std::time::Instant::now();
    let found = store
        .recall_search(RecallScope::default(), "scene bound".into(), false, 5, 0)
        .await
        .unwrap();
    eprintln!("search in {:?}: {:#?}", started.elapsed(), found.hits);
    let started = std::time::Instant::now();
    let grep = store
        .recall_grep(
            RecallScope::default(),
            GrepQuery {
                pattern: "Forge".into(),
                ignore_case: false,
                output: true,
                context: 0,
                max_matches: Some(5),
                cursor: None,
            },
        )
        .await
        .unwrap();
    eprintln!(
        "grep in {:?}: {} matches",
        started.elapsed(),
        grep.matches.len()
    );
}
