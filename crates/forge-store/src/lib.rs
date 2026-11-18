#![expect(
    clippy::field_scoped_visibility_modifiers,
    clippy::iter_over_hash_type,
    clippy::needless_pass_by_ref_mut,
    clippy::renamed_function_params,
    clippy::string_slice,
    clippy::unused_result_ok,
    missing_debug_implementations,
    unreachable_pub,
    unused_qualifications,
    reason = "pre-existing violations of the workspace lint policy, cleared in plan.md phase 2"
)]
#![allow(
    clippy::assertions_on_result_states,
    clippy::rest_pat_in_fully_bound_structs,
    reason = "pre-existing violations that only occur in some builds, cleared in plan.md phase 2"
)]

pub mod admin;
mod blobs;
mod collection;
mod content;
pub mod coordination;
pub mod discovery;
mod error;
pub mod hot;
mod items;
mod pairing;
mod read;
mod recall;
mod resources;
mod retention;
mod schema;
mod settings;
mod source_index;
mod sources;
#[cfg(test)]
mod stamped_writes;
mod text_search;
mod threads;
mod writer;

use artisan_api::{
    BlobHash, DeviceId, Envelope, Event, Item, ItemId, ItemKind, Project, ProjectId, Run, RunId,
    Seq, Status, Thread, ThreadId, Timestamp,
};
use pairing::Devices;
use rusqlite::Connection;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;
use tokio::sync::{Semaphore, broadcast, mpsc, oneshot};
use writer::{Durability, Job, Tx, Writer};

pub use blobs::{
    BlobFailure, BlobFileError, BlobRejected, CollectBlobsError, InputImagesError, MAX_BLOB_BYTES,
    PutBlobError, PutImageError,
};
pub use collection::AnswerRefused;
pub use content::{ContentError, Damage};
pub use error::StorageError;
pub use items::{
    AcceptInputsError, Acceptance, AgentByAddressError, AnswerFailure, AnswerQuestionError,
    AwaitingItemsError, BeginRunError, CancelInputsError, CancelledItemsError,
    ContinuationChainError, ContinuePendingError, InsertMessageError, ItemByNativeError,
    ItemsPageError, NativeIdError, NextParkedError, PendingInputsError, PendingThreadsError,
    ReadContinueError, ReadItemError, ReopenAgentError, RequestContinueError,
    ResolveCollectionsError, RestoreInputsError, RunnableInputError, SendMessageError,
    SetItemStatusError, SettleAgentError, SettleRunError, SpawnAgentError, SpawnFailure,
    TakeContinueError, TranscriptPageError, TranscriptRowsError, UnparkError, UpsertItemError,
    ValidateAnswersError, WorkPageError,
};
pub use pairing::{
    AddDeviceError, AddPairCodeError, AuthorizeError, Device, PairDeviceError, RevokeDeviceError, TouchDeviceError,
};
pub use read::ThreadsPageFailure;
pub use recall::{
    GrepFailure, GrepMatch, GrepPage, GrepQuery, RecallGrepError, RecallScope, RecallSearchError,
    RecallTouchingError, SearchHit, SearchPage, Touch,
};
pub use resources::{
    CollectSnapshotsError, MAX_RESOURCE_SNAPSHOT_BYTES, ResourceFailure,
    keep as keep_resource_snapshot,
};
pub use retention::RemoveInactiveError;
pub use schema::{BackupError, MigrationFailure, QuestionPart, RollbackError, SchemaError};
pub use settings::{
    BumpRevisionError, McpSecret, MutateMcpError, PromptVersionError, ReadDraftError,
    ReadSecretError, ReadSettingError, SavePromptError, SetDefaultModelError, SetDraftError,
    SetMcpSecretError, SetModelConfigError, SetModelGroupError, SetSecretError, SetSettingError,
    SetSkillEnabledError, SettingFailure,
};
pub use source_index::SourceIndexError;
pub use sources::{
    CollectContentError, CollectionRangeError, ContentRangeError, ResolveTextError,
    SourceIndexPageError,
};
pub use text_search::TextSearchError;
pub use threads::{
    AgentThreadsError, AwaitsUserError, ChangesSinceError, CreateProjectError,
    InspectorError, LastReplyError, LatestRunError, MarkUnreadError, ReadProjectError,
    ReadRunError, ReadThreadError, RemoveProjectError, ReplaceTitleError, SettleThreadError, StateError, ThreadTitlesError, ThreadsPageError, UpdateThreadError,
    UpsertRunError, WorkingAgentsError,
};
pub use writer::{InputFailure, UpdateThreadFailure, WriteFailure};

