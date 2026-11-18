use artisan_api::{
    CreateProject, CreateThread, EngineConfig, EngineId, ProjectId, ThreadId, ThreadWindow,
    ThreadsQuery,
};
use forge_store::{
    CreateProjectFailure, MAX_LIVE_PROJECTS, MAX_PROJECT_NAME_BYTES, MAX_PROJECT_PATH_BYTES,
    STATE_BYTES, Store,
};
use std::collections::HashSet;

fn project(path: &str) -> CreateProject {
    CreateProject {
        id: ProjectId::new(),
        path: path.into(),
        name: None,
    }
}

#[tokio::test]
async fn the_starting_state_fits_the_editor_and_pages_the_threads_it_leaves_out() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let project = store.create_project(project("/work/app")).await.unwrap();
    let title = "t".repeat(1024 * 1024);
    let mut created = HashSet::new();
    for _ in 0..30 {
        let thread = store
            .create_thread(CreateThread {
                id: ThreadId::new(),
                project_id: project.id.clone(),
                config: EngineConfig::new(EngineId::Claude),
                title: Some(title.clone()),
                parent_thread_id: None,
                spawned_by: None,
            })
            .await
            .unwrap();
        created.insert(thread.id);
    }

    let state = store.state().await.unwrap();
    let shipped = serde_json::to_vec(&state.threads).unwrap().len();
    assert!(
        shipped <= STATE_BYTES,
        "the starting state ships {shipped} bytes of threads"
    );
    let mut seen: HashSet<ThreadId> = state
        .threads
        .iter()
        .map(|thread| thread.id.clone())
        .collect();
    assert!(
        seen.len() < created.len(),
        "some threads are left for paging"
    );

    let mut cursor = state
        .threads_next
        .expect("the state says where the rest starts");
    loop {
        let page = store
            .threads_page(ThreadsQuery {
                window: ThreadWindow::Before(cursor),
                ..Default::default()
            })
            .await
            .unwrap();
        seen.extend(page.threads.iter().map(|thread| thread.id.clone()));
        let Some(next) = page.next else { break };
        cursor = next;
    }
    assert_eq!(
        seen, created,
        "paging on from the state reaches every thread"
    );
}

#[tokio::test]
async fn a_project_with_an_oversized_path_or_name_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let long_path = format!("/{}", "p".repeat(MAX_PROJECT_PATH_BYTES));
    let error = store.create_project(project(&long_path)).await.unwrap_err();
    assert!(
        matches!(*error.cause, CreateProjectFailure::PathTooLong { .. }),
        "{error:?}"
    );
    let mut named = project("/work/app");
    named.name = Some("n".repeat(MAX_PROJECT_NAME_BYTES + 1));
    let error = store.create_project(named).await.unwrap_err();
    assert!(
        matches!(*error.cause, CreateProjectFailure::NameTooLong { .. }),
        "{error:?}"
    );
    assert!(store.state().await.unwrap().projects.is_empty());
}

#[tokio::test]
async fn projects_past_the_limit_are_refused_until_one_is_removed() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let mut first = None;
    for index in 0..MAX_LIVE_PROJECTS {
        let made = store
            .create_project(project(&format!("/work/{index}")))
            .await
            .unwrap();
        first.get_or_insert(made.id);
    }
    let error = store
        .create_project(project("/work/extra"))
        .await
        .unwrap_err();
    assert!(
        matches!(*error.cause, CreateProjectFailure::TooMany { .. }),
        "{error:?}"
    );
    store.remove_project(&first.unwrap()).await.unwrap();
    store.create_project(project("/work/extra")).await.unwrap();
}

#[tokio::test]
async fn the_starting_state_carries_an_unread_thread_older_than_its_first_page() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let project = store.create_project(project("/work/app")).await.unwrap();
    let create = || CreateThread {
        id: ThreadId::new(),
        project_id: project.id.clone(),
        config: EngineConfig::new(EngineId::Claude),
        title: None,
        parent_thread_id: None,
        spawned_by: None,
    };
    let unread = store.create_thread(create()).await.unwrap().id;
    store.mark_unread(&unread).await.unwrap();
    for _ in 0..210 {
        store.create_thread(create()).await.unwrap();
    }
    let state = store.state().await.unwrap();
    assert!(
        state.threads.iter().any(|thread| thread.id == unread),
        "the inbox shows the unread thread without paging the whole history"
    );
}
