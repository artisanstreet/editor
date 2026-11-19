pub mod admin;
mod blobs;
mod collection;
mod content;
pub mod coordination;
mod data_lock;
pub mod discovery;
mod encoded;
mod environment;
mod error;
mod feed;
pub mod hot;
mod items;
mod pairing;
mod progress;
mod read;
mod recall;
mod resources;
mod retained;
mod retention;
mod rollbacks;
mod schema;
mod secrets;
mod settings;
mod source_index;
mod sources;
#[cfg(test)]
mod stamped_writes;
mod text_search;
mod threads;
mod writer;

use crate::error::AtStep as _;
use artisan_api::{
    BlobHash, DeviceId, Envelope, Event, Item, ItemId, ItemKind, Project, ProjectId, Run, RunId,
    Seq, Status, Thread, ThreadId, Timestamp,
};
use pairing::{Authorizer, Devices};
use rusqlite::Connection;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;
use tokio::sync::{Semaphore, broadcast, mpsc, oneshot, watch};
use writer::{Durability, Job, Tx, Writer};

pub use blobs::{
    BlobFailure, BlobFileError, BlobRejected, BlobUpload, CollectBlobsError, InputImagesError,
    MAX_BLOB_BYTES, MAX_IMAGE_PIXELS, MAX_MESSAGE_IMAGE_BYTES, MAX_MESSAGE_IMAGES, PutBlobError,
    PutImageError,
};
pub use collection::{AnswerRefused, MAX_COLLECTION_BYTES};
pub use content::{
    ContentError, Damage, MAX_FIELD_BYTES, MAX_FIELD_ENTRIES, MAX_ITEM_BYTES, MAX_SOURCE_BYTES,
};
pub use data_lock::{DataLock, LockDataError};
pub use environment::{Connections, Environment};
pub use error::StorageError;
pub use feed::Subscription;
pub use items::{
    AcceptInputsError, Acceptance, AgentByAddressError, AnswerFailure, AnswerQuestionError,
    AwaitingItemsError, BeginRunError, CancelInputsError, CancelledItemsError,
    ContinuationChainError, ContinuePendingError, InsertMessageError, ItemByNativeError,
    ItemsPageError, NativeIdError, NextParkedError, PendingInputsError, PendingThreadsError,
    ReadContinueError, ReadItemError, ReopenAgentError, RequestContinueError,
    ResolveCollectionsError, RestoreInputsError, RunnableInputError, SendMessageError,
    SetItemStatusError, SettleAgentError, SettleRunError, SpawnAgentError, SpawnFailure,
    TakeContinueError, TranscriptPageError, TranscriptRowsError, TurnUnsandboxedError, UnparkError,
    UpsertItemError, ValidateAnswersError, WorkPageError,
};
pub use pairing::{
    AddDeviceError, AddPairCodeError, AuthorizeError, CertificateError, Device, DeviceCertificate,
    PairCodeHash, PairDeviceError, RevokeDeviceError, TouchDeviceError,
};
pub use read::{MAX_SEARCH_BYTES, ThreadsPageFailure, WAITING_ITEMS};
pub use recall::{
    GrepCursor, GrepFailure, GrepField, GrepMatch, GrepPage, GrepQuery, PrepareIndexError,
    RecallGrepError, RecallIndex, RecallKind, RecallScope, RecallSearchError, RecallTouchingError,
    SearchCursor, SearchHit, SearchPage, Touch,
};
pub use resources::{
    CollectSnapshotsError, MAX_RESOURCE_SNAPSHOT_BYTES, ResourceFailure,
    keep as keep_resource_snapshot,
};
pub use retained::RetainedError;
pub use retention::RemoveInactiveError;
pub use rollbacks::DismissRollbackError;
pub use schema::{
    BackupError, FinishUpgradeError, MigrationFailure, QuestionPart, RollbackError, SchemaError,
};
pub use settings::{
    BumpRevisionError, McpSecret, MutateMcpError, PromptVersionError, ReadDraftError,
    ReadSecretError, ReadSettingError, SavePromptError, SetDefaultModelError, SetDraftError,
    SetMcpSecretError, SetModelConfigError, SetModelGroupError, SetSecretError, SetSettingError,
    SetSkillEnabledError, SettingFailure,
};
pub use source_index::{MAX_INDEX_BYTES, SourceIndexError};
pub use sources::{
    CollectContentError, CollectionRangeError, ContentRangeError, ResolveTextError,
    SourceIndexPageError,
};
pub use text_search::TextSearchError;
pub use threads::{
    AgentThreadsError, AwaitsUserError, ChangesSinceError, CreateProjectError,
    CreateProjectFailure, InspectorError, LastReplyError, LatestRunError, MAX_LIVE_PROJECTS,
    MAX_PROJECT_NAME_BYTES, MAX_PROJECT_PATH_BYTES, MarkUnreadError, ReadProjectError,
    ReadRunError, ReadThreadError, RemoveProjectError, ReplaceTitleError, STATE_BYTES,
    SettleThreadError, StateError, ThreadTitlesError, ThreadsPageError, UpdateThreadError,
    UpsertRunError, WorkingAgentsError,
};
pub use writer::{
    InputFailure, MAX_MESSAGE_TEXT_BYTES, SETTINGS_BYTES, UpdateThreadFailure, WriteFailure,
};

type Result<T, E = StorageError> = std::result::Result<T, E>;

