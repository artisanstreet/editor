//! The reader's thread changes through the request handler: each answers
//! the archived threads, and unknown threads fail as unknown.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use artisan_backend::{ForgeStorage, RequestHandler};
use artisan_database::{AttachProjectInput, CreateThreadInput, SqliteConfig};
use artisan_domain::{
    ArchivedThreadListing, Command, DirectoryId, DisplayName, ProjectId, Query,
    ReadArchivedThreads, RequestId, RootPath, ThreadChange, ThreadId, ThreadTitle, UnixMillis,
    UpdateThread,
};
use artisan_protocol::{ClientRequest, ErrorCode, ResponsePayload};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct Directory(PathBuf);

impl Drop for Directory {
    fn drop(&mut self) {
        let _cleanup_result = fs::remove_dir_all(&self.0);
    }
}

async fn storage() -> (Directory, ForgeStorage) {
    let directory = std::env::temp_dir().join(format!(
        "artisan-forge-thread-lifecycle-{}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&directory).expect("temporary directory should be created");
    let storage =
        ForgeStorage::open(SqliteConfig::file(directory.join("forge.sqlite3")).sqlx_logging(false))
            .await
            .expect("Forge storage should open and migrate");
    let repository = storage.repository();
    repository
        .attach_project(AttachProjectInput {
            request_id: request("attach"),
            directory_id: DirectoryId::parse("directory").unwrap(),
            project_id: ProjectId::parse("project").unwrap(),
            root_path: RootPath::parse("C:/repos/project").unwrap(),
            display_name: DisplayName::parse("Project").unwrap(),
            attached_at: UnixMillis::from_millis(100),
        })
        .await
        .expect("project should attach");
    repository
        .create_thread(CreateThreadInput {
            request_id: request("create"),
            thread_id: ThreadId::parse("thread").unwrap(),
            project_id: ProjectId::parse("project").unwrap(),
            title: ThreadTitle::parse("First thread").unwrap(),
            created_at: UnixMillis::from_millis(200),
            updated_at: UnixMillis::from_millis(200),
        })
        .await
        .expect("thread should create");
    (Directory(directory), storage)
}

fn request(value: &str) -> RequestId {
    RequestId::parse(value).expect("test request id should be valid")
}

async fn update(
    handler: &RequestHandler,
    id: &str,
    thread: &str,
    change: ThreadChange,
) -> Result<ArchivedThreadListing, ErrorCode> {
    let response = handler
        .respond(
            &request(id),
            &ClientRequest::Command(Command::UpdateThread(UpdateThread {
                request_id: request(id),
                thread_id: ThreadId::parse(thread).unwrap(),
                change,
            })),
        )
        .await
        .map_err(|failure| failure.code)?;
    match response.payload {
        ResponsePayload::ArchivedThreads(listing) => Ok(listing),
        other => panic!("expected the archived threads, got {other:?}"),
    }
}

#[tokio::test]
async fn thread_changes_answer_the_archive() {
    let (_directory, storage) = storage().await;
    let handler = RequestHandler::new(storage.repository().clone());

    let renamed = update(
        &handler,
        "rename",
        "thread",
        ThreadChange::Rename(ThreadTitle::parse("Renamed").unwrap()),
    )
    .await
    .expect("rename should apply");
    assert!(renamed.threads().is_empty());

    let archived = update(&handler, "archive", "thread", ThreadChange::Archive)
        .await
        .expect("archive should apply");
    assert_eq!(archived.threads().len(), 1);
    let row = &archived.threads()[0];
    assert_eq!(row.thread.thread.title.as_str(), "Renamed");
    assert_eq!(row.thread.subtitle.as_str(), "Project");
    assert!(row.erases_at() > row.archived_at);

    let read = handler
        .respond(
            &request("read"),
            &ClientRequest::Query(Query::ReadArchivedThreads(ReadArchivedThreads)),
        )
        .await
        .expect("archive should read");
    assert!(matches!(
        read.payload,
        ResponsePayload::ArchivedThreads(listing) if listing == archived
    ));

    let restored = update(&handler, "restore", "thread", ThreadChange::Unarchive)
        .await
        .expect("restore should apply");
    assert!(restored.threads().is_empty());

    assert_eq!(
        update(&handler, "unknown", "missing", ThreadChange::Settle).await,
        Err(ErrorCode::ThreadUnknown)
    );
    storage.close().await.expect("storage should close");
}
