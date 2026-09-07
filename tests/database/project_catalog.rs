use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use artisan_database::entities;
use artisan_database::{AttachProjectInput, Repository, RepositoryError, SqliteConfig, connect};
use artisan_domain::{
    DirectoryId, DisplayName, PROJECT_LISTING_MAX_PROJECTS, ProjectId, RequestId, RootPath,
    UnixMillis,
};
use artisan_migrations::migrate_to_current;
use sea_orm::{ActiveModelTrait, ActiveValue::Set, DatabaseConnection};

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

fn attach_input(project: &str, attached_at_ms: i64) -> AttachProjectInput {
    AttachProjectInput {
        request_id: RequestId::parse(format!("request-{project}"))
            .expect("request id should be valid"),
        directory_id: DirectoryId::parse(format!("directory-{project}"))
            .expect("directory id should be valid"),
        project_id: ProjectId::parse(project).expect("project id should be valid"),
        root_path: RootPath::parse(format!("C:/repos/{project}"))
            .expect("root path should be valid"),
        display_name: DisplayName::parse(format!("Project {project}"))
            .expect("display name should be valid"),
        attached_at: UnixMillis::from_millis(attached_at_ms),
    }
}

#[tokio::test]
async fn empty_catalog_is_a_valid_bounded_listing() {
    let (_database, repository) = memory_repository().await;
    let catalog = repository
        .list_projects()
        .await
        .expect("empty catalog should list");
    assert!(catalog.projects().is_empty());
}

#[tokio::test]
async fn catalog_reopens_in_deterministic_recency_and_identity_order() {
    let temporary = TemporaryDatabase::new("project-catalog");
    let database = connect(SqliteConfig::file(temporary.path()).sqlx_logging(false))
        .await
        .expect("file database should open");
    migrate_to_current(&database)
        .await
        .expect("file database should migrate");
    let repository = Repository::new(database.clone());
    for input in [
        attach_input("project-b", 300),
        attach_input("project-c", 100),
        attach_input("project-a", 300),
    ] {
        repository
            .attach_project(input)
            .await
            .expect("project should attach");
    }
    database.close().await.expect("database should close");

    let reopened = connect(SqliteConfig::file(temporary.path()).sqlx_logging(false))
        .await
        .expect("database should reopen");
    migrate_to_current(&reopened)
        .await
        .expect("reopen migration should be idempotent");
    let catalog = Repository::new(reopened.clone())
        .list_projects()
        .await
        .expect("reopened catalog should list");
    assert_eq!(
        catalog
            .projects()
            .iter()
            .map(|project| project.project_id.as_str())
            .collect::<Vec<_>>(),
        ["project-a", "project-b", "project-c"]
    );
    assert_eq!(catalog.projects()[0].attached_at.as_millis(), 300);
    assert_eq!(catalog.projects()[2].attached_at.as_millis(), 100);
    reopened.close().await.expect("database should close");
}

#[tokio::test]
async fn catalog_overflow_is_typed_at_one_row_beyond_the_bound() {
    let (database, repository) = memory_repository().await;
    for index in 0..=PROJECT_LISTING_MAX_PROJECTS {
        entities::attached_project::ActiveModel {
            project_id: Set(format!("project-{index:03}")),
            root_path: Set(format!("C:/repos/project-{index:03}")),
            display_name: Set(format!("Project {index}")),
            attached_at_ms: Set(i64::try_from(index).expect("fixture index should fit i64")),
        }
        .insert(&database)
        .await
        .expect("overflow fixture should insert");
    }

    assert!(matches!(
        repository
            .list_projects()
            .await
            .expect_err("257 projects must exceed the domain catalog bound"),
        RepositoryError::ProjectListing { .. }
    ));
}

#[tokio::test]
async fn catalog_rejects_rows_that_violate_domain_text_invariants() {
    let (database, repository) = memory_repository().await;
    entities::attached_project::ActiveModel {
        project_id: Set("project-corrupt".to_owned()),
        root_path: Set("C:/repos/corrupt".to_owned()),
        display_name: Set("   ".to_owned()),
        attached_at_ms: Set(100),
    }
    .insert(&database)
    .await
    .expect("schema-level corruption fixture should insert");

    assert!(matches!(
        repository
            .list_projects()
            .await
            .expect_err("corrupt rows must not escape as domain values"),
        RepositoryError::CorruptData {
            table: "attached_projects",
            field: "display_name",
            ..
        }
    ));
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
