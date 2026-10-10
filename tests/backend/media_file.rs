//! Media files a reply names, read through the request handler from the
//! Forge's machine: relative to the thread's project, or absolute.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use artisan_backend::{ForgeStorage, RequestHandler};
use artisan_database::{AttachProjectInput, CreateThreadInput, SqliteConfig};
use artisan_domain::{
    DirectoryId, DisplayName, MediaFilePath, MediaPathAnchor, ProjectId, Query, ReadMediaFile,
    RequestId, RootPath, ThreadId, ThreadTitle, UnixMillis,
};
use artisan_protocol::{ClientRequest, ErrorCode, ResponsePayload};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

const PNG: &[u8] = b"\x89PNG\r\n\x1a\nnot a real body";

struct Directory(PathBuf);

impl Drop for Directory {
    fn drop(&mut self) {
        let _cleanup_result = fs::remove_dir_all(&self.0);
    }
}

/// A Forge whose one thread belongs to a project in a real folder.
async fn forge() -> (Directory, PathBuf, ForgeStorage) {
    let directory = std::env::temp_dir().join(format!(
        "artisan-forge-media-file-{}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let project = directory.join("project");
    fs::create_dir_all(project.join("docs")).unwrap();
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
            root_path: RootPath::parse(project.to_str().unwrap()).unwrap(),
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
            title: ThreadTitle::parse("Thread").unwrap(),
            created_at: UnixMillis::from_millis(200),
            updated_at: UnixMillis::from_millis(200),
        })
        .await
        .expect("thread should create");
    (Directory(directory), project, storage)
}

fn request(value: &str) -> RequestId {
    RequestId::parse(value).expect("test request id should be valid")
}

async fn read(handler: &RequestHandler, thread: &str, path: &str) -> Result<Vec<u8>, ErrorCode> {
    let query = ReadMediaFile::new(
        ThreadId::parse(thread).unwrap(),
        MediaFilePath::new(path.to_owned(), MediaPathAnchor::Project).unwrap(),
    );
    let response = handler
        .respond(
            &request("read-media"),
            &ClientRequest::Query(Query::ReadMediaFile(query)),
        )
        .await
        .map_err(|failure| failure.code)?;
    match response.payload {
        ResponsePayload::MediaFile(result) => Ok(result.bytes),
        other => panic!("expected a media file, got {other:?}"),
    }
}

#[tokio::test]
async fn media_files_resolve_on_the_forges_machine() {
    let (_directory, project, storage) = forge().await;
    let handler = RequestHandler::new(storage.repository().clone());
    fs::write(project.join("docs/shot.png"), PNG).unwrap();
    fs::write(project.join("notes.txt"), "not media").unwrap();

    // Relative to the thread's project, with or without `./`.
    assert_eq!(
        read(&handler, "thread", "docs/shot.png").await,
        Ok(PNG.to_vec())
    );
    assert_eq!(
        read(&handler, "thread", "./docs/shot.png").await,
        Ok(PNG.to_vec())
    );
    // Absolute on this machine.
    let absolute = project.join("docs/shot.png");
    assert_eq!(
        read(&handler, "thread", absolute.to_str().unwrap()).await,
        Ok(PNG.to_vec())
    );
    // Missing, non-media, a folder, and another machine's path are refused.
    for path in [
        "docs/missing.png",
        "notes.txt",
        "docs",
        "C:\\Users\\me\\a.png",
    ] {
        assert_eq!(
            read(&handler, "thread", path).await,
            Err(ErrorCode::InvalidInput),
            "{path}"
        );
    }
    // An unknown thread has no project to resolve against.
    assert!(
        read(&handler, "missing-thread", "docs/shot.png")
            .await
            .is_err()
    );
}
