//! External lifecycle tests for the configured Forge application boundary.

use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use artisan_backend::{ForgeApp, ForgeConfig, ForgeStorageOpenError};
use artisan_database::{AttachProjectInput, ConnectError, SqliteConfig};
use artisan_domain::{DirectoryId, DisplayName, ProjectId, RequestId, RootPath, UnixMillis};

const _: fn() = || {
    struct DefaultMarker;
    trait AmbiguousIfDefault<A> {
        fn marker() {}
    }
    impl<T: ?Sized> AmbiguousIfDefault<()> for T {}
    impl<T: Default> AmbiguousIfDefault<DefaultMarker> for T {}
    let _ = <ForgeConfig as AmbiguousIfDefault<_>>::marker;
};

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct TemporaryDatabase {
    directory: PathBuf,
    database: PathBuf,
}

impl TemporaryDatabase {
    fn new(label: &str) -> Self {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!(
            "artisan-forge-app-{label}-{}-{sequence}",
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

fn find_in_chain<'origin, E>(source: Option<&'origin (dyn Error + 'static)>) -> Option<&'origin E>
where
    E: Error + 'static,
{
    let mut current = source;
    while let Some(error) = current {
        if let Some(typed) = error.downcast_ref::<E>() {
            return Some(typed);
        }
        current = error.source();
    }
    None
}

#[tokio::test]
async fn configured_start_migrates_before_ready_and_shuts_down_cleanly() {
    let temporary = TemporaryDatabase::new("ready");
    let app = ForgeApp::start(ForgeConfig::new(
        SqliteConfig::file(temporary.path()).sqlx_logging(false),
    ))
    .await
    .expect("injected configuration should start a ready application");

    let empty = app
        .repository()
        .list_projects()
        .await
        .expect("ready application should expose its migrated repository");
    assert!(empty.projects().is_empty());

    app.shutdown()
        .await
        .expect("owned storage should close through the typed shutdown boundary");
}

#[tokio::test]
async fn startup_failure_preserves_the_typed_connection_source() {
    let started = ForgeApp::start(ForgeConfig::new(
        SqliteConfig::in_memory()
            .min_connections(2)
            .max_connections(1)
            .sqlx_logging(false),
    ))
    .await;

    let Err(error) = started else {
        panic!("invalid injected configuration must not become ready");
    };
    assert_eq!(error.to_string(), "failed to start the Forge application");
    find_in_chain::<ForgeStorageOpenError>(error.source())
        .expect("startup failure should preserve the storage open source");

    let connect = find_in_chain::<ConnectError>(error.source())
        .expect("startup failure should preserve the connection source");
    assert!(matches!(connect, ConnectError::InvalidConfig { .. }));
}

#[tokio::test]
async fn owned_storage_survives_shutdown_and_reopens_durable_state() {
    let temporary = TemporaryDatabase::new("reopen");
    let config = || ForgeConfig::new(SqliteConfig::file(temporary.path()).sqlx_logging(false));

    let app = ForgeApp::start(config())
        .await
        .expect("first start should own storage");
    app.repository()
        .attach_project(attach_input())
        .await
        .expect("project should persist through the owned repository");
    app.shutdown()
        .await
        .expect("shutdown should release the first owner cleanly");

    let reopened = ForgeApp::start(config())
        .await
        .expect("second start should reopen the same configured database");
    let catalog = reopened
        .repository()
        .list_projects()
        .await
        .expect("reopened repository should list durable state");
    assert_eq!(catalog.projects().len(), 1);
    assert_eq!(catalog.projects()[0].project_id.as_str(), "project-1");
    reopened
        .shutdown()
        .await
        .expect("reopened application should shut down");
}
