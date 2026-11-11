#![cfg(test)]

use artisan_api::{
    CreateProject, CreateThread, EngineConfig, EngineId, ItemId, ItemKind, ProjectId, Status,
    ThreadId, now,
};
use forge_store::{
    NewItem, Store,
    coordination::{
        ClaimState, Content, Intent, MessageState, Query, Resource, Update, WorkState,
        normalize_path,
    },
    discovery,
};

async fn setup() -> (tempfile::TempDir, Store, Vec<ThreadId>) {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    let project = ProjectId::new();
    store
        .create_project(CreateProject {
            id: project.clone(),
            path: "/repo".into(),
            name: None,
        })
        .await
        .unwrap();
    let mut threads = Vec::new();
    for _ in 0..3 {
        let id = ThreadId::new();
        store
            .create_thread(CreateThread {
                id: id.clone(),
                project_id: project.clone(),
                config: EngineConfig::new(EngineId::Codex),
                title: None,
                parent_thread_id: None,
                spawned_by: None,
            })
            .await
            .unwrap();
        threads.push(id);
    }
    (directory, store, threads)
}

fn path(root: &str, value: &str, recursive: bool) -> Resource {
    Resource::Path {
        worktree: root.into(),
        path: value.into(),
        recursive,
    }
    .normalize()
    .unwrap()
}

fn claim() -> Content {
    Content::Claim {
        resources: vec![path("/repo", "src/a.rs", false)],
        purpose: "fix parser".into(),
        state: ClaimState::Active,
        expires_at: now() + 60_000,
    }
}

fn work(dependencies: Vec<ThreadId>) -> Content {
    Content::Work {
        objective: "fix parser".into(),
        worktree: "/repo".into(),
        state: WorkState::Running,
        resources: vec![],
        dependencies,
        summary: String::new(),
        evidence: vec![],
    }
}

#[test]
fn paths_normalize_idempotently_and_overlap_only_within_the_declared_checkout() {
    for raw in [
        "/repo/./src/../lib",
        "C:\\Repo\\src\\..\\lib",
        "C:\\",
        "//server/share/repo/./lib",
    ] {
        let normalized = normalize_path(raw).unwrap();
        assert_eq!(normalize_path(&normalized).unwrap(), normalized);
    }
    assert!(path("/repo", "src", true).overlaps(&path("/repo", "src/a.rs", false)));
    assert!(!path("/repo", "src", true).overlaps(&path("/repo", "src-other/a.rs", false)));
    assert!(!path("/repo", "src/a.rs", false).overlaps(&path("/other", "src/a.rs", false)));
    assert!(path("C:\\Repo", "src\\A.rs", false).overlaps(&path("c:/repo", "SRC/a.rs", false)));
    assert!(
        Resource::Path {
            worktree: "/repo".into(),
            path: "../other/a.rs".into(),
            recursive: false
        }
        .normalize()
        .is_err()
    );
    assert!(normalize_path("//server/share/../outside").is_err());
}

#[tokio::test]
async fn overlapping_claims_are_advisory_and_updates_preserve_ownership_and_revisions() {
    let (_directory, store, threads) = setup().await;
    let (a, b) = tokio::join!(
        store.coordination_put(threads[0].clone(), None, claim(), None),
        store.coordination_put(threads[1].clone(), None, claim(), None)
    );
    let a = a.unwrap();
    let b = b.unwrap();
    assert_ne!(a.id, b.id);
    assert!(a.active_claim(now()) && b.active_claim(now()));
    assert!(
        store
            .coordination_update(
                threads[1].clone(),
                a.id.clone(),
                a.revision,
                Update::ReleaseClaim
            )
            .await
            .is_err()
    );
    let renewed = store
        .coordination_update(
            threads[0].clone(),
            a.id.clone(),
            a.revision,
            Update::RenewClaim { seconds: 120 },
        )
        .await
        .unwrap();
    let stale = store
        .coordination_update(
            threads[0].clone(),
            a.id.clone(),
            a.revision,
            Update::ReleaseClaim,
        )
        .await
        .unwrap_err();
    assert!(
        stale
            .downcast_ref::<forge_store::coordination::Conflict>()
            .is_some()
    );
    let released = store
        .coordination_update(
            threads[0].clone(),
            a.id,
            renewed.revision,
            Update::ReleaseClaim,
        )
        .await
        .unwrap();
    assert!(!released.active_claim(now()));
    assert!(
        store
            .coordination_get(b.id.clone())
            .await
            .unwrap()
            .unwrap()
            .active_claim(now())
    );
}