type Result<T, E = StorageError> = std::result::Result<T, E>;

const EVENT_CAPACITY: usize = 8192;
const READ_SLOTS: usize = 4;
const SCAN_SLOTS: usize = 2;
const READERS_KEPT: usize = READ_SLOTS + SCAN_SLOTS;
const WRITER_CACHE_KIB: i64 = 32 * 1024;
const WAL_BACKSTOP_PAGES: i64 = 16 * 1024;
const CHECKPOINT_AFTER_COMMITS: u64 = 512;
const CHECKPOINT_AFTER: Duration = Duration::from_secs(15);
const SLOW_READ: std::time::Duration = std::time::Duration::from_millis(500);
const WRITE_QUEUE_CAPACITY: usize = 256;
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);
pub const CONTINUE_AFTER_RESTART_KEY: &str = "continue_after_restart";
pub const MAX_RESTART_CONTINUATIONS: usize = 3;
pub const RETRY_WHEN_OVERLOADED_KEY: &str = "retry_when_overloaded";
pub const CONTINUE_AFTER_USAGE_LIMIT_KEY: &str = "continue_after_usage_limit";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Park {
    pub continuation: artisan_api::Continuation,
    pub due_at: artisan_api::Timestamp,
}

#[must_use]
pub const fn park_setting(continuation: artisan_api::Continuation) -> Option<&'static str> {
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
    pub steer: bool,
    pub acceptance: Acceptance,
}

#[derive(Clone, Debug)]
pub struct Settlement {
    pub run: Run,
    pub open: Vec<ItemId>,
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
    pub run: RunId,
    pub stored: ThreadId,
    pub requested: ThreadId,
}

#[derive(Debug, thiserror::Error)]
#[error("item {0} not found")]
pub struct ItemNotFound(pub ItemId);

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
}

#[derive(Clone)]
pub struct Store {
    inner: Arc<Inner>,
}

struct Inner {
    #[cfg(feature = "qualification")]
    writer: std::thread::JoinHandle<()>,
    #[cfg(feature = "qualification")]
    checkpointer: std::thread::JoinHandle<()>,
    #[cfg_attr(
        not(feature = "qualification"),
        expect(dead_code, reason = "dropping it stops the checkpointer")
    )]
    checkpoints: std::sync::mpsc::Sender<()>,
    jobs: mpsc::Sender<Job>,
    events: broadcast::Sender<Envelope>,
    local_live: broadcast::Sender<Envelope>,
    revocations: broadcast::Sender<DeviceId>,
    head: Arc<AtomicU64>,
    live_revision: AtomicU64,
    readers: Mutex<Vec<Connection>>,
    read_slots: Arc<Semaphore>,
    scan_slots: Arc<Semaphore>,
    hot: Arc<hot::HotPages>,
    index_build: Arc<Semaphore>,
    devices: Mutex<Devices>,
    database: PathBuf,
    blob_dir: PathBuf,
    resource_dir: PathBuf,
    blob_lock: Mutex<()>,
}

fn open_database(path: &Path) -> Result<Connection> {
    let opened = Connection::open(path).and_then(|conn| {
        conn.busy_timeout(BUSY_TIMEOUT)?;
        Ok(conn)
    });
    opened.map_err(|source| StorageError::Open {
        database: path.to_path_buf(),
        source: Box::new(source),
    })
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
        source: blobs::BlobFileError,
    },
    #[error("roll {} back to a schema this Forge supports", database.display())]
    RollBack {
        database: PathBuf,
        #[source]
        source: RollbackError,
    },
    #[error("upgrade {}", database.display())]
    Migrate {
        database: PathBuf,
        #[source]
        source: SchemaError,
    },
    #[error("there is no running Forge store to join at {}", database.display())]
    NotRunning { database: PathBuf },
    #[error(
        "online upgrade requires database schema {expected}, the running store uses {found}; the current Forge was left running"
    )]
    Schema { expected: usize, found: usize },
    #[error("{} refused WAL journaling (mode {mode})", database.display())]
    Wal { database: PathBuf, mode: String },
    #[error("start the store's {thread} thread")]
    Spawn {
        thread: &'static str,
        #[source]
        source: std::io::Error,
    },
    #[error("recover the runs the last Forge left live")]
    Recover(#[source] WriteFailure),
    #[error("prepare the database {}", database.display())]
    Database {
        database: PathBuf,
        #[source]
        source: StorageError,
    },
}

