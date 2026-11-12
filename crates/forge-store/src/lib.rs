//! Forge's durable state: one SQLite file plus content-addressed blobs.
//!
//! Every stored change (project, thread, item, run, setting) takes the next
//! value of one global seq. A single writer thread commits changes and
//! publishes them to [`Store::subscribe`] in commit order; reads go through a
//! small pool of read connections.
//!
//! `threads`, `items`, `sources`, `settings`, `pairing` and `blobs` hold the
//! [`Store`] operations of each part; `read` and `writer` hold the SQL.

pub mod admin;
mod blobs;
mod collection;
mod content;
pub mod coordination;
pub mod discovery;
mod hot;
#[cfg(any(feature = "legacy-import", test))]
mod import_writes;
mod items;
#[cfg(feature = "legacy-import")]
pub mod legacy;
mod pairing;
mod read;
mod recall;
mod resources;
mod retention;
mod schema;
mod settings;
mod source_index;
mod sources;
mod text_search;
mod threads;
mod writer;

use anyhow::{Context, Result, anyhow, ensure};
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
use writer::{Job, Tx, Writer};

pub use blobs::{BlobRejected, MAX_BLOB_BYTES};
pub use items::Acceptance;
pub use pairing::Device;
pub use recall::{GrepMatch, GrepPage, GrepQuery, RecallScope, SearchHit, SearchPage, Touch};
pub use resources::{MAX_RESOURCE_SNAPSHOT_BYTES, keep as keep_resource_snapshot};

const EVENT_CAPACITY: usize = 8192;
const READ_SLOTS: usize = 4;
const SCAN_SLOTS: usize = 2;
const READERS_KEPT: usize = READ_SLOTS + SCAN_SLOTS;
const SLOW_READ: std::time::Duration = std::time::Duration::from_millis(500);
const WRITE_QUEUE_CAPACITY: usize = 256;
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);
/// Whether runs a restart cut short carry on by themselves; default on.
pub const CONTINUE_AFTER_RESTART_KEY: &str = "continue_after_restart";
/// How many restarts in a row a turn carries on through before it waits
/// for the user: one that keeps taking Forge down would otherwise loop.
pub const MAX_RESTART_CONTINUATIONS: usize = 3;
/// Whether a turn the provider's capacity failed is retried; default on.
pub const RETRY_WHEN_OVERLOADED_KEY: &str = "retry_when_overloaded";
/// Whether a turn the usage limit stopped carries on once it resets;
/// default on.
pub const CONTINUE_AFTER_USAGE_LIMIT_KEY: &str = "continue_after_usage_limit";

/// A failed turn Forge picks up again by itself at `due_at`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Park {
    pub continuation: artisan_api::Continuation,
    pub due_at: artisan_api::Timestamp,
}

/// The setting that turns parking for `continuation` on and off.
#[must_use]
pub const fn park_setting(continuation: artisan_api::Continuation) -> Option<&'static str> {
    match continuation {
        artisan_api::Continuation::Overloaded => Some(RETRY_WHEN_OVERLOADED_KEY),
        artisan_api::Continuation::UsageLimit => Some(CONTINUE_AFTER_USAGE_LIMIT_KEY),
        artisan_api::Continuation::Requested | artisan_api::Continuation::Restart => None,
    }
}

/// An item as an engine adapter or the HTTP layer reports it; the store
/// fills in position, seq and timestamps.
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

/// A requested project does not exist. HTTP can map this canonical rejection
/// without reading the same project before submitting the operation.
#[derive(Debug, thiserror::Error)]
#[error("project {0} not found")]
pub struct ProjectNotFound(pub ProjectId);

#[derive(Debug, thiserror::Error)]
#[error("item {0} not found")]
pub struct ItemNotFound(pub ItemId);

/// A request the stored state turns down, such as an answer to a question
/// that is no longer waiting. The message is for the user.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct Refused(pub String);