#[tokio::test]
async fn work_dependencies_reject_cycles_without_overwriting_the_last_valid_graph() {
    let (_directory, store, threads) = setup().await;
    let b = store
        .coordination_put(threads[1].clone(), None, work(vec![]), None)
        .await
        .unwrap();
    store
        .coordination_put(
            threads[0].clone(),
            None,
            work(vec![threads[1].clone()]),
            None,
        )
        .await
        .unwrap();
    assert!(
        store
            .coordination_put(
                threads[1].clone(),
                None,
                work(vec![threads[0].clone()]),
                Some(b.revision)
            )
            .await
            .is_err()
    );
    assert_eq!(
        store.coordination_get(b.id.clone()).await.unwrap().unwrap(),
        b
    );
    assert!(
        store
            .coordination_put(threads[2].clone(), None, work(vec![ThreadId::new()]), None)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn requests_require_recipient_acknowledgment_and_terminal_decisions_do_not_reopen() {
    let (_directory, store, threads) = setup().await;
    let message = store
        .coordination_put(
            threads[0].clone(),
            None,
            Content::Message {
                recipient: threads[1].clone(),
                intent: Intent::Boundary,
                text: "Please leave src/a.rs to me".into(),
                state: MessageState::Prepared,
                reply: None,
                delivery_item: None,
                receipt_revision: 0,
            },
            None,
        )
        .await
        .unwrap();
    assert!(
        store
            .coordination_update(
                threads[2].clone(),
                message.id.clone(),
                message.revision,
                Update::Acknowledge {
                    state: MessageState::Acknowledged,
                    reply: None
                }
            )
            .await
            .is_err()
    );
    let accepted = store
        .coordination_update(
            threads[1].clone(),
            message.id.clone(),
            message.revision,
            Update::Acknowledge {
                state: MessageState::Acknowledged,
                reply: Some("Agreed; I own src/b.rs".into()),
            },
        )
        .await
        .unwrap();
    assert!(matches!(
        accepted.content,
        Content::Message {
            state: MessageState::Acknowledged,
            ..
        }
    ));
    let resolved = store
        .coordination_update(
            threads[1].clone(),
            accepted.id.clone(),
            accepted.revision,
            Update::Acknowledge {
                state: MessageState::Resolved,
                reply: None,
            },
        )
        .await
        .unwrap();
    assert!(
        store
            .coordination_update(
                threads[1].clone(),
                resolved.id,
                resolved.revision,
                Update::Acknowledge {
                    state: MessageState::Acknowledged,
                    reply: None
                }
            )
            .await
            .is_err()
    );
}

#[tokio::test]
async fn coordination_is_durable_and_event_cursors_do_not_lose_intervening_changes() {
    let (directory, store, threads) = setup().await;
    let snapshot = store
        .coordination_list(Query {
            limit: 30,
            ..Default::default()
        })
        .await
        .unwrap();
    let record = store
        .coordination_put(threads[0].clone(), None, claim(), None)
        .await
        .unwrap();
    let reopened = Store::open(directory.path()).unwrap();
    assert_eq!(
        reopened.coordination_get(record.id.clone()).await.unwrap(),
        Some(record.clone())
    );
    let changes = reopened
        .coordination_changes(snapshot.event_cursor, None, 30)
        .await
        .unwrap();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].record_id, record.id);
    assert!(changes[0].seq > snapshot.event_cursor);
}