#[cfg(feature = "qualification")]
#[derive(Debug, thiserror::Error)]
pub enum ShutdownError {
    #[error("the Forge store still has live handles")]
    Shared,
    #[error("the Forge store writer panicked")]
    WriterPanicked,
    #[error("the Forge store checkpointer panicked")]
    CheckpointerPanicked,
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

pub struct PendingInputs {
    pub items: Vec<Item>,
    pub cutoff: Seq,
}

impl Store {
    pub fn open(dir: &Path) -> Result<Self, OpenStoreError> {
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
        let rollback = schema::roll_back(&database).map_err(|source| OpenStoreError::RollBack {
            database: database.clone(),
            source,
        })?;
        match rollback {
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
                "rolled the database back to the schema this Forge supports; newer data was set aside"
            ),
        }
        Self::start(database, blob_dir, dir.join("resources"), true)
    }

    pub fn join(dir: &Path) -> Result<Self, OpenStoreError> {
        let database = dir.join("forge.sqlite3");
        if !database.is_file() {
            return Err(OpenStoreError::NotRunning { database });
        }
        let prepare = |source| OpenStoreError::Database {
            database: database.clone(),
            source,
        };
        let conn = open_database(&database).map_err(prepare)?;
        let found: usize = conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .map_err(|source| prepare(source.into()))?;
        if found != schema::version() {
            return Err(OpenStoreError::Schema {
                expected: schema::version(),
                found,
            });
        }
        drop(conn);
        Self::start(database, dir.join("blobs"), dir.join("resources"), false)
    }

    pub const fn schema_version() -> usize {
        schema::version()
    }

