use artisan_api::{
    Collection, CreateProject, CreateThread, EngineConfig, EngineId, FileChange, FileOp, ItemId,
    ItemKind, Output, Phase, ProjectId, Run, RunId, RunState, Status, ThreadId, ToolKind,
    UpdateThread, Usage, now,
};
use forge_store::{GrepFailure, GrepField, GrepQuery, NewItem, RecallScope, Store};

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
    let dir = tempfile::tempdir().expect("create the test's temporary directory");
    let store = Store::open(dir.path()).unwrap();
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
        .recall_search(
            RecallScope::default(),
            "scene bound".into(),
            false,
            10,
            None,
        )
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
        .recall_search(codex_only, "scene bound".into(), false, 10, None)
        .await
        .unwrap();
    assert!(none.hits.is_empty());

    let quiet = store
        .recall_search(RecallScope::default(), "41 passed".into(), false, 10, None)
        .await
        .unwrap();
    assert!(quiet.hits.is_empty());
    let loud = store
        .recall_search(RecallScope::default(), "41 passed".into(), true, 10, None)
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
    let dir = tempfile::tempdir().expect("create the test's temporary directory");
    let store = Store::open(dir.path()).unwrap();
    let thread = thread(&store, EngineId::Claude, "Drafts").await;
    let mut draft = said(&thread, "first wording about pelicans");
    store.upsert_item(draft.clone()).await.unwrap();
    draft.kind = ItemKind::AssistantMessage {
        text: "second wording about flamingos".into(),
        phase: Phase::Final,
    };
    store.upsert_item(draft).await.unwrap();
    let search =
        |query: &str| store.recall_search(RecallScope::default(), query.into(), false, 10, None);
    assert!(search("pelicans").await.unwrap().hits.is_empty());
    assert_eq!(search("flamingos").await.unwrap().hits.len(), 1);
}

#[tokio::test]
async fn a_pattern_that_does_not_compile_says_so() {
    let dir = tempfile::tempdir().expect("create the test's temporary directory");
    let store = Store::open(dir.path()).unwrap();
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
    assert!(matches!(*error.cause, GrepFailure::Pattern(_)), "{error:#}");
}

fn grep(pattern: &str, output: bool, max: u32) -> GrepQuery {
    GrepQuery {
        pattern: pattern.into(),
        ignore_case: false,
        output,
        context: 0,
        max_matches: Some(max),
        cursor: None,
    }
}

fn shell(thread: &ThreadId, output: &str) -> NewItem {
    item(
        thread,
        Status::Done,
        ItemKind::Tool {
            tool: ToolKind::Shell {
                command: "cargo build".into(),
                cwd: None,
                background: false,
                watch: false,
            },
            title: None,
            output: Some(Output::from_text(output)),
            exit_code: Some(0),
            approval: None,
        },
    )
}

#[tokio::test]
async fn grep_pages_through_every_match_inside_one_item() {
    let dir = tempfile::tempdir().expect("create the test's temporary directory");
    let store = Store::open(dir.path()).unwrap();
    let thread = thread(&store, EngineId::Claude, "Warnings").await;
    store
        .upsert_item(said(
            &thread,
            "warning one\nfine\nwarning two\nwarning three\nwarning four\nwarning five",
        ))
        .await
        .unwrap();
    store
        .upsert_item(shell(&thread, "warning six\nwarning seven"))
        .await
        .unwrap();
    let mut query = grep("warning", true, 2);
    let mut found = Vec::new();
    let mut pages = 0;
    loop {
        let page = store
            .recall_grep(RecallScope::default(), query.clone())
            .await
            .unwrap();
        pages += 1;
        found.extend(
            page.matches
                .into_iter()
                .map(|found| (found.field, found.text)),
        );
        match page.next {
            Some(next) => query.cursor = Some(next),
            None => break,
        }
        assert!(pages < 10, "grep never finished: {found:?}");
    }
    let texts: Vec<&str> = found.iter().map(|(_, text)| text.as_str()).collect();
    assert_eq!(
        texts,
        [
            "warning six",
            "warning seven",
            "warning one",
            "warning two",
            "warning three",
            "warning four",
            "warning five"
        ]
    );
    assert_eq!(found[0].0, GrepField::Output);
    assert_eq!(found[2].0, GrepField::Text);
}