#[derive(Debug, thiserror::Error)]
pub enum SendRejected {
    #[error("thread {0} not found")]
    ThreadNotFound(ThreadId),
    #[error("message id {0} was already used with different content or thread")]
    IdConflict(ItemId),
    #[error("{0}")]
    Payload(String),
    #[error("image blob {0} is missing or invalid")]
    MissingBlob(BlobHash),
}

/// Everything stored that a client needs on connect, besides item windows.
#[derive(Clone, Debug, PartialEq)]
pub struct Snapshot {
    /// Every change up to here is included; resume events after it.
    pub head: Seq,
    /// Projects not removed.
    pub projects: Vec<Project>,
    pub threads: Vec<Thread>,
    pub threads_next: Option<artisan_api::ThreadCursor>,
    /// Live runs plus the latest run of every thread.
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
    /// One source index build at a time; see [`Store::source_index`].
    index_build: Arc<Semaphore>,
    devices: Mutex<Devices>,
    database: PathBuf,
    blob_dir: PathBuf,
    resource_dir: PathBuf,
    /// Orders uploads against blob deletion.
    blob_lock: Mutex<()>,
}

fn open_database(path: &Path) -> Result<Connection> {
    let conn =
        Connection::open(path).with_context(|| format!("open database {}", path.display()))?;
    conn.busy_timeout(BUSY_TIMEOUT)?;
    Ok(conn)
}

/// A bounded delivery batch; cutoff is the newest pending input observed in the same snapshot.
pub struct PendingInputs {
    pub items: Vec<Item>,
    pub cutoff: Seq,
}

impl Store {
    pub fn open(dir: &Path) -> Result<Self> {
        let blob_dir = dir.join("blobs");
        std::fs::create_dir_all(&blob_dir)
            .with_context(|| format!("create {}", blob_dir.display()))?;
        blobs::name_files(&blob_dir)?;
        Self::start(
            dir.join("forge.sqlite3"),
            blob_dir,
            dir.join("resources"),
            true,
        )
    }

