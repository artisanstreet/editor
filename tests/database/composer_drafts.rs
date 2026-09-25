//! Forge-owned composer drafts and stored attachments against migrated SQLite.

use artisan_database::{
    COMPOSER_ATTACHMENT_UNREFERENCED_GRACE_MS, ComposerDraftRepositoryError, Repository,
    SaveComposerDraftInput, SqliteConfig, connect,
};
use artisan_domain::{
    AuthoredText, ComposerAttachmentDigest, ComposerAttachmentRef, ComposerDraftScope,
    ImageAttachment, ImageMimeType, ProjectId, RequestId, ThreadId, UnixMillis,
};
use artisan_migrations::migrate_to_current;
use sea_orm::ConnectionTrait;

async fn repository() -> Repository {
    let database = connect(
        SqliteConfig::in_memory()
            .min_connections(1)
            .max_connections(1)
            .sqlx_logging(false),
    )
    .await
    .expect("memory database should open");
    migrate_to_current(&database)
        .await
        .expect("memory database should migrate");
    for statement in [
        "INSERT INTO attached_projects (project_id, root_path, display_name, attached_at_ms) VALUES ('p1', 'C:/work/p1', 'Project', 1)",
        "INSERT INTO threads (thread_id, project_id, title, created_at_ms, updated_at_ms) VALUES ('t1', 'p1', 'Thread', 2, 2)",
    ] {
        database
            .execute_unprepared(statement)
            .await
            .expect("seed row");
    }
    Repository::new(database)
}

fn thread() -> ComposerDraftScope {
    ComposerDraftScope::Thread(ThreadId::parse("t1").unwrap())
}

fn save(
    scope: ComposerDraftScope,
    text: &str,
    attachments: Vec<ComposerAttachmentRef>,
    saved_at: i64,
) -> SaveComposerDraftInput {
    static SAVES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let save = SAVES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    SaveComposerDraftInput {
        request_id: RequestId::parse(format!("save-{save}")).unwrap(),
        scope,
        text: AuthoredText::parse(text).unwrap(),
        attachments,
        saved_at: UnixMillis::from_millis(saved_at),
    }
}

fn image(byte: u8) -> ImageAttachment {
    ImageAttachment::new("image/png", vec![byte; 3], "shot.png").unwrap()
}

#[tokio::test]
async fn every_save_applies_and_the_forge_assigns_the_next_revision() {
    let repository = repository().await;
    assert_eq!(
        repository.read_composer_draft(&thread()).await.unwrap(),
        None
    );
    let first = repository
        .save_composer_draft(save(thread(), "first", Vec::new(), 10))
        .await
        .unwrap();
    assert_eq!(first.revision.get(), 1);

    // Two Editors that each think they are current both persist; the last
    // save to arrive is the stored draft.
    for (text, at) in [("editor a", 11), ("editor b", 12)] {
        repository
            .save_composer_draft(save(thread(), text, Vec::new(), at))
            .await
            .unwrap();
    }
    let draft = repository
        .read_composer_draft(&thread())
        .await
        .unwrap()
        .expect("draft stored");
    assert_eq!(draft.revision().get(), 3);
    assert_eq!(draft.text().as_str(), "editor b");
    assert_eq!(draft.updated_at(), UnixMillis::from_millis(12));

    // Clearing keeps the row, so the revision sequence never restarts.
    let cleared = repository
        .save_composer_draft(save(thread(), "", Vec::new(), 13))
        .await
        .unwrap();
    assert_eq!(cleared.revision.get(), 4);
    let stored = repository
        .read_composer_draft(&thread())
        .await
        .unwrap()
        .unwrap();
    assert!(stored.is_empty());
    assert_eq!(stored.revision().get(), 4);
}

#[tokio::test]
async fn a_save_from_a_client_that_never_read_is_applied_past_a_high_revision() {
    let repository = repository().await;
    for index in 0..100 {
        repository
            .save_composer_draft(save(thread(), &format!("v{index}"), Vec::new(), index))
            .await
            .unwrap();
    }
    let late = repository
        .save_composer_draft(save(thread(), "fresh editor", Vec::new(), 200))
        .await
        .unwrap();
    assert_eq!(late.revision.get(), 101);
    assert_eq!(
        repository
            .read_composer_draft(&thread())
            .await
            .unwrap()
            .unwrap()
            .text()
            .as_str(),
        "fresh editor"
    );
}