const CHANGES: feed::Budget = feed::Budget {
    bytes: 32 * 1024 * 1024,
    envelopes: 8192,
};
const READ_SLOTS: usize = 4;
const SCAN_SLOTS: usize = 2;
const READERS_KEPT: usize = READ_SLOTS + SCAN_SLOTS;
const WRITER_CACHE_KIB: i64 = 32 * 1024;
const WAL_BACKSTOP_PAGES: i64 = 16 * 1024;
const CHECKPOINT_AFTER_COMMITS: u64 = 512;
const CHECKPOINT_AFTER: Duration = Duration::from_secs(15);
const CHECKPOINT_RETRY: Duration = Duration::from_secs(1);
const SLOW_READ: Duration = Duration::from_millis(500);
const WRITE_QUEUE_CAPACITY: usize = 256;
const WRITE_QUEUE_BYTES: usize = 64 * 1024 * 1024;
const SMALL_WRITE_BYTES: usize = 4 * 1024;
const BLOB_UPLOADS: usize = 4;
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);
pub const CONTINUE_AFTER_RESTART_KEY: &str = "continue_after_restart";
pub const MAX_RESTART_CONTINUATIONS: usize = 3;
pub const RETRY_WHEN_OVERLOADED_KEY: &str = "retry_when_overloaded";
pub const CONTINUE_AFTER_USAGE_LIMIT_KEY: &str = "continue_after_usage_limit";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Park {
    pub continuation: artisan_api::Continuation,
    pub due_at: Timestamp,
}

#[must_use]
const fn park_setting(continuation: artisan_api::Continuation) -> Option<&'static str> {
    match continuation {
        artisan_api::Continuation::Overloaded => Some(RETRY_WHEN_OVERLOADED_KEY),
        artisan_api::Continuation::UsageLimit => Some(CONTINUE_AFTER_USAGE_LIMIT_KEY),
        artisan_api::Continuation::Requested | artisan_api::Continuation::Restart => None,
    }
}

