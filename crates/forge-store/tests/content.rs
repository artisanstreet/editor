use artisan_api::{
    ContentQuery, CreateProject, CreateThread, EngineConfig, EngineId, Event, ItemId, ItemKind,
    ItemsQuery, Phase, ProjectId, Status, TextContent, ThreadId,
};
use forge_store::{NewItem, Store};

#[tokio::test]
async fn large_item_emits_once_reuses_source_and_ranges_ignore_item_json() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let project = store
        .create_project(CreateProject {
            id: ProjectId::new(),
            path: "/work/content".into(),
            name: None,
        })
        .await
        .unwrap();
    let thread = store
        .create_thread(CreateThread {
            id: ThreadId::new(),
            project_id: project.id,
            config: EngineConfig::new(EngineId::Codex),
            title: None,
            parent_thread_id: None,
        })
        .await
        .unwrap();
    let text = "α🙂\n".repeat(3_000_000);
    assert!(text.len() > 20 * 1024 * 1024);
    let mut events = store.subscribe();
    let input = NewItem {
        id: ItemId::new(),
        thread_id: thread.id.clone(),
        run_id: None,
        parent_id: None,
        native_id: Some("native".into()),
        status: Status::Running,
        kind: ItemKind::AssistantMessage {
            text: text.clone().into(),
            phase: Phase::Final,
        },
    };
    let first = store.upsert_item(input.clone()).await.unwrap();
    let ItemKind::AssistantMessage {
        text: TextContent::Stored(source),
        ..
    } = &first.kind
    else {
        panic!("large text wasn't externalized")
    };
    let source = source.clone();
    let event = events.try_recv().unwrap();
    assert!(matches!(&event.event, Event::Item { .. }));
    assert!(serde_json::to_vec(&event).unwrap().len() < 2048);
    assert!(
        events.try_recv().is_err(),
        "one Item write must emit one Item event"
    );
    assert_eq!(store.upsert_item(input.clone()).await.unwrap(), first);
    assert!(
        events.try_recv().is_err(),
        "normalized replay must not save again"
    );
    let mut settled = input;
    settled.status = Status::Done;
    let last = store.upsert_item(settled).await.unwrap();
    assert!(last.seq > first.seq);
    let ItemKind::AssistantMessage {
        text: TextContent::Stored(final_source),
        ..
    } = &last.kind
    else {
        panic!("status change lost descriptor")
    };
    assert_eq!(final_source, &source);
    let page = store
        .items_page(
            &thread.id,
            ItemsQuery {
                before: None,
                after: None,
                limit: None,
            },
        )
        .await
        .unwrap();
    assert!(serde_json::to_vec(&page).unwrap().len() < 4096);
    let conn = rusqlite::Connection::open(dir.path().join("forge.sqlite3")).unwrap();
    assert_eq!(
        conn.query_row("SELECT count(*) FROM content", [], |row| row
            .get::<_, u64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        conn.query_row("SELECT count(*) FROM item_content", [], |row| row
            .get::<_, u64>(0))
            .unwrap(),
        1
    );
    // Source reads depend only on the indexed chunk table, not Item decoding.
    conn.execute(
        "UPDATE items SET body='{\"invalid_item_schema\":true}' WHERE id=?1",
        [last.id.as_str()],
    )
    .unwrap();
    let mut reconstructed = String::new();
    let mut offset = 0;
    while offset < source.bytes {
        let range = store
            .content_range(
                &source.id,
                ContentQuery {
                    offset,
                    limit: None,
                },
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(range.start, offset);
        assert!(range.text.len() <= 65536);
        offset = range.end;
        reconstructed.push_str(&range.text);
    }
    assert_eq!(reconstructed, text);
    assert_eq!(artisan_api::ContentRef::from_text(&reconstructed), source);
}

#[tokio::test]
async fn version_twelve_migration_preserves_revisions_native_identity_and_complete_output() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let project = store
        .create_project(CreateProject {
            id: ProjectId::new(),
            path: "/work/migrate".into(),
            name: None,
        })
        .await
        .unwrap();
    let thread = store
        .create_thread(CreateThread {
            id: ThreadId::new(),
            project_id: project.id,
            config: EngineConfig::new(EngineId::Codex),
            title: None,
            parent_thread_id: None,
        })
        .await
        .unwrap();
    let item = store
        .upsert_item(NewItem {
            id: ItemId::new(),
            thread_id: thread.id,
            run_id: None,
            parent_id: None,
            native_id: Some("preserved-native-id".into()),
            status: Status::Done,
            kind: ItemKind::Tool {
                tool: artisan_api::ToolKind::Shell {
                    command: "printf source".into(),
                    cwd: None,
                    background: false,
                },
                title: None,
                output: Some(artisan_api::Output::from_text("placeholder")),
                exit_code: Some(0),
                approval: None,
            },
        })
        .await
        .unwrap();
    let expected = "παλιό🙂\n".repeat(3000) + "exact tail";
    let mut legacy = serde_json::to_value(&item).unwrap();
    legacy["kind"]["output"] = serde_json::json!({ "head": &expected[..expected.len()-10], "tail": "exact tail", "truncated_bytes": 0 });
    drop(store);
    let conn = rusqlite::Connection::open(dir.path().join("forge.sqlite3")).unwrap();
    conn.execute(
        "UPDATE items SET body=?2 WHERE id=?1",
        rusqlite::params![item.id.as_str(), serde_json::to_string(&legacy).unwrap()],
    )
    .unwrap();
    conn.execute_batch(
        "DROP TRIGGER item_content_owner_delete;
         DROP INDEX items_pending_input;
         DROP INDEX items_pending_inputs;
         DROP INDEX items_thread_kind_position;
         DROP INDEX items_thread_parent_position;
         DROP INDEX items_agent_message_target;
         ALTER TABLE items DROP COLUMN kind_type;
         ALTER TABLE items DROP COLUMN parent_id;
         DROP TABLE source_index_leaves;
         DROP TABLE source_indexes;
         DROP TABLE content_records;
         DROP TABLE content_collections;
         DROP TABLE content_dependencies;
         DROP TABLE item_content;
         DROP TABLE content_chunks;
         DROP TABLE content;
         PRAGMA user_version=12;",
    )
    .unwrap();
    drop(conn);
    let migrated = Store::open(dir.path()).unwrap();
    let actual = migrated.item(&item.id).await.unwrap().unwrap();
    assert_eq!(actual.seq, item.seq);
    assert_eq!(actual.position, item.position);
    assert_eq!(actual.status, item.status);
    assert_eq!(actual.created_at, item.created_at);
    assert_eq!(actual.updated_at, item.updated_at);
    assert_eq!(
        migrated.native_id(&item.id).await.unwrap().as_deref(),
        Some("preserved-native-id")
    );
    let ItemKind::Tool {
        output: Some(output),
        ..
    } = &actual.kind
    else {
        panic!("migration lost tool output")
    };
    assert_eq!(output.omitted_bytes, 0);
    assert!(matches!(output.text, TextContent::Stored(_)));
    assert_eq!(migrated.resolve_text(&output.text).await.unwrap(), expected);
    let events = migrated.subscribe();
    let reloaded = migrated
        .upsert_item(NewItem {
            id: actual.id.clone(),
            thread_id: actual.thread_id.clone(),
            run_id: actual.run_id.clone(),
            parent_id: actual.parent_id.clone(),
            native_id: Some("preserved-native-id".into()),
            status: actual.status,
            kind: actual.kind.clone(),
        })
        .await
        .unwrap();
    assert_eq!(reloaded, actual);
    assert!(events.is_empty());
}