#[tokio::test]
async fn grep_shows_a_match_late_in_a_long_line() {
    let dir = tempfile::tempdir().expect("create the test's temporary directory");
    let store = Store::open(dir.path()).unwrap();
    let thread = thread(&store, EngineId::Claude, "Minified").await;
    let line = format!("{}needle{}", "x".repeat(2_000), "y".repeat(2_000));
    store.upsert_item(said(&thread, &line)).await.unwrap();
    let page = store
        .recall_grep(RecallScope::default(), grep("needle", false, 5))
        .await
        .unwrap();
    let text = &page.matches[0].text;
    assert!(text.contains("needle"), "{text}");
    assert!(text.starts_with('…') && text.ends_with('…'), "{text}");
}

#[tokio::test]
async fn output_search_continues_past_its_first_page() {
    let dir = tempfile::tempdir().expect("create the test's temporary directory");
    let store = Store::open(dir.path()).unwrap();
    let thread = thread(&store, EngineId::Claude, "Builds").await;
    for build in 0..5 {
        store
            .upsert_item(shell(&thread, &format!("build {build}: linker crashed")))
            .await
            .unwrap();
    }
    let mut cursor = None;
    let mut seen = Vec::new();
    for _ in 0..10 {
        let page = store
            .recall_search(
                RecallScope::default(),
                "linker crashed".into(),
                true,
                2,
                cursor,
            )
            .await
            .unwrap();
        seen.extend(page.hits.into_iter().map(|hit| hit.item_id));
        cursor = page.next;
        if cursor.is_none() {
            break;
        }
    }
    assert!(cursor.is_none(), "search never finished");
    let distinct: std::collections::HashSet<_> = seen.iter().collect();
    assert_eq!(seen.len(), 5, "{seen:?}");
    assert_eq!(distinct.len(), 5, "{seen:?}");
}

