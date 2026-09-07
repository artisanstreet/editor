use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use artisan_database::entities;
use artisan_database::{
    AttachProjectInput, AttachProjectResult, CreateThreadInput, Repository, RepositoryError,
    SqliteConfig, connect,
};
use artisan_domain::{
    DirectoryId, DisplayName, ProjectId, ReceiptDisposition, RequestId, RootPath,
    THREAD_LISTING_MAX_THREADS, ThreadId, ThreadTitle, UnixMillis,
};
use artisan_migrations::migrate_to_current;
use sea_orm::{ActiveModelTrait, ActiveValue::Set, DatabaseConnection, EntityTrait};

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

async fn memory_repository() -> (DatabaseConnection, Repository) {
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
    (database.clone(), Repository::new(database))
}

fn request(value: &str) -> RequestId {
    RequestId::parse(value).expect("test request id should be valid")
}

fn directory(value: &str) -> DirectoryId {
    DirectoryId::parse(value).expect("test directory id should be valid")
}

fn project_id(value: &str) -> ProjectId {
    ProjectId::parse(value).expect("test project id should be valid")
}

fn thread_id(value: &str) -> ThreadId {
    ThreadId::parse(value).expect("test thread id should be valid")
}

fn title(value: &str) -> ThreadTitle {
    ThreadTitle::parse(value).expect("test title should be valid")
}

fn attach_input(
    request_id: &str,
    directory_id: &str,
    project_id: &str,
    root_path: &str,
) -> AttachProjectInput {
    AttachProjectInput {
        request_id: request(request_id),
        directory_id: directory(directory_id),
        project_id: self::project_id(project_id),
        root_path: RootPath::parse(root_path).expect("test root should be valid"),
        display_name: DisplayName::parse("Artisan").expect("test display name should be valid"),
        attached_at: UnixMillis::from_millis(100),
    }
}

fn create_input(
    request_id: &str,
    thread_id: &str,
    project_id: &str,
    title: &str,
    updated_at_ms: i64,
) -> CreateThreadInput {
    CreateThreadInput {
        request_id: request(request_id),
        thread_id: self::thread_id(thread_id),
        project_id: self::project_id(project_id),
        title: self::title(title),
        created_at: UnixMillis::from_millis(200),
        updated_at: UnixMillis::from_millis(updated_at_ms),
    }
}

async fn attach_project(repository: &Repository) -> AttachProjectResult {
    repository
        .attach_project(attach_input(
            "attach-request",
            "directory-1",
            "project-1",
            "C:/repos/artisan",
        ))
        .await
        .expect("project should attach")
}

#[tokio::test]
async fn attach_receipt_returns_original_project_and_detects_request_conflicts() {
    let (database, repository) = memory_repository().await;
    let accepted = attach_project(&repository).await;
    assert_eq!(accepted.receipt.disposition, ReceiptDisposition::Accepted);

    let lookup = repository
        .lookup_attach_project(&request("attach-request"), &directory("directory-1"))
        .await
        .expect("receipt lookup should work")
        .expect("receipt should exist");
    assert_eq!(lookup.receipt.disposition, ReceiptDisposition::Duplicate);
    assert_eq!(lookup.project, accepted.project);

    let duplicate = repository
        .attach_project(attach_input(
            "attach-request",
            "directory-1",
            "discarded-project-id",
            "C:/repos/changed-after-retry",
        ))
        .await
        .expect("exact command retry should return its original result");
    assert_eq!(duplicate.receipt.disposition, ReceiptDisposition::Duplicate);
    assert_eq!(duplicate.project, accepted.project);

    assert!(matches!(
        repository
            .lookup_attach_project(&request("attach-request"), &directory("directory-2"))
            .await
            .expect_err("same request id cannot name another directory"),
        RepositoryError::IdempotencyConflict { .. }
    ));
    assert!(matches!(
        repository
            .attach_project(attach_input(
                "attach-project-conflict",
                "directory-3",
                "project-1",
                "C:/repos/other",
            ))
            .await
            .expect_err("one project id cannot identify two roots"),
        RepositoryError::ProjectConflict { .. }
    ));

    let another_request = repository
        .attach_project(attach_input(
            "attach-request-2",
            "directory-2",
            "discarded-project-id",
            "C:/repos/artisan",
        ))
        .await
        .expect("another command may resolve to the stable project");
    assert_eq!(
        another_request.receipt.disposition,
        ReceiptDisposition::Accepted
    );
    assert_eq!(another_request.project.project_id, project_id("project-1"));
    assert_eq!(
        entities::attached_project::Entity::find()
            .all(&database)
            .await
            .expect("projects should query")
            .len(),
        1
    );
}

