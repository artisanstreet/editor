#![cfg(test)]

use artisan_api::{
    BlobHash, CreateProject, CreateThread, EngineConfig, EngineId, Event, ItemId, ItemKind,
    ProjectId, Run, RunId, RunStatus, Status, ThreadId, Usage, now,
};
use forge_store::{NewItem, Store};
use std::collections::HashSet;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("forge-retention-test-{}", ItemId::new()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

async fn add_thread(store: &Store, project: &ProjectId, parent: Option<&ThreadId>) -> ThreadId {
    store
        .create_thread(CreateThread {
            id: ThreadId::new(),
            project_id: project.clone(),
            config: EngineConfig::new(EngineId::Claude),
            title: None,
            parent_thread_id: parent.cloned(),
            spawned_by: None,
        })
        .await
        .unwrap()
        .id
}

fn message(thread: &ThreadId, status: Status, images: Vec<BlobHash>) -> NewItem {
    NewItem {
        id: ItemId::new(),
        thread_id: thread.clone(),
        run_id: None,
        parent_id: None,
        native_id: None,
        status,
        kind: ItemKind::UserMessage {
            text: "look at this".into(),
            images,
            steer: false,
            from_agent: None,
            sender: None,
        },
    }
}

fn run(thread: &ThreadId, status: RunStatus) -> Run {
    Run {
        config: None,
        id: RunId::new(),
        thread_id: thread.clone(),
        engine: EngineId::Claude,
        model: None,
        input_cutoff: 0,
        status,
        usage: Usage::default(),
        session_id: None,
        error: None,
        seq: 0,
        started_at: now(),
        ended_at: None,
        continuation: None,
    }
}

async fn open() -> (TempDir, Store, ProjectId) {
    let dir = TempDir::new();
    let store = Store::open(&dir.0).unwrap();
    let project = store
        .create_project(CreateProject {
            id: ProjectId::new(),
            path: "/work/app".into(),
            name: None,
        })
        .await
        .unwrap()
        .id;
    (dir, store, project)
}

#[tokio::test]
async fn idle_threads_go_with_their_images_and_busy_families_stay() {
    let (_dir, store, project) = open().await;
    let image = webp::Encoder::from_rgb(&[7, 7, 7], 1, 1)
        .encode_lossless()
        .to_vec();
    let blob = store.put_blob(image).await.unwrap().hash;

    let idle = add_thread(&store, &project, None).await;
    store
        .upsert_item(message(&idle, Status::Done, vec![blob.clone()]))
        .await
        .unwrap();
    store.upsert_run(run(&idle, RunStatus::Done)).await.unwrap();

    let asking = add_thread(&store, &project, None).await;
    store
        .upsert_item(message(&asking, Status::AwaitingUser, Vec::new()))
        .await
        .unwrap();

    let streaming = add_thread(&store, &project, None).await;

    let handed_off = add_thread(&store, &project, None).await;
    let continued = add_thread(&store, &project, Some(&handed_off)).await;
    store
        .upsert_run(run(&continued, RunStatus::Running))
        .await
        .unwrap();

    let head = store.changes_since(0, 10_000).await.unwrap().unwrap().len();
    let removed = store
        .remove_inactive_threads(now() + 60_000, HashSet::from([streaming.clone()]), 8)
        .await
        .unwrap();
    let removed: Vec<ThreadId> = removed.into_iter().map(|thread| thread.id).collect();
    assert_eq!(removed, vec![idle.clone()]);

    for kept in [&asking, &streaming, &handed_off, &continued] {
        assert!(store.thread(kept).await.unwrap().is_some());
    }
    assert!(store.thread(&idle).await.unwrap().is_none());
    assert!(store.latest_run(&idle).await.unwrap().is_none());

    let changes = store.changes_since(0, 10_000).await.unwrap().unwrap();
    assert!(
        changes.len() < head,
        "the thread's items and runs left the feed"
    );
    assert!(changes.iter().all(|envelope| match &envelope.event {
        Event::Thread { thread } => thread.id != idle,
        Event::Item { item } => item.thread_id != idle,
        Event::Run { run } => run.thread_id != idle,
        Event::Hello { .. }
        | Event::Project { .. }
        | Event::ThreadRemoved { .. }
        | Event::ItemState { .. }
        | Event::Setting { .. }
        | Event::Live { .. }
        | Event::Suggestion { .. }
        | Event::Engine { .. }
        | Event::EngineInstall { .. }
        | Event::EditorCall { .. }
        | Event::Reset => true,
    }));
    assert_eq!(
        changes.last().map(|envelope| &envelope.event),
        Some(&Event::ThreadRemoved {
            thread_id: idle.clone()
        })
    );

    let path = store.blob_path(&blob).unwrap();
    std::fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(SystemTime::now() - Duration::from_hours(2))
        .unwrap();
    assert_eq!(store.gc_blobs().await.unwrap(), 1);
    assert_eq!(store.blob_path(&blob), None);
}

#[tokio::test]
async fn recent_threads_stay_and_a_removal_survives_a_restart() {
    let dir = TempDir::new();
    let store = Store::open(&dir.0).unwrap();
    let project = store
        .create_project(CreateProject {
            id: ProjectId::new(),
            path: "/work/app".into(),
            name: None,
        })
        .await
        .unwrap()
        .id;
    let thread = add_thread(&store, &project, None).await;

    let kept = store
        .remove_inactive_threads(now() - 60_000, HashSet::new(), 8)
        .await
        .unwrap();
    assert!(kept.is_empty());

    store
        .remove_inactive_threads(now() + 60_000, HashSet::new(), 8)
        .await
        .unwrap();
    let before = store.changes_since(0, 10_000).await.unwrap().unwrap();
    drop(store);

    let store = Store::open(&dir.0).unwrap();
    let after = store.changes_since(0, 10_000).await.unwrap().unwrap();
    assert_eq!(before, after);
    let next = add_thread(&store, &project, None).await;
    let created = store.thread(&next).await.unwrap().unwrap();
    assert!(
        after
            .iter()
            .all(|envelope| envelope.seq < Some(created.seq)),
        "a restarted Forge never reuses the removal's seq"
    );
    assert!(store.thread(&thread).await.unwrap().is_none());
}
