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
        &id[id.len() - 12..]
    ));
    std::fs::create_dir_all(&earlier).unwrap();

    assert_eq!(fixture.hub.thread_folder(&thread).await.unwrap(), earlier);
    fixture.hub.shutdown();
}