#[tokio::test]
async fn create_receipt_prevents_reminting_and_listing_is_deterministic() {
    let (database, repository) = memory_repository().await;
    attach_project(&repository).await;

    let accepted = repository
        .create_thread(create_input(
            "create-request-a",
            "thread-a",
            "project-1",
            "Alpha",
            300,
        ))
        .await
        .expect("thread should create");
    assert_eq!(accepted.receipt.disposition, ReceiptDisposition::Accepted);

    let preflight = repository
        .lookup_create_thread(
            &request("create-request-a"),
            &project_id("project-1"),
            &title("Alpha"),
        )
        .await
        .expect("create receipt lookup should work")
        .expect("create receipt should exist");
    assert_eq!(preflight.receipt.disposition, ReceiptDisposition::Duplicate);
    assert_eq!(preflight.thread, accepted.thread);

    let duplicate = repository
        .create_thread(create_input(
            "create-request-a",
            "discarded-thread-id",
            "project-1",
            "Alpha",
            999,
        ))
        .await
        .expect("retry should return the original thread");
    assert_eq!(duplicate.receipt.disposition, ReceiptDisposition::Duplicate);
    assert_eq!(duplicate.thread, accepted.thread);
    assert!(
        entities::thread::Entity::find_by_id("discarded-thread-id")
            .one(&database)
            .await
            .expect("thread query should work")
            .is_none()
    );

    assert!(matches!(
        repository
            .lookup_create_thread(
                &request("create-request-a"),
                &project_id("project-1"),
                &title("Changed"),
            )
            .await
            .expect_err("same request id cannot change title"),
        RepositoryError::IdempotencyConflict { .. }
    ));
    repository
        .create_thread(create_input(
            "create-request-c",
            "thread-c",
            "project-1",
            "Charlie",
            300,
        ))
        .await
        .expect("second recent thread should create");
    repository
        .create_thread(create_input(
            "create-request-b",
            "thread-b",
            "project-1",
            "Bravo",
            250,
        ))
        .await
        .expect("older thread should create");

    let listing = repository
        .list_threads(&project_id("project-1"))
        .await
        .expect("threads should list");
    assert_eq!(
        listing
            .threads()
            .iter()
            .map(|thread| thread.thread_id.as_str())
            .collect::<Vec<_>>(),
        ["thread-a", "thread-c", "thread-b"]
    );
}

#[tokio::test]
async fn create_rejects_missing_projects_and_thread_identity_collisions() {
    let (_database, repository) = memory_repository().await;
    attach_project(&repository).await;
    repository
        .create_thread(create_input(
            "create-request-a",
            "thread-a",
            "project-1",
            "Alpha",
            300,
        ))
        .await
        .expect("fixture thread should create");

    assert!(matches!(
        repository
            .create_thread(create_input(
                "create-thread-collision",
                "thread-a",
                "project-1",
                "Alpha",
                300,
            ))
            .await
            .expect_err("a new request cannot claim an existing thread id"),
        RepositoryError::ThreadConflict { .. }
    ));
    assert!(matches!(
        repository
            .create_thread(create_input(
                "create-missing-project",
                "orphan",
                "missing-project",
                "Orphan",
                200,
            ))
            .await
            .expect_err("missing projects should be typed"),
        RepositoryError::ProjectNotFound { .. }
    ));
}

#[tokio::test]
async fn request_identity_is_global_across_attach_and_create() {
    let (_database, repository) = memory_repository().await;
    attach_project(&repository).await;

    let error = repository
        .create_thread(create_input(
            "attach-request",
            "thread-1",
            "project-1",
            "First",
            200,
        ))
        .await
        .expect_err("an attach request id cannot be reused for create");
    assert!(matches!(error, RepositoryError::IdempotencyConflict { .. }));
}