#[tokio::test]
async fn goal_summary_reads_a_bounded_prefix_and_preserves_full_source_identity() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let project = store
        .create_project(CreateProject {
            id: ProjectId::new(),
            path: "/work/goal".into(),
            name: None,
        })
        .await
        .unwrap();
    let thread = store
        .create_thread(CreateThread {
            id: ThreadId::new(),
            project_id: project.id,
            config: EngineConfig::new(EngineId::Codex),
            title: None,
            parent_thread_id: None,
        })
        .await
        .unwrap();
    let objective = "🙂α finish the full objective\n".repeat(100_000);
    let item = store
        .upsert_item(NewItem {
            id: ItemId::new(),
            thread_id: thread.id.clone(),
            run_id: None,
            parent_id: None,
            native_id: None,
            status: Status::Done,
            kind: ItemKind::Notice {
                kind: artisan_api::NoticeKind::Goal {
                    state: "active".into(),
                },
                text: objective.clone().into(),
            },
        })
        .await
        .unwrap();
    let ItemKind::Notice {
        text: TextContent::Stored(source),
        ..
    } = &item.kind
    else {
        panic!("large objective must be a source descriptor")
    };
    let conn = rusqlite::Connection::open(dir.path().join("forge.sqlite3")).unwrap();
    conn.execute(
        "DELETE FROM content_chunks WHERE content_id=?1 AND start>1024",
        [source.id.as_str()],
    )
    .unwrap();
    let summary = store.inspector(&thread.id).await.unwrap();
    let goal = summary.goal.unwrap();
    assert_eq!(goal.objective, TextContent::Stored(source.clone()));
    assert_eq!(
        goal.summary,
        objective.chars().take(160).collect::<String>() + "…"
    );
    assert!(goal.summary.chars().count() <= 161);
    assert!(
        store.resolve_text(&goal.objective).await.is_err(),
        "fixture must prevent accidental full-source reads"
    );
}