#[derive(Clone, Debug)]
pub struct AgentReport {
    pub thread: ThreadId,
    pub message: artisan_api::SendMessage,
    pub acceptance: Acceptance,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Delivered {
    Initial,
    Steer,
}

#[derive(Clone, Debug)]
pub struct Settlement {
    pub run: Run,
    pub open: Vec<ItemId>,
    pub attempted: Vec<ItemId>,
    pub park: Option<Park>,
    pub note: Option<NewItem>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewItem {
    pub id: ItemId,
    pub thread_id: ThreadId,
    pub run_id: Option<RunId>,
    pub parent_id: Option<ItemId>,
    pub native_id: Option<String>,
    pub status: Status,
    pub kind: ItemKind,
}

#[derive(Debug, thiserror::Error)]
pub enum CreateThreadError {
    #[error("create thread {thread}: project {project} not found")]
    ProjectNotFound {
        thread: ThreadId,
        project: ProjectId,
    },
    #[error("create thread {thread}: project {project} was removed")]
    ProjectRemoved {
        thread: ThreadId,
        project: ProjectId,
    },
    #[error("create thread {thread}: parent thread {parent} not found")]
    ParentNotFound { thread: ThreadId, parent: ThreadId },
    #[error("create thread {thread}: agent {agent} needs a parent thread")]
    AgentWithoutParent { thread: ThreadId, agent: ItemId },
    #[error("create thread {thread}: {agent} is not an agent of parent thread {parent}")]
    AgentNotInParent {
        thread: ThreadId,
        parent: ThreadId,
        agent: ItemId,
    },
    #[error("create thread {thread}: the config sets a context window its engine does not offer")]
    InvalidConfig { thread: ThreadId },
    #[error("create thread {thread}")]
    Storage {
        thread: ThreadId,
        #[source]
        source: StorageError,
    },
}

#[derive(Debug, thiserror::Error)]
#[error("save run {run}: it belongs to thread {stored}, not {requested}")]
pub struct RunMoved {
    run: RunId,
    stored: ThreadId,
    requested: ThreadId,
}

#[derive(Debug, thiserror::Error)]
#[error("item {0} not found")]
pub struct ItemNotFound(ItemId);

#[derive(Debug, thiserror::Error)]
pub enum SendRejected {
    #[error("thread {0} not found")]
    ThreadNotFound(ThreadId),
    #[error("message id {0} was already used with different content or thread")]
    IdConflict(ItemId),
    #[error("the message text is {bytes} bytes; the limit is 32 MiB")]
    TextTooLarge { bytes: u64 },
    #[error("the message has {count} image references; the limit is 128")]
    TooManyImages { count: usize },
    #[error("image blob {blob} is {bytes} bytes; the limit is 32 MiB")]
    ImageTooLarge { blob: BlobHash, bytes: u64 },
    #[error("the message's images total {bytes} bytes; the limit is 128 MiB")]
    ImagesTooLarge { bytes: u64 },
    #[error("image blob {0} is missing or invalid")]
    MissingBlob(BlobHash),
    #[error("the message config sets a context window its engine does not offer")]
    InvalidConfig,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Snapshot {
    pub head: Seq,
    pub projects: Vec<Project>,
    pub threads: Vec<Thread>,
    pub threads_next: Option<artisan_api::ThreadCursor>,
    pub runs: Vec<Run>,
    pub settings: serde_json::Map<String, serde_json::Value>,
    pub database_rollback: Option<artisan_api::DatabaseRollback>,
}

#[derive(Clone)]
pub struct Store {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Store")
            .field("database", &self.inner.database)
            .field("head", &self.inner.head)
            .finish_non_exhaustive()
    }
}

struct Inner {
    jobs: mpsc::Sender<Job>,
    queued_write_bytes: Arc<Semaphore>,
    events: feed::Feed,
    local_live: feed::Feed,
    coordination: watch::Sender<()>,
    revocations: broadcast::Sender<DeviceId>,
    head: Arc<AtomicU64>,
    live_revision: AtomicU64,
    reads: AtomicU64,
    commits: Arc<AtomicU64>,
    readers: Mutex<Vec<Connection>>,
    read_slots: Arc<Semaphore>,
    scan_slots: Arc<Semaphore>,
    hot: Arc<hot::HotPages>,
    index_build: Arc<Semaphore>,
    recall_index: watch::Receiver<RecallIndex>,
    authorizer: Authorizer,
    device_watch: pairing::DeviceWatch,
    database: PathBuf,
    connections: Connections,
    clock: artisan_api::WallClock,
    blob_dir: PathBuf,
    resource_dir: PathBuf,
    blob_lock: Arc<tokio::sync::Mutex<()>>,
    blob_uploads: Arc<Semaphore>,
    _threads: Threads,
}

#[derive(Default)]
struct Threads {
    recall: Option<recall::Builder>,
    writer: Option<std::thread::JoinHandle<()>>,
    checkpointer: Option<std::thread::JoinHandle<()>>,
}

impl Threads {
    fn join(&mut self) -> Result<(), ShutdownError> {
        let here = std::thread::current().id();
        if let Some(recall) = self.recall.take() {
            recall
                .stop()
                .map_err(|_panic| ShutdownError::RecallBuilderPanicked)?;
        }
        for (thread, panicked) in [
            (self.writer.take(), ShutdownError::WriterPanicked),
            (
                self.checkpointer.take(),
                ShutdownError::CheckpointerPanicked,
            ),
        ] {
            if let Some(thread) = thread.filter(|thread| thread.thread().id() != here) {
                thread.join().map_err(|_panic| panicked)?;
            }
        }
        Ok(())
    }
}

impl Drop for Threads {
    fn drop(&mut self) {
        if let Err(error) = self.join() {
            tracing::error!(error = %error, "close the Forge store");
        }
    }
}

fn open_database(connections: &Connections, path: &Path) -> Result<Connection> {
    let opened = connections.open(path).and_then(|conn| {
        conn.busy_timeout(BUSY_TIMEOUT)?;
        Ok(conn)
    });
    opened.map_err(|source| StorageError::Open {
        database: path.to_path_buf(),
        source: Box::new(source),
    })
}

fn start_devices(
    connections: &Connections,
    database: &Path,
    revocations: broadcast::Sender<DeviceId>,
) -> Result<(Authorizer, pairing::DeviceWatch), OpenStoreError> {
    let prepare = |source| OpenStoreError::Database {
        database: database.to_path_buf(),
        source,
    };
    let devices =
        Devices::open(open_database(connections, database).map_err(prepare)?).map_err(prepare)?;
    let watch = pairing::DeviceWatch::start(database).map_err(OpenStoreError::WatchDevices)?;
    Ok((Authorizer::new(devices, revocations), watch))
}

#[derive(Debug, thiserror::Error)]
pub enum OpenStoreError {
    #[error("secure the database in {}", dir.display())]
    Secure {
        dir: PathBuf,
        #[source]
        source: SecureDatabaseError,
    },
    #[error("create the blob folder {}", path.display())]
    BlobFolder {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("name the blob files in {}", path.display())]
    NameBlobs {
        path: PathBuf,
        #[source]
        source: BlobFileError,
    },
    #[error("roll {} back to a schema this Forge supports", database.display())]
    RollBack {
        database: PathBuf,
        #[source]
        source: Box<RollbackError>,
    },
    #[error("upgrade {}", database.display())]
    Migrate {
        database: PathBuf,
        #[source]
        source: Box<SchemaError>,
    },
    #[error("there is no running Forge store to join at {}", database.display())]
    NotRunning { database: PathBuf },
    #[error(
        "{} is at schema {found} but this build works with schema {supported}; start Forge so it upgrades the database first",
        database.display()
    )]
    Mismatch {
        database: PathBuf,
        found: usize,
        supported: usize,
    },
    #[error("record that {} was rolled back", database.display())]
    RecordRollback {
        database: PathBuf,
        #[source]
        source: StorageError,
    },
    #[error("read whether a rollback of {} is still to be reported", database.display())]
    PendingRollback {
        database: PathBuf,
        #[source]
        source: Box<RetainedError>,
    },
    #[error(
        "online upgrade requires database schema {expected}, the running store uses {found}; the current Forge was left running"
    )]
    Schema { expected: usize, found: usize },
    #[error("{} refused WAL journaling (mode {mode})", database.display())]
    Wal { database: PathBuf, mode: String },
    #[error("watch for device changes from other processes")]
    WatchDevices(#[source] pairing::WatchDevicesError),
    #[error("start the store's {thread} thread")]
    Spawn {
        thread: &'static str,
        #[source]
        source: std::io::Error,
    },
    #[error("recover the runs the last Forge left live")]
    Recover(#[source] WriteFailure),
    #[error("prepare the recall index of {}", database.display())]
    PrepareRecall {
        database: PathBuf,
        #[source]
        source: Box<PrepareIndexError>,
    },
    #[error("read the recall index state of {}", database.display())]
    RecallState {
        database: PathBuf,
        #[source]
        source: StorageError,
    },
    #[error(
        "the recall index of {} is at version {built:?}, not {expected}; start the Forge that owns it so it rebuilds the index",
        database.display()
    )]
    RecallStale {
        database: PathBuf,
        built: Option<i64>,
        expected: i64,
    },
    #[error("prepare the database {}", database.display())]
    Database {
        database: PathBuf,
        #[source]
        source: StorageError,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum ShutdownError {
    #[error("the Forge store still has live handles")]
    Shared,
    #[error("the Forge store writer panicked")]
    WriterPanicked,
    #[error("the Forge store checkpointer panicked")]
    CheckpointerPanicked,
    #[error("the Forge store's recall index builder panicked")]
    RecallBuilderPanicked,
}

error::operation_error!(SynchronizeError(StorageError), "catch up from seq {cursor}", cursor: Seq);
error::operation_error!(RecoverThreadError(WriteFailure), "recover thread {thread}", thread: ThreadId);
error::operation_error!(
    RunningThreadsError(StorageError),
    "list the threads with live runs"
);

#[derive(Debug, thiserror::Error)]
pub enum SecureDatabaseError {
    #[error("making Forge's data folder private")]
    Folder(#[source] artisan_paths::private::PrivateFileError),
    #[error("creating the private database {database}")]
    Create {
        database: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("listing the database files in {folder}")]
    List {
        folder: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("making the database file private")]
    Restrict(#[source] artisan_paths::private::PrivateFileError),
}

fn secure_database(dir: &Path, database: &Path) -> Result<(), SecureDatabaseError> {
    artisan_paths::private::create_folder(dir).map_err(SecureDatabaseError::Folder)?;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(false);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options
        .open(database)
        .map_err(|source| SecureDatabaseError::Create {
            database: database.to_path_buf(),
            source,
        })?;
    let family = database
        .file_name()
        .expect("invariant: the database path ends in its file name")
        .to_os_string();
    let listed = |source| SecureDatabaseError::List {
        folder: dir.to_path_buf(),
        source,
    };
    for entry in std::fs::read_dir(dir).map_err(listed)? {
        let entry = entry.map_err(listed)?;
        let is_file = entry.file_type().map_err(listed)?.is_file();
        if is_file
            && entry
                .file_name()
                .as_encoded_bytes()
                .starts_with(family.as_encoded_bytes())
        {
            artisan_paths::private::restrict_file(&entry.path())
                .map_err(SecureDatabaseError::Restrict)?;
        }
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum Opening {
    Upgrade,
    Join,
}

#[derive(Clone, Copy)]
enum Opened {
    Upgraded { from: usize },
    Joined,
}

fn recover_after_opening(
    writer: &mut Writer,
    opened: Opened,
    database: &Path,
) -> Result<(), OpenStoreError> {
    let from = match opened {
        Opened::Joined => return Ok(()),
        Opened::Upgraded { from } => from,
    };
    writer
        .transact(Durability::Background, |tx| tx.recover_threads(None))
        .map_err(OpenStoreError::Recover)?;
    let pending = |source| OpenStoreError::PendingRollback {
        database: database.to_owned(),
        source: Box::new(source),
    };
    let Some(rolled_back) = retained::pending_notice(database).map_err(pending)? else {
        return Ok(());
    };
    if u32::try_from(from).is_ok_and(|from| from < rolled_back.found) {
        writer
            .transact(Durability::Acknowledged, |tx| {
                tx.record_rollback(&rolled_back)
            })
            .map_err(|source| OpenStoreError::RecordRollback {
                database: database.to_owned(),
                source,
            })?;
    } else {
        tracing::warn!(
            database = %database.display(),
            found = rolled_back.found,
            restored = rolled_back.restored,
            opened_at = from,
            "a rollback was started but the database was never swapped; nothing was rolled back"
        );
    }
    retained::release_notice(database).map_err(pending)
}

fn stored_version(connections: &Connections, database: &Path) -> Result<usize, OpenStoreError> {
    let prepare = |source| OpenStoreError::Database {
        database: database.to_owned(),
        source,
    };
    open_database(connections, database)
        .map_err(prepare)?
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .at("read the schema version")
        .map_err(prepare)
}

fn writer_connection(
    connections: &Connections,
    database: &Path,
) -> Result<Connection, OpenStoreError> {
    let prepare = |source| OpenStoreError::Database {
        database: database.to_owned(),
        source,
    };
    let conn = open_database(connections, database).map_err(prepare)?;
    let mode: String = conn
        .pragma_update_and_check(None, "journal_mode", "WAL", |row| row.get(0))
        .at("switch to write-ahead logging")
        .map_err(prepare)?;
    if !mode.eq_ignore_ascii_case("wal") {
        return Err(OpenStoreError::Wal {
            database: database.to_owned(),
            mode,
        });
    }
    conn.pragma_update(None, "cache_size", -WRITER_CACHE_KIB)
        .at("size the writer's page cache")
        .map_err(prepare)?;
    conn.pragma_update(None, "wal_autocheckpoint", WAL_BACKSTOP_PAGES)
        .at("set the checkpoint backstop")
        .map_err(prepare)?;
    Ok(conn)
}

fn upgrade(
    conn: &mut Connection,
    database: &Path,
    upgrade: schema::Upgrade,
) -> Result<(), OpenStoreError> {
    schema::migrate(conn, upgrade).map_err(|source| OpenStoreError::Migrate {
        database: database.to_owned(),
        source: Box::new(source),
    })?;
    let progress = |source| OpenStoreError::Database {
        database: database.to_owned(),
        source,
    };
    progress::watch(conn, progress::Step::RecallIndex, upgrade.every)
        .at("watch the recall index preparation")
        .map_err(progress)?;
    let prepared = recall::prepare_index(conn);
    progress::unwatch(conn)
        .at("stop watching the recall index preparation")
        .map_err(progress)?;
    prepared
        .map(drop)
        .map_err(|source| OpenStoreError::PrepareRecall {
            database: database.to_owned(),
            source: Box::new(source),
        })
}

fn recall_builder(
    conn: &Connection,
    jobs: &mpsc::Sender<Job>,
    database: &Path,
) -> Result<(Option<recall::Builder>, watch::Receiver<RecallIndex>), OpenStoreError> {
    let state = recall::index_state(conn).map_err(|source| OpenStoreError::RecallState {
        database: database.to_owned(),
        source,
    })?;
    match state {
        recall::IndexState::Building { .. } => {
            let (status, watching) = watch::channel(RecallIndex::Building);
            recall::Builder::spawn(jobs.clone(), status)
                .map(|builder| (Some(builder), watching))
                .map_err(|source| OpenStoreError::Spawn {
                    thread: "recall index builder",
                    source,
                })
        }
        recall::IndexState::Built => Ok((None, watch::channel(RecallIndex::Built).1)),
        recall::IndexState::Stale { built } => Err(OpenStoreError::RecallStale {
            database: database.to_owned(),
            built,
            expected: recall::INDEX_VERSION,
        }),
    }
}

#[derive(Debug)]
pub struct PendingInputs {
    pub items: Vec<Item>,
    pub cutoff: Seq,
}

impl Store {
    fn prepare(dir: &Path) -> Result<(PathBuf, PathBuf), OpenStoreError> {
        let database = dir.join("forge.sqlite3");
        secure_database(dir, &database).map_err(|source| OpenStoreError::Secure {
            dir: dir.to_path_buf(),
            source,
        })?;
        let blob_dir = dir.join("blobs");
        std::fs::create_dir_all(&blob_dir).map_err(|source| OpenStoreError::BlobFolder {
            path: blob_dir.clone(),
            source,
        })?;
        blobs::name_files(&blob_dir).map_err(|source| OpenStoreError::NameBlobs {
            path: blob_dir.clone(),
            source,
        })?;
        Ok((database, blob_dir))
    }

    pub fn open(dir: &Path) -> Result<Self, OpenStoreError> {
        Self::open_in(dir, Environment::system())
    }

    pub fn open_in(dir: &Path, environment: Environment) -> Result<Self, OpenStoreError> {
        let (database, blob_dir) = Self::prepare(dir)?;
        Self::start(
            database,
            blob_dir,
            dir.join("resources"),
            Opening::Upgrade,
            environment,
        )
    }

    pub fn open_restoring(dir: &Path, data_lock: &DataLock) -> Result<Self, OpenStoreError> {
        Self::open_restoring_in(dir, data_lock, Environment::system())
    }

    pub fn open_restoring_in(
        dir: &Path,
        data_lock: &DataLock,
        environment: Environment,
    ) -> Result<Self, OpenStoreError> {
        data_lock.guards(dir);
        let (database, blob_dir) = Self::prepare(dir)?;
        match schema::roll_back(
            &database,
            environment.clock.now(),
            environment.progress_every,
        )
        .map_err(|source| OpenStoreError::RollBack {
            database: database.clone(),
            source: Box::new(source),
        })? {
            schema::Rollback::Current => {}
            schema::Rollback::Restored {
                found,
                restored,
                aside,
            } => tracing::warn!(
                database = %database.display(),
                found,
                restored,
                aside = %aside.display(),
                "rolled the database back to a schema this Forge supports; newer data was set aside"
            ),
        }
        Self::start(
            database,
            blob_dir,
            dir.join("resources"),
            Opening::Upgrade,
            environment,
        )
    }

    pub fn open_current(dir: &Path) -> Result<Self, OpenStoreError> {
        let database = dir.join("forge.sqlite3");
        if database.is_file() {
            match stored_version(&Connections::system(), &database)? {
                0 => {}
                found if found == schema::version() => {}
                found => {
                    return Err(OpenStoreError::Mismatch {
                        database,
                        found,
                        supported: schema::version(),
                    });
                }
            }
        }
        Self::open(dir)
    }

    pub fn join(dir: &Path) -> Result<Self, OpenStoreError> {
        Self::join_in(dir, Environment::system())
    }

    pub fn join_in(dir: &Path, environment: Environment) -> Result<Self, OpenStoreError> {
        let database = dir.join("forge.sqlite3");
        if !database.is_file() {
            return Err(OpenStoreError::NotRunning { database });
        }
        let found = stored_version(&environment.connections, &database)?;
        if found != schema::version() {
            return Err(OpenStoreError::Schema {
                expected: schema::version(),
                found,
            });
        }
        Self::start(
            database,
            dir.join("blobs"),
            dir.join("resources"),
            Opening::Join,
            environment,
        )
    }

    #[must_use]
    pub fn now(&self) -> Timestamp {
        self.inner.clock.now()
    }

    pub const fn schema_version() -> usize {
        schema::version()
    }

    fn start(
        database: PathBuf,
        blob_dir: PathBuf,
        resource_dir: PathBuf,
        opening: Opening,
        environment: Environment,
    ) -> Result<Self, OpenStoreError> {
        let upgrading = schema::Upgrade::of(&environment);
        let Environment {
            connections, clock, ..
        } = environment;
        let prepare = |source| OpenStoreError::Database {
            database: database.clone(),
            source,
        };
        let mut conn = writer_connection(&connections, &database)?;
        let opened = match opening {
            Opening::Upgrade => {
                let from = conn
                    .pragma_query_value(None, "user_version", |row| row.get(0))
                    .at("read the schema version before upgrading")
                    .map_err(prepare)?;
                upgrade(&mut conn, &database, upgrading)?;
                Opened::Upgraded { from }
            }
            Opening::Join => Opened::Joined,
        };

        let events = feed::Feed::new(CHANGES);
        let head = Arc::new(AtomicU64::new(0));
        let hot = Arc::new(hot::HotPages::default());
        let commits = Arc::new(AtomicU64::new(0));
        let (committed, wakes) = std::sync::mpsc::sync_channel(1);
        let mut writer = Writer::new(
            conn,
            Arc::clone(&head),
            matches!(opened, Opened::Upgraded { .. }).then(|| events.clone()),
            Arc::clone(&hot),
            writer::Commits {
                count: Arc::clone(&commits),
                committed,
            },
            clock.clone(),
        )
        .map_err(prepare)?;
        recover_after_opening(&mut writer, opened, &database)?;

        let mut threads = Threads::default();
        let (jobs, queue) = mpsc::channel::<Job>(WRITE_QUEUE_CAPACITY);
        let (builder, recall_index) = recall_builder(writer.connection(), &jobs, &database)?;
        threads.writer = Some(
            std::thread::Builder::new()
                .name("forge-store-writer".into())
                .spawn(move || writer.run(queue))
                .map_err(|source| OpenStoreError::Spawn {
                    thread: "writer",
                    source,
                })?,
        );
        threads.recall = builder;

        let checkpoint_conn = open_database(&connections, &database).map_err(prepare)?;
        threads.checkpointer = Some(
            std::thread::Builder::new()
                .name("forge-store-checkpoint".into())
                .spawn({
                    let commits = Arc::clone(&commits);
                    move || checkpoint_when_due(&checkpoint_conn, &commits, &wakes)
                })
                .map_err(|source| OpenStoreError::Spawn {
                    thread: "checkpointer",
                    source,
                })?,
        );

        let first_reader = open_database(&connections, &database).map_err(prepare)?;
        let revocations = broadcast::channel(64).0;
        let (authorizer, device_watch) =
            start_devices(&connections, &database, revocations.clone())?;
        Ok(Self {
            inner: Arc::new(Inner {
                jobs,
                queued_write_bytes: Arc::new(Semaphore::new(WRITE_QUEUE_BYTES)),
                events,
                local_live: feed::Feed::new(CHANGES),
                coordination: watch::Sender::new(()),
                revocations,
                head,
                live_revision: AtomicU64::new(0),
                reads: AtomicU64::new(0),
                commits,
                readers: Mutex::new(vec![first_reader]),
                read_slots: Arc::new(Semaphore::new(READ_SLOTS)),
                scan_slots: Arc::new(Semaphore::new(SCAN_SLOTS)),
                hot,
                index_build: Arc::new(Semaphore::new(1)),
                recall_index,
                authorizer,
                device_watch,
                database,
                connections,
                clock,
                blob_dir,
                resource_dir,
                blob_lock: Arc::new(tokio::sync::Mutex::new(())),
                blob_uploads: Arc::new(Semaphore::new(BLOB_UPLOADS)),
                _threads: threads,
            }),
        })
    }

    #[cfg(feature = "qualification")]
    pub fn qualification_is_unique(&self) -> bool {
        Arc::strong_count(&self.inner) == 1
    }

    #[cfg(feature = "qualification")]
    pub fn shutdown_unique(self) -> Result<(), ShutdownError> {
        let inner = Arc::try_unwrap(self.inner).map_err(|_shared| ShutdownError::Shared)?;
        let Inner {
            jobs,
            readers,
            authorizer,
            _threads: mut threads,
            ..
        } = inner;
        drop(readers);
        drop(authorizer);
        drop(jobs);
        threads.join()
    }

    #[must_use]
    pub fn recall_index(&self) -> watch::Receiver<RecallIndex> {
        self.inner.recall_index.clone()
    }

    #[must_use]
    pub const fn recall_index_version() -> i64 {
        recall::INDEX_VERSION
    }

    #[must_use]
    pub fn subscribe(&self) -> Subscription {
        self.inner.events.subscribe()
    }

    #[must_use]
    pub fn retained_change_bytes(&self) -> usize {
        self.inner.events.retained_bytes()
    }

    #[must_use]
    pub fn retained_changes(&self) -> usize {
        self.inner.events.retained_envelopes()
    }

    pub fn publish_live(&self, event: Event) {
        self.inner.local_live.send(Envelope {
            seq: None,
            live_revision: 0,
            event: event.clone(),
        });
        self.relay_live(event);
    }

    #[must_use]
    pub fn subscribe_local_live(&self) -> Subscription {
        self.inner.local_live.subscribe()
    }

    pub fn relay_live(&self, event: Event) {
        let live_revision = self.inner.live_revision.fetch_add(1, Ordering::AcqRel) + 1;
        self.inner.events.send(Envelope {
            seq: None,
            live_revision,
            event,
        });
    }

    #[must_use]
    pub fn watch_coordination(&self) -> watch::Receiver<()> {
        self.inner.coordination.subscribe()
    }

    fn coordination_changed(&self) {
        self.inner.coordination.send_replace(());
    }

    #[must_use]
    pub fn reads(&self) -> u64 {
        self.inner.reads.load(Ordering::Acquire)
    }

    #[must_use]
    pub fn commits(&self) -> u64 {
        self.inner.commits.load(Ordering::Acquire)
    }

    #[must_use]
    pub fn queued_write_bytes(&self) -> usize {
        WRITE_QUEUE_BYTES - self.inner.queued_write_bytes.available_permits()
    }

    #[must_use]
    pub fn live_revision(&self) -> u64 {
        self.inner.live_revision.load(Ordering::Acquire)
    }

    #[must_use]
    pub fn head(&self) -> Seq {
        self.inner.head.load(Ordering::Acquire)
    }

    pub async fn synchronize(&self, cursor: &mut Seq) -> Result<(), SynchronizeError> {
        let from = *cursor;
        let failed = |cause| SynchronizeError {
            cursor: from,
            cause: Box::new(cause),
        };
        let changes = self
            .read(move |conn| read::changes_since(conn, from, 20_000, false, None))
            .await
            .map_err(failed)?;
        if let Some(changes) = changes {
            self.inner.hot.committed(&changes);
            if !changes.is_empty() {
                self.coordination_changed();
            }
            for envelope in changes {
                if let Some(seq) = envelope.seq {
                    *cursor = (*cursor).max(seq);
                }
                self.inner.events.send(envelope);
            }
        } else {
            *cursor = self.read(read::max_seq).await.map_err(failed)?;
            self.inner.hot.reset();
            self.coordination_changed();
            self.relay_live(Event::Reset);
        }
        self.inner.head.fetch_max(*cursor, Ordering::AcqRel);
        Ok(())
    }

    pub async fn recover_thread(&self, thread: ThreadId) -> Result<(), RecoverThreadError> {
        let recovered = thread.clone();
        self.write(move |tx| tx.recover_threads(Some(vec![recovered])))
            .await
            .map_err(|cause| RecoverThreadError {
                thread,
                cause: Box::new(cause),
            })
    }

    pub async fn running_threads(&self) -> Result<Vec<ThreadId>, RunningThreadsError> {
        self.read(|conn| {
            let mut statement = conn
                .prepare(include_str!("sql/recovery/threads.sql"))
                .at("recovery/threads.sql")?;
            statement
                .query_map([], |row| Ok(ThreadId::from(row.get::<_, String>(0)?)))
                .at("recovery/threads.sql")?
                .collect::<rusqlite::Result<Vec<_>>>()
                .at("recovery/threads.sql")
        })
        .await
        .map_err(|cause| RunningThreadsError {
            cause: Box::new(cause),
        })
    }

    async fn write<R, E, F>(&self, job: F) -> Result<R, E>
    where
        R: Send + 'static,
        E: From<StorageError> + Send + 'static,
        F: FnOnce(&mut Tx<'_>) -> Result<R, E> + Send + 'static,
    {
        self.write_with(Durability::Background, SMALL_WRITE_BYTES, job)
            .await
    }

    async fn write_carrying<R, E, F>(&self, bytes: usize, job: F) -> Result<R, E>
    where
        R: Send + 'static,
        E: From<StorageError> + Send + 'static,
        F: FnOnce(&mut Tx<'_>) -> Result<R, E> + Send + 'static,
    {
        self.write_with(Durability::Background, bytes, job).await
    }

    async fn write_acknowledged<R, E, F>(&self, job: F) -> Result<R, E>
    where
        R: Send + 'static,
        E: From<StorageError> + Send + 'static,
        F: FnOnce(&mut Tx<'_>) -> Result<R, E> + Send + 'static,
    {
        self.write_with(Durability::Acknowledged, SMALL_WRITE_BYTES, job)
            .await
    }

    async fn write_acknowledged_carrying<R, E, F>(&self, bytes: usize, job: F) -> Result<R, E>
    where
        R: Send + 'static,
        E: From<StorageError> + Send + 'static,
        F: FnOnce(&mut Tx<'_>) -> Result<R, E> + Send + 'static,
    {
        self.write_with(Durability::Acknowledged, bytes, job).await
    }

    async fn write_with<R, E, F>(
        &self,
        durability: Durability,
        bytes: usize,
        job: F,
    ) -> Result<R, E>
    where
        R: Send + 'static,
        E: From<StorageError> + Send + 'static,
        F: FnOnce(&mut Tx<'_>) -> Result<R, E> + Send + 'static,
    {
        let charged = u32::try_from(bytes.clamp(SMALL_WRITE_BYTES, WRITE_QUEUE_BYTES))
            .expect("invariant: the write queue's byte budget fits a semaphore");
        let permit = Arc::clone(&self.inner.queued_write_bytes)
            .acquire_many_owned(charged)
            .await
            .map_err(|_closed| StorageError::WriterStopped)?;
        let (reply, result) = oneshot::channel();
        self.inner
            .jobs
            .send(Box::new(move |writer: &mut Writer| {
                let outcome = writer.transact(durability, job);
                drop(permit);
                let _requester_gone = reply.send(outcome);
            }))
            .await
            .map_err(|_unsent_job| StorageError::WriterStopped)?;
        result.await.map_err(StorageError::WriteDropped)?
    }

    async fn tail_page<E, F>(
        &self,
        thread: &ThreadId,
        tail: Option<hot::Tail>,
        query: F,
    ) -> Result<artisan_api::ItemsPage, E>
    where
        E: From<StorageError> + Send + 'static,
        F: FnOnce(&Connection) -> Result<artisan_api::ItemsPage, E> + Send + 'static,
    {
        let Some(tail) = tail else {
            return self.read(query).await;
        };
        match self.inner.hot.lookup(thread, &tail) {
            hot::Lookup::Hit(page) => Ok(artisan_api::ItemsPage::clone(&page)),
            hot::Lookup::Miss(read_at) => {
                let page = self.read(query).await?;
                self.inner.hot.hold(thread, tail, read_at, &page);
                Ok(page)
            }
        }
    }

    async fn read<R, E, F>(&self, query: F) -> Result<R, E>
    where
        R: Send + 'static,
        E: From<StorageError> + Send + 'static,
        F: FnOnce(&Connection) -> Result<R, E> + Send + 'static,
    {
        self.read_in(&self.inner.read_slots, query).await
    }

    async fn scan<R, E, F>(&self, query: F) -> Result<R, E>
    where
        R: Send + 'static,
        E: From<StorageError> + Send + 'static,
        F: FnOnce(&Connection) -> Result<R, E> + Send + 'static,
    {
        self.read_in(&self.inner.scan_slots, query).await
    }

    async fn read_in<R, E, F>(&self, slots: &Arc<Semaphore>, query: F) -> Result<R, E>
    where
        R: Send + 'static,
        E: From<StorageError> + Send + 'static,
        F: FnOnce(&Connection) -> Result<R, E> + Send + 'static,
    {
        self.inner.reads.fetch_add(1, Ordering::AcqRel);
        let inner = Arc::clone(&self.inner);
        let asked = std::time::Instant::now();
        let slot = Arc::clone(slots)
            .acquire_owned()
            .await
            .map_err(StorageError::ReadSlot)?;
        let queued = asked.elapsed();
        tokio::task::spawn_blocking(move || {
            let _slot = slot;
            let started = asked.elapsed();
            let result = inner.with_reader(query);
            let total = asked.elapsed();
            if total >= SLOW_READ {
                tracing::warn!(
                    read = std::any::type_name::<F>(),
                    waited_for_slot_ms = queued.as_millis(),
                    waited_for_thread_ms = started.saturating_sub(queued).as_millis(),
                    ran_ms = total.saturating_sub(started).as_millis(),
                    "store read was slow"
                );
            }
            result
        })
        .await
        .map_err(StorageError::Task)?
    }
}

impl Inner {
    fn with_reader<R, E: From<StorageError>>(
        &self,
        query: impl FnOnce(&Connection) -> Result<R, E>,
    ) -> Result<R, E> {
        let pooled = self
            .readers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pop();
        let conn = match pooled {
            Some(conn) => conn,
            None => open_database(&self.connections, &self.database)?,
        };
        let result = query(&conn);
        let mut readers = self.readers.lock().unwrap_or_else(PoisonError::into_inner);
        if readers.len() < READERS_KEPT {
            readers.push(conn);
        }
        result
    }
}

fn checkpoint_when_due(
    conn: &Connection,
    commits: &AtomicU64,
    wakes: &std::sync::mpsc::Receiver<()>,
) {
    use std::sync::mpsc::RecvTimeoutError;
    let mut checkpointed = commits.load(Ordering::Acquire);
    let mut waiting_since = None;
    loop {
        let committed = commits.load(Ordering::Acquire);
        let pending = committed.saturating_sub(checkpointed);
        if pending == 0 {
            waiting_since = None;
            if wakes.recv().is_err() {
                return;
            }
            continue;
        }
        let since = *waiting_since.get_or_insert_with(std::time::Instant::now);
        let due_in = CHECKPOINT_AFTER.saturating_sub(since.elapsed());
        if pending < CHECKPOINT_AFTER_COMMITS && !due_in.is_zero() {
            match wakes.recv_timeout(due_in) {
                Ok(()) | Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
        let progress = conn.query_row(include_str!("sql/wal/checkpoint.sql"), [], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
            ))
        });
        match progress {
            Ok((0, log_frames, checkpointed_frames)) if checkpointed_frames >= log_frames => {
                checkpointed = committed;
                waiting_since = None;
                continue;
            }
            Ok((busy, log_frames, checkpointed_frames)) => tracing::debug!(
                busy,
                log_frames,
                checkpointed_frames,
                pending,
                "store checkpoint was partial; it stays pending"
            ),
            Err(error) => tracing::warn!(pending, "checkpoint the store: {error}"),
        }
        if matches!(
            wakes.recv_timeout(CHECKPOINT_RETRY),
            Err(RecvTimeoutError::Disconnected)
        ) {
            return;
        }
    }
}

fn fresh_item(new: NewItem, created_at: Timestamp) -> Item {
    Item {
        id: new.id,
        thread_id: new.thread_id,
        position: 0,
        seq: 0,
        run_id: new.run_id,
        parent_id: new.parent_id,
        status: new.status,
        created_at,
        updated_at: created_at,
        kind: new.kind,
    }
}

fn project_name(path: &str) -> String {
    Path::new(path).file_name().map_or_else(
        || path.to_owned(),
        |name| name.to_string_lossy().into_owned(),
    )
}