#[tokio::test]
async fn search_filters_items_by_the_engine_that_wrote_them() {
    let dir = tempfile::tempdir().expect("create the test's temporary directory");
    let store = Store::open(dir.path()).unwrap();
    let thread = thread(&store, EngineId::Claude, "Switched").await;
    let run = store
        .begin_run(Run {
            config: None,
            id: RunId::new(),
            thread_id: thread.clone(),
            engine: EngineId::Claude,
            model: None,
            input_cutoff: 0,
            state: RunState::Running,
            usage: Usage::default(),
            session_id: None,
            seq: 0,
            started_at: now(),
            continuation: None,
        })
        .await
        .unwrap();
    let mut reply = said(&thread, "the pelican migration is done");
    reply.run_id = Some(run.id);
    store.upsert_item(reply).await.unwrap();
    store
        .update_thread(
            &thread,
            UpdateThread {
                title: None,
                archived: None,
                config: Some(EngineConfig::new(EngineId::Codex)),
                read_seq: None,
            },
        )
        .await
        .unwrap();
    let on = |engine| RecallScope {
        engine: Some(engine),
        ..RecallScope::default()
    };
    let search =
        |engine| store.recall_search(on(engine), "pelican migration".into(), false, 10, None);
    assert_eq!(search(EngineId::Claude).await.unwrap().hits.len(), 1);
    assert!(search(EngineId::Codex).await.unwrap().hits.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn forge_starts_with_a_damaged_item_and_still_finds_the_rest() {
    let dir = tempfile::tempdir().expect("create the test's temporary directory");
    let store = Store::open(dir.path()).expect("open the store");
    let notes = thread(&store, EngineId::Claude, "Notes").await;
    store
        .upsert_item(said(&notes, "The scene bound stops at 512 items."))
        .await
        .expect("save the healthy note");
    let damaged = store
        .upsert_item(said(&notes, "This note loses its text."))
        .await
        .expect("save the note that gets damaged")
        .id;
    drop(store);
    let raw = rusqlite::Connection::open(dir.path().join("forge.sqlite3"))
        .expect("open the database beside the store");
    let lost = serde_json::to_string(&artisan_api::ContentRef::from_text("words never stored"))
        .expect("encode a reference to missing content");
    raw.execute(
        include_str!("recall/damage_item.sql"),
        [damaged.as_str(), lost.as_str()],
    )
    .expect("point the note at content that is not stored");
    raw.execute_batch(include_str!("recall/forget_index.sql"))
        .expect("make the next start rebuild the recall index");
    drop(raw);

    let store = Store::open(dir.path()).expect("Forge starts despite the damaged item");
    let mut index = store.recall_index();
    let built = *index
        .wait_for(|index| *index != forge_store::RecallIndex::Building)
        .await
        .expect("the recall index build reports how it ended");
    assert_eq!(built, forge_store::RecallIndex::Built);
    let found = store
        .recall_search(
            RecallScope::default(),
            "scene bound".into(),
            false,
            10,
            None,
        )
        .await
        .expect("search recall");
    assert_eq!(found.hits.len(), 1, "{found:?}");
}

const UNLIMITED: usize = usize::MAX;

#[expect(
    clippy::wildcard_enum_match_arm,
    reason = "every other statement is authorised as usual"
)]
fn store_counting_advances(
    dir: &std::path::Path,
    advances: &std::sync::Arc<std::sync::atomic::AtomicUsize>,
    refuse_index_tables: bool,
) -> Result<(Store, forge_store::DataLock), forge_store::OpenStoreError> {
    use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
    use std::sync::atomic::Ordering;
    let budget = std::sync::Arc::clone(advances);
    let connections = forge_store::Connections::new(move |path| {
        let mut conn = rusqlite::Connection::open(path)?;
        let budget = std::sync::Arc::clone(&budget);
        conn.authorizer(Some(move |context: AuthContext<'_>| match context.action {
            AuthAction::CreateTable {
                table_name: "item_text",
            } if refuse_index_tables => Authorization::Deny,
            AuthAction::Update {
                table_name: "recall_index",
                column_name: "building_after",
            } => {
                let spent = budget.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                    (left != UNLIMITED).then(|| left.saturating_sub(1))
                });
                match spent {
                    Ok(0) => Authorization::Deny,
                    Ok(_) | Err(_) => Authorization::Allow,
                }
            }
            _ => Authorization::Allow,
        }))?;
        Ok(conn)
    });
    let lock = forge_store::DataLock::acquire(dir).expect("hold the data folder");
    let store = Store::open_restoring_in(
        dir,
        &lock,
        forge_store::Environment {
            connections,
            ..forge_store::Environment::system()
        },
    )?;
    Ok((store, lock))
}

fn forget_recall_index(dir: &std::path::Path) {
    rusqlite::Connection::open(dir.join("forge.sqlite3"))
        .expect("open the database beside the store")
        .execute_batch(include_str!("recall/forget_index.sql"))
        .expect("make the next start rebuild the recall index");
}

fn ledger_entry(number: usize) -> String {
    format!("ledger entry {number:04} closes")
}

async fn found(store: &Store, text: &str) -> Vec<ItemId> {
    store
        .recall_search(RecallScope::default(), text.into(), false, 100, None)
        .await
        .expect("search recall")
        .hits
        .into_iter()
        .map(|hit| hit.item_id)
        .collect()
}

async fn index_built(store: &Store) -> forge_store::RecallIndex {
    let mut index = store.recall_index();
    *tokio::time::timeout(
        test_support::wait::LIVENESS,
        index.wait_for(|index| *index != forge_store::RecallIndex::Building),
    )
    .await
    .expect("the recall index build ends in time")
    .expect("the recall index build reports how it ended")
}