#[tokio::test]
async fn bounded_thread_listing_reports_overflow_without_loading_the_whole_table() {
    let (database, repository) = memory_repository().await;
    attach_project(&repository).await;

    for index in 0..=THREAD_LISTING_MAX_THREADS {
        entities::thread::ActiveModel {
            thread_id: Set(format!("thread-{index:03}")),
            project_id: Set("project-1".to_owned()),
            title: Set(format!("Thread {index}")),
            created_at_ms: Set(200),
            updated_at_ms: Set(200),
            engine_run_config_version: Set(None),
            engine_run_config_revision: Set(0),
            engine_run_config: Set(None),
        }
        .insert(&database)
        .await
        .expect("bounded overflow fixture should insert");
    }

    assert!(matches!(
        repository
            .list_threads(&project_id("project-1"))
            .await
            .expect_err("257 rows must exceed the domain listing bound"),
        RepositoryError::ThreadListing { .. }
    ));
}

#[tokio::test]
async fn concurrent_create_retry_commits_one_thread_and_reopens_original_receipt() {
    let temporary = TemporaryDatabase::new("create-contention");
    let database = connect(
        SqliteConfig::file(temporary.path())
            .min_connections(1)
            .max_connections(4)
            .sqlx_logging(false),
    )
    .await
    .expect("file database should open");
    migrate_to_current(&database)
        .await
        .expect("file database should migrate");
    let repository = Repository::new(database.clone());
    attach_project(&repository).await;

    let first_repository = repository.clone();
    let second_repository = repository.clone();
    let first = tokio::spawn(async move {
        first_repository
            .create_thread(create_input(
                "create-request",
                "thread-first",
                "project-1",
                "Concurrent",
                200,
            ))
            .await
    });
    let second = tokio::spawn(async move {
        second_repository
            .create_thread(create_input(
                "create-request",
                "thread-second",
                "project-1",
                "Concurrent",
                200,
            ))
            .await
    });
    let results = [
        first
            .await
            .expect("first task should finish")
            .expect("first create should work"),
        second
            .await
            .expect("second task should finish")
            .expect("second create should work"),
    ];
    assert_eq!(
        results
            .iter()
            .filter(|result| result.receipt.disposition == ReceiptDisposition::Accepted)
            .count(),
        1
    );
    assert_eq!(
        results
            .iter()
            .filter(|result| result.receipt.disposition == ReceiptDisposition::Duplicate)
            .count(),
        1
    );
    assert_eq!(results[0].thread, results[1].thread);
    database.close().await.expect("database should close");

    let reopened = connect(SqliteConfig::file(temporary.path()).sqlx_logging(false))
        .await
        .expect("database should reopen");
    migrate_to_current(&reopened)
        .await
        .expect("reopen migration should be idempotent");
    let reopened_repository = Repository::new(reopened.clone());
    let receipt = reopened_repository
        .lookup_create_thread(
            &request("create-request"),
            &project_id("project-1"),
            &title("Concurrent"),
        )
        .await
        .expect("reopened receipt lookup should work")
        .expect("receipt should survive reopen");
    assert_eq!(receipt.thread, results[0].thread);
    assert_eq!(receipt.receipt.disposition, ReceiptDisposition::Duplicate);
    assert_eq!(
        entities::thread::Entity::find()
            .all(&reopened)
            .await
            .expect("threads should query")
            .len(),
        1
    );
    reopened.close().await.expect("database should close");
}

struct TemporaryDatabase {
    path: PathBuf,
}

impl TemporaryDatabase {
    fn new(label: &str) -> Self {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should follow Unix epoch")
            .as_nanos();
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "artisan-database-{label}-{}-{timestamp}-{sequence}.sqlite3",
            std::process::id()
        ));
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TemporaryDatabase {
    fn drop(&mut self) {
        for suffix in ["", "-shm", "-wal"] {
            let candidate = PathBuf::from(format!("{}{suffix}", self.path.display()));
            if let Err(error) = std::fs::remove_file(&candidate) {
                assert_eq!(
                    error.kind(),
                    std::io::ErrorKind::NotFound,
                    "failed to remove temporary database file {}: {error}",
                    candidate.display()
                );
            }
        }
    }
}
