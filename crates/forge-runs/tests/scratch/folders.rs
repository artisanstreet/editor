use super::*;

async fn scratch_thread(fixture: &Fixture, id: &str, title: &str) -> artisan_api::Thread {
    fixture.hub.ensure_scratch_project().await.unwrap();
    fixture
        .hub
        .store()
        .create_thread(CreateThread {
            id: ThreadId::from(id),
            project_id: ProjectId::scratch(),
            config: EngineConfig::new(EngineId::Claude),
            title: Some(title.to_owned()),
            parent_thread_id: None,
            spawned_by: None,
        })
        .await
        .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn threads_whose_ids_end_alike_never_share_or_delete_a_folder() {
    let fixture = fixture(Arc::new(|_turn, _io| Box::pin(async { Outcome::done() }))).await;
    let first = scratch_thread(&fixture, "alpha-0123456789ab", "Plan the trip").await;
    let second = scratch_thread(&fixture, "beta-0123456789ab", "Plan the trip").await;
    let first_folder = fixture.hub.thread_folder(&first).await.unwrap();
    let second_folder = fixture.hub.thread_folder(&second).await.unwrap();
    std::fs::write(second_folder.join("notes.md"), "keep me").unwrap();

    scratch::remove_scratch_folder(&fixture.hub.scratch_root(), &first).unwrap();

    assert_ne!(first_folder, second_folder);
    assert!(!first_folder.exists());
    assert_eq!(
        std::fs::read_to_string(second_folder.join("notes.md")).unwrap(),
        "keep me"
    );
    fixture.hub.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_title_with_one_very_long_word_still_gets_a_folder() {
    let fixture = fixture(Arc::new(|_turn, _io| Box::pin(async { Outcome::done() }))).await;
    let thread = scratch_thread(&fixture, &ThreadId::new().to_string(), &"a".repeat(400)).await;

    let folder = fixture.hub.thread_folder(&thread).await.unwrap();

    assert!(folder.is_dir());
    fixture.hub.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_thread_keeps_the_folder_it_was_given_before_folders_named_whole_ids() {
    let fixture = fixture(Arc::new(|_turn, _io| Box::pin(async { Outcome::done() }))).await;
    let thread = scratch_thread(&fixture, &ThreadId::new().to_string(), "Plan the trip").await;
    let id: String = thread
        .id
        .as_str()
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect();
    let earlier = fixture.hub.scratch_root().join(format!(
        "{}-plan-the-trip-{}",
        scratch::date(thread.created_at),
        id.split_at(id.len() - 12).1
    ));
    std::fs::create_dir_all(&earlier).unwrap();

    assert_eq!(fixture.hub.thread_folder(&thread).await.unwrap(), earlier);
    fixture.hub.shutdown();
}

fn repository_with_worktree(common: &std::path::Path, project: &std::path::Path) {
    for folder in ["objects", "refs"] {
        std::fs::create_dir_all(common.join(folder)).unwrap();
    }
    std::fs::write(common.join("HEAD"), "ref: refs/heads/main\n").unwrap();
    let own = common.join("worktrees").join("project");
    std::fs::create_dir_all(&own).unwrap();
    std::fs::write(
        own.join("gitdir"),
        format!("{}\n", project.join(".git").display()),
    )
    .unwrap();
    std::fs::write(own.join("commondir"), "../..\n").unwrap();
    std::fs::write(own.join("HEAD"), "ref: refs/heads/feature\n").unwrap();
    std::fs::write(project.join(".git"), format!("gitdir: {}\n", own.display())).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_rewritten_git_pointer_does_not_change_the_git_folder_a_project_may_write() {
    let fixture = fixture(Arc::new(|_turn, _io| Box::pin(async { Outcome::done() }))).await;
    let folders = tempfile::tempdir().unwrap();
    let project = folders.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let first = folders.path().join("first");
    let second = folders.path().join("second");
    repository_with_worktree(&second, &project);
    repository_with_worktree(&first, &project);
    let project_id = ProjectId::new();

    let found = fixture
        .hub
        .sandbox_git_common(&project_id, &project)
        .unwrap();
    std::fs::write(
        project.join(".git"),
        format!("gitdir: {}\n", second.join("worktrees/project").display()),
    )
    .unwrap();
    let later = fixture
        .hub
        .sandbox_git_common(&project_id, &project)
        .unwrap();

    let first = std::fs::canonicalize(&first).unwrap();
    assert_eq!(found, Some(first.clone()));
    assert_eq!(
        later,
        Some(first),
        "a `.git` pointer an Auto run rewrote changed which git folder later runs may write"
    );
    fixture.hub.shutdown();
}
