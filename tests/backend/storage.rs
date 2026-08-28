//! External lifecycle tests for Forge-owned native storage assembly.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use artisan_backend::{ForgeStorage, ForgeStorageOpenError};
use artisan_database::{AttachProjectInput, ConnectError, SqliteConfig, connect};
use artisan_domain::{DirectoryId, DisplayName, ProjectId, RequestId, RootPath, UnixMillis};
use sea_orm::ConnectionTrait;

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct TemporaryDatabase {
    directory: PathBuf,
    database: PathBuf,
}

impl TemporaryDatabase {
    fn new(label: &str) -> Self {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!(
            "artisan-forge-storage-{label}-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&directory).expect("temporary database directory should be created");
        let database = directory.join("forge.sqlite3");
        Self {
            directory,
            database,
        }
    }

    fn path(&self) -> &Path {
        &self.database
    }
}

impl Drop for TemporaryDatabase {
    fn drop(&mut self) {
        let _cleanup_result = fs::remove_dir_all(&self.directory);
    }
}

fn attach_input() -> AttachProjectInput {
    AttachProjectInput {
        request_id: RequestId::parse("request-project-1").expect("valid request id"),
        directory_id: DirectoryId::parse("directory-project-1").expect("valid directory id"),
        project_id: ProjectId::parse("project-1").expect("valid project id"),
        root_path: RootPath::parse("C:/repos/project-1").expect("valid root path"),
        display_name: DisplayName::parse("Project One").expect("valid display name"),
        attached_at: UnixMillis::from_millis(100),
    }
}

#[tokio::test]
async fn startup_migrates_before_repository_use_and_reopens_durable_state() {
    let temporary = TemporaryDatabase::new("restart");
    let config = || SqliteConfig::file(temporary.path()).sqlx_logging(false);

    let storage = ForgeStorage::open(config())
        .await
        .expect("Forge storage should open and migrate");
    let empty = storage
        .repository()
        .list_projects()
        .await
        .expect("migrated repository should be immediately usable");
    assert!(empty.projects().is_empty());
    storage
        .repository()
        .attach_project(attach_input())
        .await
        .expect("project should persist through the ready repository");
    storage.close().await.expect("storage should close cleanly");

    let reopened = ForgeStorage::open(config())
        .await
        .expect("Forge storage should reopen and rerun migrations idempotently");
    let catalog = reopened
        .repository()
        .list_projects()
        .await
        .expect("reopened repository should list durable state");
    assert_eq!(catalog.projects().len(), 1);
    assert_eq!(catalog.projects()[0].project_id.as_str(), "project-1");
    assert_eq!(
        catalog.projects()[0].root_path.as_str(),
        "C:/repos/project-1"
    );
    reopened
        .close()
        .await
        .expect("reopened storage should close");
}

#[tokio::test]
async fn invalid_connection_policy_remains_a_typed_startup_failure() {
    let result = ForgeStorage::open(
        SqliteConfig::in_memory()
            .min_connections(2)
            .max_connections(1)
            .sqlx_logging(false),
    )
    .await;

    assert!(matches!(
        result,
        Err(ForgeStorageOpenError::Connect {
            source: ConnectError::InvalidConfig { .. }
        })
    ));
}

#[tokio::test]
async fn incompatible_migration_ledger_remains_a_typed_startup_failure() {
    let temporary = TemporaryDatabase::new("migration-failure");
    let config = || SqliteConfig::file(temporary.path()).sqlx_logging(false);
    let database = connect(config())
        .await
        .expect("fixture database should open");
    database
        .execute_unprepared("CREATE TABLE seaql_migrations (broken TEXT NOT NULL)")
        .await
        .expect("incompatible migration ledger should be seeded");
    database
        .close()
        .await
        .expect("fixture database should close");

    assert!(matches!(
        ForgeStorage::open(config()).await,
        Err(ForgeStorageOpenError::Migrate { .. })
    ));
}