    fn start(
        database: PathBuf,
        blob_dir: PathBuf,
        resource_dir: PathBuf,
        recover: bool,
    ) -> Result<Self, OpenStoreError> {
        let prepare = |source| OpenStoreError::Database {
            database: database.clone(),
            source,
        };
        let sql = |source: rusqlite::Error| prepare(source.into());
        let mut conn = open_database(&database).map_err(prepare)?;
        let mode: String = conn
            .pragma_update_and_check(None, "journal_mode", "WAL", |row| row.get(0))
            .map_err(sql)?;
        if !mode.eq_ignore_ascii_case("wal") {
            return Err(OpenStoreError::Wal { database, mode });
        }
        conn.pragma_update(None, "cache_size", -WRITER_CACHE_KIB)
            .map_err(sql)?;
        conn.pragma_update(None, "wal_autocheckpoint", WAL_BACKSTOP_PAGES)
            .map_err(sql)?;
        if recover {
            schema::migrate(&mut conn).map_err(|source| OpenStoreError::Migrate {
                database: database.clone(),
                source,
            })?;
        }

        let (events, _) = broadcast::channel(EVENT_CAPACITY);
        let head = Arc::new(AtomicU64::new(0));
        let hot = Arc::new(hot::HotPages::default());
        let commits = Arc::new(AtomicU64::new(0));
        let mut writer = Writer::new(
            conn,
            Arc::clone(&head),
            recover.then(|| events.clone()),
            Arc::clone(&hot),
            Arc::clone(&commits),
        )
        .map_err(prepare)?;
        if recover {
            writer
                .transact(Durability::Background, |tx| tx.recover_threads(None))
                .map_err(OpenStoreError::Recover)?;
        }

        let (jobs, queue) = mpsc::channel::<Job>(WRITE_QUEUE_CAPACITY);
        let writer_thread = std::thread::Builder::new()
            .name("forge-store-writer".into())
            .spawn(move || writer.run(queue))
            .map_err(|source| OpenStoreError::Spawn {
                thread: "writer",
                source,
            })?;
        #[cfg(not(feature = "qualification"))]
        drop(writer_thread);

        let (checkpoints, stopped) = std::sync::mpsc::channel();
        let checkpoint_conn = open_database(&database).map_err(prepare)?;
        let checkpointer_thread = std::thread::Builder::new()
            .name("forge-store-checkpoint".into())
            .spawn(move || checkpoint_when_due(&checkpoint_conn, &commits, &stopped))
            .map_err(|source| OpenStoreError::Spawn {
                thread: "checkpointer",
                source,
            })?;
        #[cfg(not(feature = "qualification"))]
        drop(checkpointer_thread);

        let first_reader = open_database(&database).map_err(prepare)?;
        let devices = Devices::open(open_database(&database).map_err(prepare)?).map_err(prepare)?;
        Ok(Self {
            inner: Arc::new(Inner {
                #[cfg(feature = "qualification")]
                writer: writer_thread,
                #[cfg(feature = "qualification")]
                checkpointer: checkpointer_thread,
                checkpoints,
                jobs,
                events,
                local_live: broadcast::channel(EVENT_CAPACITY).0,
                revocations: broadcast::channel(64).0,
                head,
                live_revision: AtomicU64::new(0),
                readers: Mutex::new(vec![first_reader]),
                read_slots: Arc::new(Semaphore::new(READ_SLOTS)),
                scan_slots: Arc::new(Semaphore::new(SCAN_SLOTS)),
                hot,
                index_build: Arc::new(Semaphore::new(1)),
                devices: Mutex::new(devices),
                database,
                blob_dir,
                resource_dir,
                blob_lock: Mutex::new(()),
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
            writer,
            checkpoints,
            checkpointer,
            readers,
            devices,
            ..
        } = inner;
        drop(readers);
        drop(devices);
        drop(jobs);
        writer
            .join()
            .map_err(|_panic| ShutdownError::WriterPanicked)?;
        drop(checkpoints);
        checkpointer
            .join()
            .map_err(|_panic| ShutdownError::CheckpointerPanicked)?;
        Ok(())
    }

    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<Envelope> {
        self.inner.events.subscribe()
    }

    pub fn publish_live(&self, event: Event) {
        self.inner
            .local_live
            .send(Envelope {
                seq: None,
                live_revision: 0,
                event: event.clone(),
            })
            .ok();
        self.relay_live(event);
    }

    pub fn subscribe_local_live(&self) -> broadcast::Receiver<Envelope> {
        self.inner.local_live.subscribe()
    }

    pub fn relay_live(&self, event: Event) {
        let live_revision = self.inner.live_revision.fetch_add(1, Ordering::AcqRel) + 1;
        self.inner
            .events
            .send(Envelope {
                seq: None,
                live_revision,
                event,
            })
            .ok();
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
            for envelope in changes {
                if let Some(seq) = envelope.seq {
                    *cursor = (*cursor).max(seq);
                }
                self.inner.events.send(envelope).ok();
            }
        } else {
            *cursor = self.read(read::max_seq).await.map_err(failed)?;
            self.inner.hot.reset();
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
            let mut statement = conn.prepare(include_str!("sql/recovery/threads.sql"))?;
            Ok(statement
                .query_map([], |row| Ok(ThreadId::from(row.get::<_, String>(0)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?)
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
        self.write_with(Durability::Background, job).await
    }

    async fn write_acknowledged<R, E, F>(&self, job: F) -> Result<R, E>
    where
        R: Send + 'static,
        E: From<StorageError> + Send + 'static,
        F: FnOnce(&mut Tx<'_>) -> Result<R, E> + Send + 'static,
    {
        self.write_with(Durability::Acknowledged, job).await
    }

    async fn write_with<R, E, F>(&self, durability: Durability, job: F) -> Result<R, E>
    where
        R: Send + 'static,
        E: From<StorageError> + Send + 'static,
        F: FnOnce(&mut Tx<'_>) -> Result<R, E> + Send + 'static,
    {
        let (reply, result) = oneshot::channel();
        self.inner
            .jobs
            .send(Box::new(move |writer: &mut Writer| {
                reply.send(writer.transact(durability, job)).ok();
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
            None => open_database(&self.database)?,
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
    stopped: &std::sync::mpsc::Receiver<()>,
) {
    let mut checkpointed = commits.load(Ordering::Acquire);
    let mut waiting_since = None;
    while matches!(
        stopped.recv_timeout(Duration::from_secs(1)),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout)
    ) {
        let committed = commits.load(Ordering::Acquire);
        let pending = committed.saturating_sub(checkpointed);
        if pending == 0 {
            continue;
        }
        let since = *waiting_since.get_or_insert_with(std::time::Instant::now);
        if pending < CHECKPOINT_AFTER_COMMITS && since.elapsed() < CHECKPOINT_AFTER {
            continue;
        }
        let progress = conn.query_row("PRAGMA wal_checkpoint(PASSIVE)", [], |row| {
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
