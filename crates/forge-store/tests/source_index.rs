use artisan_api::{
    ContentQuery, CreateProject, CreateThread, EngineConfig, EngineId, ItemId, ItemKind, Phase,
    ProjectId, SourceIndexQuery, Status, TextContent, ThreadId,
};
use artisan_markdown_index::LeafRecord;
use forge_store::{NewItem, Store};

#[tokio::test]
#[expect(clippy::too_many_lines, reason = "one scenario walked step by step")]
async fn twenty_mib_source_latest_leaf_reads_only_bounded_ranges() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let project = store
        .create_project(CreateProject {
            id: ProjectId::new(),
            path: "/work/source-index".into(),
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
            spawned_by: None,
        })
        .await
        .unwrap();
    let paragraph = format!("{}\n\n", "α".repeat(8192));
    let text = paragraph.repeat(1280)
        + "Latest &amp; [reference][definition].\n\n[definition]: https://example.com\n";
    assert!(text.len() > 20 * 1024 * 1024);
    let saved = store
        .upsert_item(NewItem {
            id: ItemId::new(),
            thread_id: thread.id,
            run_id: None,
            parent_id: None,
            native_id: None,
            status: Status::Done,
            kind: ItemKind::AssistantMessage {
                text: text.into(),
                phase: Phase::Final,
            },
        })
        .await
        .unwrap();
    let ItemKind::AssistantMessage {
        text: TextContent::Stored(source),
        ..
    } = saved.kind
    else {
        panic!("stored source")
    };
    let seq_before = store.head();
    let page = store
        .source_index(
            &source.id,
            SourceIndexQuery {
                format: artisan_api::SourceIndexFormat::Markdown,
                offset: None,
                limit: Some(1),
            },
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(page.leaves.len(), 1);
    assert_eq!(page.end, page.index.leaves);
    assert!(serde_json::to_vec(&page).unwrap().len() < 2048);
    let leaf = &page.leaves[0];
    let metadata = store
        .content_range(
            &page.index.metadata.id,
            ContentQuery {
                offset: leaf.metadata_start,
                limit: None,
            },
        )
        .await
        .unwrap()
        .unwrap();
    let record: LeafRecord = serde_json::from_str(
        metadata
            .text
            .get(
                usize::try_from(leaf.metadata_start - metadata.start).unwrap()
                    ..usize::try_from(leaf.metadata_end - metadata.start).unwrap(),
            )
            .expect("the leaf's metadata lies on character boundaries"),
    )
    .unwrap();
    let raw = store
        .content_range(
            &source.id,
            ContentQuery {
                offset: leaf.source_start,
                limit: None,
            },
        )
        .await
        .unwrap()
        .unwrap();
    let events = record
        .replay(usize::try_from(raw.start).unwrap(), &raw.text)
        .unwrap();
    assert!(events.iter().any(|(event,_)|matches!(event,artisan_markdown_index::Event::Start(artisan_markdown_index::Tag::Link{dest_url,..}) if dest_url.as_ref()=="https://example.com")));
    assert!(
        metadata.text.len() + raw.text.len() < 2 * 65536,
        "the latest leaf of a 20 MiB source reads two bounded ranges, not the source"
    );
    assert_eq!(
        store.head(),
        seq_before,
        "derived index must not write Item revisions or events"
    );
    let conn = rusqlite::Connection::open(dir.path().join("forge.sqlite3")).unwrap();
    let metadata_id = page.index.metadata.id.as_str();
    assert_eq!(
        conn.query_row(
            include_str!("source_index/count_dependency.sql"),
            [source.id.as_str(), metadata_id],
            |row| row.get::<_, u64>(0)
        )
        .unwrap(),
        1
    );
    let first_count: u64 = conn
        .query_row(include_str!("source_index/count_leaves.sql"), [], |row| {
            row.get(0)
        })
        .unwrap();
    store
        .source_index(
            &source.id,
            SourceIndexQuery {
                format: artisan_api::SourceIndexFormat::Markdown,
                offset: Some(0),
                limit: Some(4),
            },
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        conn.query_row(include_str!("source_index/count_leaves.sql"), [], |row| row
            .get::<_, u64>(0))
            .unwrap(),
        first_count,
        "cached immutable topology is not duplicated"
    );
    conn.execute(include_str!("source_index/corrupt_items.sql"), [])
        .unwrap();
    assert!(
        store
            .source_index(
                &source.id,
                SourceIndexQuery {
                    format: artisan_api::SourceIndexFormat::Markdown,
                    offset: None,
                    limit: None
                }
            )
            .await
            .unwrap()
            .is_some()
    );
    store
        .source_index(
            &source.id,
            SourceIndexQuery {
                format: artisan_api::SourceIndexFormat::Markdown,
                offset: Some(page.index.leaves + 1),
                limit: Some(1),
            },
        )
        .await
        .expect_err("an offset past the last leaf is refused");
    store
        .source_index(
            &source.id,
            SourceIndexQuery {
                format: artisan_api::SourceIndexFormat::Markdown,
                offset: None,
                limit: Some(65),
            },
        )
        .await
        .expect_err("a limit over the page cap is refused");
    conn.execute(include_str!("source_index/delete_items.sql"), [])
        .unwrap();
    conn.execute(
        include_str!("source_index/delete_content.sql"),
        [source.id.as_str()],
    )
    .unwrap();
    assert_eq!(
        conn.query_row(include_str!("source_index/count_leaves.sql"), [], |row| row
            .get::<_, u64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        conn.query_row(include_str!("source_index/count_indexes.sql"), [], |row| {
            row.get::<_, u64>(0)
        })
        .unwrap(),
        0
    );
    assert!(
        conn.query_row(
            include_str!("source_index/unreferenced_at.sql"),
            [metadata_id],
            |row| row.get::<_, Option<i64>>(0)
        )
        .unwrap()
        .is_some(),
        "derived index follows source ownership and becomes a normal grace-period orphan"
    );
}

#[tokio::test]
#[expect(clippy::too_many_lines, reason = "one scenario walked step by step")]
async fn same_source_literal_and_markdown_have_distinct_bounded_indexes() {
    use artisan_api::SourceIndexFormat;
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let project = store
        .create_project(CreateProject {
            id: ProjectId::new(),
            path: "/work/literal-index".into(),
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
            spawned_by: None,
        })
        .await
        .unwrap();
    let text = "\n**α** &amp; [x][ref]\r\n\n".repeat(1024) + "[ref]: https://example.com\n";
    let item = store
        .upsert_item(NewItem {
            id: ItemId::new(),
            thread_id: thread.id,
            run_id: None,
            parent_id: None,
            native_id: None,
            status: Status::Done,
            kind: ItemKind::AssistantMessage {
                text: text.clone().into(),
                phase: Phase::Final,
            },
        })
        .await
        .unwrap();
    let ItemKind::AssistantMessage {
        text: TextContent::Stored(source),
        ..
    } = item.kind
    else {
        panic!("large authored source must externalize")
    };
    let literal = store
        .source_index(
            &source.id,
            SourceIndexQuery {
                format: SourceIndexFormat::Literal,
                offset: None,
                limit: Some(64),
            },
        )
        .await
        .unwrap()
        .unwrap();
    let markdown = store
        .source_index(
            &source.id,
            SourceIndexQuery {
                format: SourceIndexFormat::Markdown,
                offset: None,
                limit: Some(64),
            },
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(literal.index.source, markdown.index.source);
    assert_eq!(literal.index.format, SourceIndexFormat::Literal);
    assert_eq!(markdown.index.format, SourceIndexFormat::Markdown);
    assert_ne!(literal.index.metadata.id, markdown.index.metadata.id);
    assert_eq!(
        literal.index.leaves,
        text.bytes().filter(|b| *b == b'\n').count() as u64 + 1
    );
    assert_eq!(literal.leaves.len(), 64);
    assert_eq!(literal.end, literal.index.leaves);
    assert!(literal.index.leaves > markdown.index.leaves);
    let last = literal.leaves.last().unwrap();
    assert_eq!(last.source_start, source.bytes);
    assert_eq!(last.source_end, source.bytes);
    let first = store
        .source_index(
            &source.id,
            SourceIndexQuery {
                format: SourceIndexFormat::Literal,
                offset: Some(0),
                limit: Some(3),
            },
        )
        .await
        .unwrap()
        .unwrap();
    for leaf in first.leaves {
        let metadata = store
            .content_range(
                &first.index.metadata.id,
                ContentQuery {
                    offset: leaf.metadata_start,
                    limit: Some(u32::try_from(leaf.metadata_end - leaf.metadata_start).unwrap()),
                },
            )
            .await
            .unwrap()
            .unwrap();
        let record: LeafRecord = serde_json::from_str(&metadata.text).unwrap();
        let raw = store
            .content_range(
                &source.id,
                ContentQuery {
                    offset: leaf.source_start,
                    limit: Some(
                        u32::try_from((leaf.source_end - leaf.source_start).max(4)).unwrap(),
                    ),
                },
            )
            .await
            .unwrap()
            .unwrap();
        let events = record
            .replay(usize::try_from(leaf.source_start).unwrap(), &raw.text)
            .unwrap();
        let artisan_markdown_index::Event::Text(value) = &events[0].0 else {
            panic!("raw literal event")
        };
        assert_eq!(
            Some(value.as_ref()),
            text.get(
                usize::try_from(leaf.source_start).unwrap()
                    ..usize::try_from(leaf.source_end).unwrap()
            )
        );
    }
    let conn = rusqlite::Connection::open(dir.path().join("forge.sqlite3")).unwrap();
    let count: u64 = conn
        .query_row(
            include_str!("source_index/count_source_indexes.sql"),
            [source.id.as_str()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn other_writes_go_through_while_a_large_source_is_indexed() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let project = store
        .create_project(CreateProject {
            id: ProjectId::new(),
            path: "/work/staged-index".into(),
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
            spawned_by: None,
        })
        .await
        .unwrap();
    let text = "x\n".repeat(100_000);
    let saved = store
        .upsert_item(NewItem {
            id: ItemId::new(),
            thread_id: thread.id.clone(),
            run_id: None,
            parent_id: None,
            native_id: None,
            status: Status::Done,
            kind: ItemKind::AssistantMessage {
                text: text.into(),
                phase: Phase::Final,
            },
        })
        .await
        .unwrap();
    let ItemKind::AssistantMessage {
        text: TextContent::Stored(source),
        ..
    } = saved.kind
    else {
        panic!("stored source")
    };
    let indexing = tokio::spawn({
        let store = store.clone();
        let id = source.id.clone();
        async move {
            store
                .source_index(
                    &id,
                    SourceIndexQuery {
                        format: artisan_api::SourceIndexFormat::Literal,
                        offset: None,
                        limit: Some(1),
                    },
                )
                .await
        }
    });
    let conn = rusqlite::Connection::open(dir.path().join("forge.sqlite3")).unwrap();
    let staged_unpublished = || {
        let staged: i64 = conn
            .query_row(include_str!("source_index/count_leaves.sql"), [], |row| {
                row.get(0)
            })
            .unwrap();
        let published: i64 = conn
            .query_row(
                include_str!("source_index/count_source_indexes.sql"),
                [source.id.as_str()],
                |row| row.get(0),
            )
            .unwrap();
        staged > 0 && published == 0
    };
    test_support::wait::wait_until(
        "a committed part of the index before it is published",
        std::time::Duration::from_secs(60),
        staged_unpublished,
    );
    store
        .upsert_item(NewItem {
            id: ItemId::new(),
            thread_id: thread.id,
            run_id: None,
            parent_id: None,
            native_id: None,
            status: Status::Done,
            kind: ItemKind::AssistantMessage {
                text: "written while indexing".into(),
                phase: Phase::Final,
            },
        })
        .await
        .unwrap();
    let page = indexing.await.unwrap().unwrap().unwrap();
    assert_eq!(page.index.leaves, 100_001);
    assert_eq!(page.leaves.len(), 1);
    assert_eq!(page.leaves[0].ordinal, 100_000);
}