    pub fn join(dir: &Path) -> Result<Self> {
        let database = dir.join("forge.sqlite3");
        ensure!(
            database.is_file(),
            "there is no running Forge store to join"
        );
        let conn = open_database(&database)?;
        let version: usize = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
        ensure!(
            version == schema::version(),
            "online upgrade requires database schema {}, running store uses {version}; the current Forge was left running",
            schema::version()
        );
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
    ) -> Result<Self> {
        let mut conn = open_database(&database)?;
        {
            let mode: String = conn
                .pragma_update_and_check(None, "journal_mode", "WAL", |row| row.get(0))
                .context("enable WAL")?;
            ensure!(
                mode.eq_ignore_ascii_case("wal"),
                "database refused WAL (mode {mode})"
            );
        }
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        if recover {
            schema::migrate(&mut conn)?;
        }

        let (events, _) = broadcast::channel(EVENT_CAPACITY);
        let head = Arc::new(AtomicU64::new(0));
        let hot = Arc::new(hot::HotPages::default());
        let mut writer = Writer::new(
            conn,
            Arc::clone(&head),
            recover.then(|| events.clone()),
            Arc::clone(&hot),
        )?;
        if recover {
            writer
                .transact(|tx| tx.recover_threads(None))
                .context("recover live runs")?;
        }

        let (jobs, queue) = mpsc::channel::<Job>(WRITE_QUEUE_CAPACITY);
        let writer_thread = std::thread::Builder::new()
            .name("forge-store-writer".into())
            .spawn(move || writer.run(queue))
            .context("spawn the store writer")?;
        #[cfg(not(feature = "qualification"))]
        drop(writer_thread);

        // Warm the bounded read pool.
        let first_reader = open_database(&database)?;
        Ok(Self {
            inner: Arc::new(Inner {
                #[cfg(feature = "qualification")]
                writer: writer_thread,
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
                devices: Mutex::new(Devices::open(open_database(&database)?)?),
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
    pub fn shutdown_unique(self) -> Result<()> {
        let inner = Arc::try_unwrap(self.inner)
            .map_err(|_shared| anyhow!("Forge store still has live handles"))?;
        let Inner {
            jobs,
            writer,
            readers,
            devices,
            ..
        } = inner;
        drop(readers);
        drop(devices);
        drop(jobs);
        writer
            .join()
            .map_err(|_panic| anyhow!("Forge store writer panicked"))?;
        Ok(())
    }

    /// Envelopes with `seq: Some` arrive in seq order, after their commit.
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<Envelope> {
        self.inner.events.subscribe()
    }

    /// Fans out a memory-only event (streaming text, shell output).
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
        // No subscribers is fine: live events are not replayed.
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

    /// The last committed seq.
    #[must_use]
    pub fn head(&self) -> Seq {
        self.inner.head.load(Ordering::Acquire)
    }

    pub async fn synchronize(&self, cursor: &mut Seq) -> Result<()> {
        if let Some(changes) = self.changes_since(*cursor, 20_000).await? {
            self.inner.hot.committed(&changes);
            for envelope in changes {
                if let Some(seq) = envelope.seq {
                    *cursor = (*cursor).max(seq);
                }
                self.inner.events.send(envelope).ok();
            }
        } else {
            *cursor = self.read(read::max_seq).await?;
            self.inner.hot.reset();
            self.relay_live(Event::Reset);
        }
        self.inner.head.fetch_max(*cursor, Ordering::AcqRel);
        Ok(())
    }

    pub async fn recover_thread(&self, thread: ThreadId) -> Result<()> {
        self.write(move |tx| tx.recover_threads(Some(vec![thread])))
            .await
    }

    pub async fn running_threads(&self) -> Result<Vec<ThreadId>> {
        self.read(|conn| {
            let mut statement = conn.prepare(include_str!("sql/recovery/threads.sql"))?;
            Ok(statement
                .query_map([], |row| Ok(ThreadId::from(row.get::<_, String>(0)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?)
        })
        .await
    }

    async fn write<R, F>(&self, job: F) -> Result<R>
    where
        R: Send + 'static,
        F: FnOnce(&mut Tx<'_>) -> Result<R> + Send + 'static,
    {
        let (reply, result) = oneshot::channel();
        self.inner
            .jobs
            .send(Box::new(move |writer: &mut Writer| {
                // The caller stopped waiting; the write still committed.
                reply.send(writer.transact(job)).ok();
            }))
            .await
            .map_err(|_unsent_job| anyhow!("the store writer stopped"))?;
        result.await.context("the store writer dropped a write")?
    }

    async fn tail_page<F>(
        &self,
        thread: &ThreadId,
        tail: Option<hot::Tail>,
        query: F,
    ) -> Result<artisan_api::ItemsPage>
    where
        F: FnOnce(&Connection) -> Result<artisan_api::ItemsPage> + Send + 'static,
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

    async fn read<R, F>(&self, query: F) -> Result<R>
    where
        R: Send + 'static,
        F: FnOnce(&Connection) -> Result<R> + Send + 'static,
    {
        self.read_in(&self.inner.read_slots, query).await
    }

    async fn scan<R, F>(&self, query: F) -> Result<R>
    where
        R: Send + 'static,
        F: FnOnce(&Connection) -> Result<R> + Send + 'static,
    {
        self.read_in(&self.inner.scan_slots, query).await
    }

    async fn read_in<R, F>(&self, slots: &Arc<Semaphore>, query: F) -> Result<R>
    where
        R: Send + 'static,
        F: FnOnce(&Connection) -> Result<R> + Send + 'static,
    {
        let inner = Arc::clone(&self.inner);
        let asked = std::time::Instant::now();
        let slot = Arc::clone(slots).acquire_owned().await?;
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
        .context("store read task")?
    }
}

impl Inner {
    fn with_reader<R>(&self, query: impl FnOnce(&Connection) -> Result<R>) -> Result<R> {
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