#[tokio::test(flavor = "multi_thread")]
async fn a_forge_that_cannot_prepare_its_search_index_refuses_to_start_instead_of_failing_every_message()
 {
    let dir = tempfile::tempdir().expect("create the test's temporary directory");
    let store = Store::open(dir.path()).expect("open the store");
    let notes = thread(&store, EngineId::Claude, "Notes").await;
    store
        .upsert_item(said(&notes, "the heron count is due on Friday"))
        .await
        .expect("save a note");
    drop(store);
    forget_recall_index(dir.path());
    let advances = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(UNLIMITED));

    let refused = store_counting_advances(dir.path(), &advances, true)
        .map(drop)
        .expect_err("Forge must not serve with a search index it couldn't prepare");

    assert!(
        matches!(refused, forge_store::OpenStoreError::PrepareRecall { .. }),
        "{}",
        diagnostics::chain(&refused)
    );
    let (store, _lock) =
        store_counting_advances(dir.path(), &advances, false).expect("the next start prepares it");
    store
        .upsert_item(said(&notes, "the heron count moved to Monday"))
        .await
        .expect("messages save once the index is prepared");
    assert_eq!(index_built(&store).await, forge_store::RecallIndex::Built);
    assert_eq!(found(&store, "heron count").await.len(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_recall_index_batch_that_fails_is_retried_without_restarting_forge() {
    let dir = tempfile::tempdir().expect("create the test's temporary directory");
    let store = Store::open(dir.path()).expect("open the store");
    let notes = thread(&store, EngineId::Claude, "Notes").await;
    let note = store
        .upsert_item(said(&notes, "the osprey nest has two eggs"))
        .await
        .expect("save a note")
        .id;
    drop(store);
    forget_recall_index(dir.path());
    let advances = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));

    let (store, _lock) = store_counting_advances(dir.path(), &advances, false)
        .expect("Forge starts and rebuilds the index in the background");
    advances.store(UNLIMITED, std::sync::atomic::Ordering::SeqCst);

    assert_eq!(index_built(&store).await, forge_store::RecallIndex::Built);
    assert_eq!(found(&store, "osprey nest").await, vec![note]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_recall_build_resumed_after_a_restart_finds_every_message_once_despite_writes_and_removals_during_it()
 {
    use std::collections::HashSet;
    const ENTRIES: usize = 600;
    let dir = tempfile::tempdir().expect("create the test's temporary directory");
    let store = Store::open(dir.path()).expect("open the store");
    let kept = thread(&store, EngineId::Claude, "Kept").await;
    let removed = thread(&store, EngineId::Claude, "Removed").await;
    let mut entries = Vec::with_capacity(ENTRIES);
    for number in 0..ENTRIES {
        let owner = if number % 3 == 0 { &removed } else { &kept };
        let saved = store
            .upsert_item(said(owner, &ledger_entry(number)))
            .await
            .expect("save a ledger entry");
        entries.push(saved.id);
    }
    drop(store);
    forget_recall_index(dir.path());
    let advances = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(1));
    let (store, lock) = store_counting_advances(dir.path(), &advances, false)
        .expect("Forge starts and rebuilds the index in the background");
    let partly = tokio::time::timeout(test_support::wait::LIVENESS, async {
        loop {
            if found(&store, &ledger_entry(0)).await.len() == 1 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await;
    partly.expect("the first batch is indexed");
    assert!(
        found(&store, &ledger_entry(ENTRIES - 1)).await.is_empty(),
        "the build stops after its first batch"
    );
    drop(store);
    drop(lock);

    let advances = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (store, _lock) = store_counting_advances(dir.path(), &advances, false)
        .expect("the next start resumes the build");
    let late = store
        .upsert_item(said(&kept, &ledger_entry(ENTRIES)))
        .await
        .expect("save an entry while the index builds")
        .id;
    let edited = ENTRIES - 2;
    let rewritten = store
        .upsert_item(NewItem {
            id: entries[edited].clone(),
            ..said(&kept, "ledger entry rewritten after the audit")
        })
        .await
        .expect("edit an entry the build hasn't reached")
        .id;
    let gone = store
        .remove_inactive_threads(now() + 60_000, HashSet::from([kept.clone()]), usize::MAX)
        .await
        .expect("remove a thread while the index builds");
    assert_eq!(gone.len(), 1);
    advances.store(UNLIMITED, std::sync::atomic::Ordering::SeqCst);

    assert_eq!(index_built(&store).await, forge_store::RecallIndex::Built);
    for (number, entry) in entries.iter().enumerate() {
        let hits = found(&store, &ledger_entry(number)).await;
        let expected = if number % 3 == 0 || number == edited {
            Vec::new()
        } else {
            vec![entry.clone()]
        };
        assert_eq!(hits, expected, "entry {number}");
    }
    assert_eq!(found(&store, &ledger_entry(ENTRIES)).await, vec![late]);
    assert_eq!(
        found(&store, "rewritten after the audit").await,
        vec![rewritten]
    );
}
