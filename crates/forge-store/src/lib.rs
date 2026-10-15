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
#[cfg(any(feature = "legacy-import", test))]
mod import_writes;
mod items;
#[cfg(feature = "legacy-import")]
pub mod legacy;
mod pairing;
mod read;
mod schema;
mod settings;
mod source_index;
mod sources;
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

const EVENT_CAPACITY: usize = 8192;
const READERS_KEPT: usize = 4;
const WRITE_QUEUE_CAPACITY: usize = 256;
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// An item as an engine adapter or the HTTP layer reports it; the store
/// fills in position, seq and timestamps.
#[derive(Clone, Debug, PartialEq)]
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
    jobs: mpsc::Sender<Job>,
    events: broadcast::Sender<Envelope>,
    revocations: broadcast::Sender<DeviceId>,
    head: Arc<AtomicU64>,
    live_revision: AtomicU64,
    readers: Mutex<Vec<Connection>>,
    read_slots: Arc<Semaphore>,
    /// One source index build at a time; see [`Store::source_index`].
    index_build: Arc<Semaphore>,
    devices: Mutex<Devices>,
    database: PathBuf,
    blob_dir: PathBuf,
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
    /// Opens (or creates) `forge.sqlite3` and `blobs/` in `dir`, recovers
    /// runs left live by a previous process. The server owns maintenance.
    pub fn open(dir: &Path) -> Result<Self> {
        let blob_dir = dir.join("blobs");
        std::fs::create_dir_all(&blob_dir)
            .with_context(|| format!("create {}", blob_dir.display()))?;
        Self::start(dir.join("forge.sqlite3"), blob_dir)
    }

    fn start(database: PathBuf, blob_dir: PathBuf) -> Result<Self> {
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
        schema::migrate(&mut conn)?;

        let (events, _) = broadcast::channel(EVENT_CAPACITY);
        let head = Arc::new(AtomicU64::new(0));
        let mut writer = Writer::new(conn, head.clone(), events.clone())?;
        // `Tx::recover` binds the impl lifetime early; the job must be generic over it.
        #[allow(clippy::redundant_closure_for_method_calls)]
        writer
            .transact(|tx| tx.recover())
            .context("recover live runs")?;

        let (jobs, queue) = mpsc::channel::<Job>(WRITE_QUEUE_CAPACITY);
        std::thread::Builder::new()
            .name("forge-store-writer".into())
            .spawn(move || writer.run(queue))
            .context("spawn the store writer")?;

        // Warm the bounded read pool.
        let first_reader = open_database(&database)?;
        Ok(Self {
            inner: Arc::new(Inner {
                jobs,
                events,
                revocations: broadcast::channel(64).0,
                head,
                live_revision: AtomicU64::new(0),
                readers: Mutex::new(vec![first_reader]),
                read_slots: Arc::new(Semaphore::new(READERS_KEPT)),
                index_build: Arc::new(Semaphore::new(1)),
                devices: Mutex::new(Devices::open(open_database(&database)?)?),
                database,
                blob_dir,
                blob_lock: Mutex::new(()),
            }),
        })
    }

    /// Envelopes with `seq: Some` arrive in seq order, after their commit.
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<Envelope> {
        self.inner.events.subscribe()
    }

    /// Fans out a memory-only event (streaming text, shell output).
    pub fn publish_live(&self, event: Event) {
        let live_revision = self.inner.live_revision.fetch_add(1, Ordering::AcqRel) + 1;
        let _ = self.inner.events.send(Envelope {
            seq: None,
            live_revision,
            event,
        });
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

    async fn write<R, F>(&self, job: F) -> Result<R>
    where
        R: Send + 'static,
        F: FnOnce(&mut Tx<'_>) -> Result<R> + Send + 'static,
    {
        let (reply, result) = oneshot::channel();
        self.inner
            .jobs
            .send(Box::new(move |writer: &mut Writer| {
                let _ = reply.send(writer.transact(job));
            }))
            .await
            .map_err(|_| anyhow!("the store writer stopped"))?;
        result.await.context("the store writer dropped a write")?
    }

    async fn read<R, F>(&self, query: F) -> Result<R>
    where
        R: Send + 'static,
        F: FnOnce(&Connection) -> Result<R> + Send + 'static,
    {
        let inner = self.inner.clone();
        let slot = inner.read_slots.clone().acquire_owned().await?;
        tokio::task::spawn_blocking(move || {
            let _slot = slot;
            inner.with_reader(query)
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