#[tokio::test]
async fn discovery_filters_before_pagination_and_can_find_other_parents_children() {
    let (_directory, store, threads) = setup().await;
    let project = store.thread(&threads[0]).await.unwrap().unwrap().project_id;
    for _ in 0..205 {
        store
            .create_thread(CreateThread {
                id: ThreadId::new(),
                project_id: project.clone(),
                config: EngineConfig::new(EngineId::Codex),
                title: Some("unrelated".into()),
                parent_thread_id: None,
                spawned_by: None,
            })
            .await
            .unwrap();
    }
    let page = store
        .discover_workers(discovery::Query {
            active: true,
            include_agents: true,
            live_threads: vec![threads[0].clone()],
            limit: 1,
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(page.workers.len(), 1);
    assert_eq!(page.workers[0].thread.id, threads[0]);
    let child = ThreadId::new();
    store
        .create_thread(CreateThread {
            id: child.clone(),
            project_id: project,
            config: EngineConfig::new(EngineId::Codex),
            title: None,
            parent_thread_id: Some(threads[1].clone()),
            spawned_by: Some(artisan_api::ItemId::new()),
        })
        .await
        .unwrap();
    let page = store
        .discover_workers(discovery::Query {
            parent: Some(threads[1].clone()),
            include_agents: true,
            limit: 30,
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(page.workers.len(), 1);
    assert_eq!(page.workers[0].thread.id, child);
}

#[tokio::test]
async fn file_discovery_matches_exact_paths_before_pagination_and_resumes_without_gaps() {
    let (_directory, store, threads) = setup().await;
    for path in [
        "/repo/a.rs",
        "/repo/a.rs",
        "/repo/a.rs.backup",
        "C:\\Repo\\B.rs",
    ] {
        store
            .upsert_item(forge_store::NewItem {
                id: artisan_api::ItemId::new(),
                thread_id: threads[0].clone(),
                run_id: None,
                parent_id: None,
                native_id: None,
                status: artisan_api::Status::Done,
                kind: artisan_api::ItemKind::Tool {
                    tool: artisan_api::ToolKind::Read {
                        path: path.into(),
                        range: None,
                    },
                    title: None,
                    output: None,
                    exit_code: None,
                    approval: None,
                },
            })
            .await
            .unwrap();
    }
    let paths = vec!["/repo/a.rs".into(), "c:/repo/b.rs".into()];
    let mut cursor = None;
    let mut found = Vec::new();
    loop {
        let page = store
            .discover_file_activity(None, paths.clone(), false, cursor, 1)
            .await
            .unwrap();
        found.extend(page.activity);
        match page.next_cursor {
            Some(next) => cursor = Some(serde_json::from_str(&next).unwrap()),
            None => break,
        }
    }
    assert_eq!(found.len(), 3);
    assert_ne!(found[1].item, found[2].item);
    assert!(found.iter().all(|entry| !entry.path.ends_with(".backup")));
    let partial = store
        .discover_file_activity(None, vec!["a.rs".into()], true, None, 30)
        .await
        .unwrap();
    assert_eq!(partial.activity.len(), 3);
}

fn message(thread: &ThreadId, status: Status) -> NewItem {
    NewItem {
        id: ItemId::new(),
        thread_id: thread.clone(),
        run_id: None,
        parent_id: None,
        native_id: None,
        status,
        kind: ItemKind::UserMessage {
            text: "which one?".into(),
            images: Vec::new(),
            steer: false,
            from_agent: None,
            sender: None,
        },
    }
}

#[tokio::test]
async fn threads_waiting_on_the_user_are_found_without_walking_every_item() {
    let (directory, store, threads) = setup().await;
    for _ in 0..40 {
        store
            .upsert_item(message(&threads[0], Status::Done))
            .await
            .unwrap();
    }
    store
        .upsert_item(message(&threads[1], Status::AwaitingUser))
        .await
        .unwrap();
    let active = store
        .discover_workers(discovery::Query {
            active: true,
            limit: 30,
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(
        active
            .workers
            .iter()
            .map(|worker| (worker.thread.id.clone(), worker.awaiting_user))
            .collect::<Vec<_>>(),
        vec![(threads[1].clone(), true)],
        "only the thread with a question for the user counts as active"
    );
    let everyone = store
        .discover_workers(discovery::Query {
            limit: 30,
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(
        everyone
            .workers
            .iter()
            .filter(|worker| worker.awaiting_user)
            .map(|worker| worker.thread.id.clone())
            .collect::<Vec<_>>(),
        vec![threads[1].clone()],
        "a thread with only finished items is not waiting"
    );
    let connection = rusqlite::Connection::open_with_flags(
        directory.path().join("forge.sqlite3"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let mut explain = connection
        .prepare(&format!(
            "EXPLAIN QUERY PLAN {}",
            include_str!("../src/sql/coordination/discovery.sql")
        ))
        .unwrap();
    let nothing = rusqlite::types::Null;
    let plan = explain
        .query_map(
            rusqlite::params![
                nothing, nothing, false, true, "[]", nothing, "", nothing, nothing, nothing,
                nothing, nothing, 31
            ],
            |row| row.get::<_, String>(3),
        )
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert!(
        plan.iter()
            .any(|step| step.contains("items_awaiting_position")),
        "waiting threads come from the small waiting index: {plan:?}"
    );
    assert!(
        !plan.iter().any(|step| step.contains("items_thread")),
        "discovery must not read each thread's whole history: {plan:?}"
    );
}