#[tokio::test]
async fn drafts_need_an_existing_scope_and_projects_have_their_own_draft() {
    let repository = repository().await;
    let unknown = ComposerDraftScope::Thread(ThreadId::parse("missing").unwrap());
    assert!(matches!(
        repository
            .save_composer_draft(save(unknown, "text", Vec::new(), 1))
            .await,
        Err(ComposerDraftRepositoryError::ScopeNotFound { kind: "thread", .. })
    ));
    let project = ComposerDraftScope::Project(ProjectId::parse("p1").unwrap());
    repository
        .save_composer_draft(save(project.clone(), "new task", Vec::new(), 1))
        .await
        .unwrap();
    assert_eq!(
        repository
            .read_composer_draft(&project)
            .await
            .unwrap()
            .unwrap()
            .text()
            .as_str(),
        "new task"
    );
    assert_eq!(
        repository.read_composer_draft(&thread()).await.unwrap(),
        None
    );
}

#[tokio::test]
async fn drafts_reference_stored_attachments_in_authored_order() {
    let repository = repository().await;
    let first = repository
        .store_composer_attachment(&image(1), UnixMillis::from_millis(1))
        .await
        .unwrap();
    let second = repository
        .store_composer_attachment(&image(2), UnixMillis::from_millis(1))
        .await
        .unwrap();
    let again = repository
        .store_composer_attachment(&image(1), UnixMillis::from_millis(2))
        .await
        .unwrap();
    assert_eq!(
        first.digest(),
        again.digest(),
        "storage is content addressed"
    );
    assert_eq!(first.mime_type(), ImageMimeType::Png);
    assert_eq!(first.size_bytes(), 3);

    repository
        .save_composer_draft(save(
            thread(),
            "look",
            vec![second.clone(), first.clone()],
            5,
        ))
        .await
        .unwrap();
    let draft = repository
        .read_composer_draft(&thread())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(draft.attachments(), [second.clone(), first.clone()]);

    let stored = repository
        .read_composer_attachment(first.digest())
        .await
        .unwrap()
        .expect("bytes stored");
    assert_eq!(stored.bytes, vec![1; 3]);

    let resolved = repository
        .resolve_composer_attachments(&[first.clone(), second])
        .await
        .unwrap();
    assert_eq!(resolved, vec![image(1), image(2)]);

    let unknown = ComposerAttachmentRef::new(
        ComposerAttachmentDigest::new([9; 32]),
        ImageMimeType::Png,
        "ghost.png",
        3,
    )
    .unwrap();
    assert!(matches!(
        repository
            .save_composer_draft(save(thread(), "ghost", vec![unknown.clone()], 6))
            .await,
        Err(ComposerDraftRepositoryError::AttachmentNotStored { .. })
    ));
    assert!(matches!(
        repository.resolve_composer_attachments(&[unknown]).await,
        Err(ComposerDraftRepositoryError::AttachmentNotStored { .. })
    ));
    let wrong_size =
        ComposerAttachmentRef::new(*first.digest(), ImageMimeType::Png, "a.png", 4).unwrap();
    assert!(matches!(
        repository
            .save_composer_draft(save(thread(), "lie", vec![wrong_size], 6))
            .await,
        Err(ComposerDraftRepositoryError::AttachmentMismatch { .. })
    ));
    // The rejected saves rolled back: revision 1 is still stored.
    assert_eq!(
        repository
            .read_composer_draft(&thread())
            .await
            .unwrap()
            .unwrap()
            .revision()
            .get(),
        1
    );
}

#[tokio::test]
async fn unreferenced_attachments_are_pruned_after_the_grace_period() {
    let repository = repository().await;
    let kept = repository
        .store_composer_attachment(&image(1), UnixMillis::from_millis(0))
        .await
        .unwrap();
    let dropped = repository
        .store_composer_attachment(&image(2), UnixMillis::from_millis(0))
        .await
        .unwrap();
    let later = COMPOSER_ATTACHMENT_UNREFERENCED_GRACE_MS + 1;
    repository
        .save_composer_draft(save(thread(), "", vec![kept.clone()], later))
        .await
        .unwrap();
    assert!(
        repository
            .read_composer_attachment(kept.digest())
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        repository
            .read_composer_attachment(dropped.digest())
            .await
            .unwrap()
            .is_none()
    );
}
