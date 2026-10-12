//! Forge's durable state: one SQLite file plus content-addressed blobs.
//!
//! Every stored change (project, thread, item, run, setting) takes the next
//! value of one global seq. A single writer thread commits changes and
//! publishes them to [`Store::subscribe`] in commit order; reads go through a
//! small pool of read connections.

pub mod admin;
mod blobs;
pub mod legacy;
mod read;
mod schema;
mod writer;

use anyhow::{Context, Result, anyhow, ensure};
use artisan_api::{
    BlobHash, BlobRef, CreateProject, CreateThread, DeviceId, Envelope, Event, Item, ItemId,
    ItemKind, ItemsPage, ItemsQuery, Project, ProjectId, Run, RunId, Seq, Status, Thread, ThreadId,
    Timestamp, UpdateThread, now,
};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::time::Duration;
use tokio::sync::{broadcast, oneshot};
use writer::{Job, Stamp, Tx, Writer};

pub use blobs::{BlobRejected, MAX_BLOB_BYTES};

const EVENT_CAPACITY: usize = 8192;
const READERS_KEPT: usize = 4;
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

/// Everything stored that a client needs on connect, besides item windows.
#[derive(Clone, Debug, PartialEq)]
pub struct Snapshot {
    /// Every change up to here is included; resume events after it.
    pub head: Seq,
    /// Projects not removed.
    pub projects: Vec<Project>,
    pub threads: Vec<Thread>,
    /// Live runs plus the latest run of every thread.
    pub runs: Vec<Run>,
    pub settings: serde_json::Map<String, serde_json::Value>,
}

/// A paired Editor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Device {
    pub id: DeviceId,
    pub name: String,
    pub public_key: String,
    pub created_at: Timestamp,
    pub last_seen: Option<Timestamp>,
}

#[derive(Clone)]
pub struct Store {
    inner: Arc<Inner>,
}

struct Inner {
    jobs: mpsc::Sender<Job>,
    events: broadcast::Sender<Envelope>,
    head: Arc<AtomicU64>,
    readers: Mutex<Vec<Connection>>,
    database: Database,
    blob_dir: PathBuf,
}

enum Database {
    File(PathBuf),
    /// A shared-cache URI, so every connection sees the same database.
    Memory(String),
}

impl Database {
    fn connect(&self) -> Result<Connection> {
        let conn = match self {
            Self::File(path) => Connection::open(path)
                .with_context(|| format!("open database {}", path.display()))?,
            Self::Memory(uri) => Connection::open_with_flags(
                uri,
                OpenFlags::SQLITE_OPEN_READ_WRITE
                    | OpenFlags::SQLITE_OPEN_CREATE
                    | OpenFlags::SQLITE_OPEN_URI
                    | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )
            .with_context(|| format!("open in-memory database {uri}"))?,
        };
        conn.busy_timeout(BUSY_TIMEOUT)?;
        Ok(conn)
    }

    fn reader(&self) -> Result<Connection> {
        let conn = self.connect()?;
        if matches!(self, Self::Memory(_)) {
            // Shared-cache tables lock against a writing connection
            // (SQLITE_LOCKED, which busy_timeout does not wait out).
            // In-memory stores are for tests, so dirty reads are fine there.
            conn.pragma_update(None, "read_uncommitted", true)?;
        }
        Ok(conn)
    }
}

impl Store {
    /// Opens (or creates) `forge.sqlite3` and `blobs/` in `dir`, recovers
    /// runs left live by a previous process and collects unreferenced blobs
    /// in the background.
    pub fn open(dir: &Path) -> Result<Self> {
        let blob_dir = dir.join("blobs");
        std::fs::create_dir_all(&blob_dir)
            .with_context(|| format!("create {}", blob_dir.display()))?;
        let store = Self::start(Database::File(dir.join("forge.sqlite3")), blob_dir)?;
        let inner = store.inner.clone();
        std::thread::Builder::new()
            .name("forge-blob-gc".into())
            .spawn(move || match inner.gc_blobs() {
                Ok(removed) => tracing::debug!(removed, "collected unreferenced blobs"),
                Err(error) => {
                    tracing::warn!(error = format!("{error:#}"), "collect unreferenced blobs");
                }
            })
            .context("spawn the blob collector")?;
        Ok(store)
    }

    /// A private in-memory database (for tests) with blobs in `blob_dir`.
    pub fn open_in_memory(blob_dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(blob_dir)
            .with_context(|| format!("create {}", blob_dir.display()))?;
        let uri = format!(
            "file:forge-{}?mode=memory&cache=shared",
            uuid::Uuid::now_v7()
        );
        Self::start(Database::Memory(uri), blob_dir.to_owned())
    }

    fn start(database: Database, blob_dir: PathBuf) -> Result<Self> {
        let mut conn = database.connect()?;
        if matches!(database, Database::File(_)) {
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

        let (jobs, queue) = mpsc::channel::<Job>();
        std::thread::Builder::new()
            .name("forge-store-writer".into())
            .spawn(move || writer.run(&queue))
            .context("spawn the store writer")?;

        // Keeps a shared-cache memory database alive and warms the pool.
        let first_reader = database.reader()?;
        Ok(Self {
            inner: Arc::new(Inner {
                jobs,
                events,
                head,
                readers: Mutex::new(vec![first_reader]),
                database,
                blob_dir,
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
        let _ = self.inner.events.send(Envelope { seq: None, event });
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
        self.write_at(None, job).await
    }

    async fn write_at<R, F>(&self, stamp: Option<Stamp>, job: F) -> Result<R>
    where
        R: Send + 'static,
        F: FnOnce(&mut Tx<'_>) -> Result<R> + Send + 'static,
    {
        let (reply, result) = oneshot::channel();
        self.inner
            .jobs
            .send(Box::new(move |writer: &mut Writer| {
                let _ = reply.send(writer.transact_at(stamp, job));
            }))
            .map_err(|_| anyhow!("the store writer stopped"))?;
        result.await.context("the store writer dropped a write")?
    }

    async fn read<R, F>(&self, query: F) -> Result<R>
    where
        R: Send + 'static,
        F: FnOnce(&Connection) -> Result<R> + Send + 'static,
    {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || inner.with_reader(query))
            .await
            .context("store read task")?
    }

    // Writes.

    /// Creating an existing id returns the stored project unchanged.
    pub async fn create_project(&self, request: CreateProject) -> Result<Project> {
        self.create_project_at(request, None).await
    }

    pub(crate) async fn create_project_at(
        &self,
        request: CreateProject,
        stamp: Option<Stamp>,
    ) -> Result<Project> {
        let id = request.id.clone();
        self.write_at(stamp, move |tx| {
            if let Some(existing) = read::project(tx, &request.id)? {
                return Ok(existing);
            }
            let name = request.name.unwrap_or_else(|| project_name(&request.path));
            tx.save_project(Project {
                id: request.id,
                path: request.path,
                name,
                seq: 0,
                created_at: tx.created_at(),
                removed: false,
            })
        })
        .await
        .with_context(|| format!("create project {id}"))
    }

    /// Marks the project removed. `None` if it does not exist.
    pub async fn remove_project(&self, id: &ProjectId) -> Result<Option<Project>> {
        let id = id.clone();
        self.write(move |tx| {
            let Some(mut project) = read::project(tx, &id)? else {
                return Ok(None);
            };
            if project.removed {
                return Ok(Some(project));
            }
            project.removed = true;
            tx.save_project(project).map(Some)
        })
        .await
    }

    /// Creating an existing id returns the stored thread unchanged.
    pub async fn create_thread(&self, request: CreateThread) -> Result<Thread> {
        self.create_thread_at(request, None).await
    }

    pub(crate) async fn create_thread_at(
        &self,
        request: CreateThread,
        stamp: Option<Stamp>,
    ) -> Result<Thread> {
        let id = request.id.clone();
        self.write_at(stamp, move |tx| {
            if let Some(existing) = read::thread(tx, &request.id)? {
                return Ok(existing);
            }
            ensure!(
                read::project(tx, &request.project_id)?.is_some(),
                "project {} not found",
                request.project_id
            );
            let created_at = tx.created_at();
            tx.save_thread(Thread {
                id: request.id,
                project_id: request.project_id,
                title: request.title,
                config: request.config,
                activity_seq: 0,
                read_seq: 0,
                seq: 0,
                created_at,
                updated_at: created_at,
                archived: false,
                parent_thread_id: request.parent_thread_id,
            })
        })
        .await
        .with_context(|| format!("create thread {id}"))
    }

    /// `None` if the thread does not exist.
    pub async fn update_thread(
        &self,
        id: &ThreadId,
        update: UpdateThread,
    ) -> Result<Option<Thread>> {
        self.update_thread_at(id, update, None).await
    }

    pub(crate) async fn update_thread_at(
        &self,
        id: &ThreadId,
        update: UpdateThread,
        stamp: Option<Stamp>,
    ) -> Result<Option<Thread>> {
        let id = id.clone();
        self.write_at(stamp, move |tx| {
            let Some(mut thread) = read::thread(tx, &id)? else {
                return Ok(None);
            };
            if let Some(title) = update.title {
                thread.title = Some(title);
            }
            if let Some(archived) = update.archived {
                thread.archived = archived;
            }
            if let Some(config) = update.config {
                thread.config = config;
            }
            if let Some(read_seq) = update.read_seq {
                thread.read_seq = read_seq;
            }
            tx.save_thread(thread).map(Some)
        })
        .await
    }

    /// Inserts the item or updates it by id. Position and creation time never
    /// change, and a terminal status never reopens.
    pub async fn upsert_item(&self, new: NewItem) -> Result<Item> {
        self.upsert_item_at(new, None).await
    }

    pub(crate) async fn upsert_item_at(&self, new: NewItem, stamp: Option<Stamp>) -> Result<Item> {
        let (id, thread_id) = (new.id.clone(), new.thread_id.clone());
        self.write_at(stamp, move |tx| {
            if let Some(old) = read::item(tx, &new.id)? {
                ensure!(
                    old.thread_id == new.thread_id,
                    "item belongs to thread {}",
                    old.thread_id
                );
                let mut kind = new.kind;
                // Engines don't know Artisan's name for a subagent, and an
                // agent adopted by a later process no longer knows its
                // prompt: keep them.
                if let (
                    ItemKind::Agent {
                        label,
                        prompt,
                        agent_id,
                        ..
                    },
                    ItemKind::Agent {
                        label: old_label,
                        prompt: old_prompt,
                        agent_id: old_agent_id,
                        ..
                    },
                ) = (&mut kind, &old.kind)
                {
                    for (slot, old) in [
                        (label, old_label),
                        (prompt, old_prompt),
                        (agent_id, old_agent_id),
                    ] {
                        if slot.is_none() {
                            slot.clone_from(old);
                        }
                    }
                }
                let item = Item {
                    // An item stays in the run that made it: a resumed agent
                    // reports from a later run, but its row stays where it
                    // was launched, in its own turn.
                    run_id: old.run_id.or(new.run_id),
                    parent_id: new.parent_id.or(old.parent_id),
                    native_id: new.native_id.or(old.native_id),
                    status: old.status.advance(new.status),
                    kind,
                    ..old
                };
                tx.save_item(item, false)
            } else {
                let created_at = tx.created_at();
                let mut new = new;
                // A new subagent gets a name no other in its thread has.
                if let ItemKind::Agent {
                    label: slot @ None, ..
                } = &mut new.kind
                {
                    let taken = read::agent_labels(tx, &new.thread_id)?;
                    *slot = Some(artisan_api::choose_agent_name(
                        new.id.as_str(),
                        taken.iter().map(String::as_str),
                    ));
                }
                tx.save_item(fresh_item(new, created_at), true)
            }
        })
        .await
        .with_context(|| format!("upsert item {id} in thread {thread_id}"))
    }

    /// A subagent an earlier run left stopped is working again in `run`
    /// (resumed by a message to it): it reopens there, so it reads as
    /// working and the run settles it. Statuses otherwise never leave a
    /// terminal state. Returns the stored agent.
    pub async fn reopen_agent(&self, id: &ItemId, run: &RunId) -> Result<Option<Item>> {
        let (id, run) = (id.clone(), run.clone());
        let what = id.clone();
        self.write(move |tx| {
            let Some(old) = read::item(tx, &id)? else {
                return Ok(None);
            };
            if !matches!(old.kind, ItemKind::Agent { .. }) || !old.status.is_terminal() {
                return Ok(Some(old));
            }
            let item = Item {
                run_id: old.run_id.or(Some(run)),
                status: Status::Running,
                ..old
            };
            tx.save_item(item, false).map(Some)
        })
        .await
        .with_context(|| format!("reopen agent {what}"))
    }

    /// Inserts a client-sent message unless its id is already stored (a
    /// retried send). Returns the stored item and whether it was created.
    pub async fn insert_user_message(&self, new: NewItem) -> Result<(Item, bool)> {
        let (id, thread_id) = (new.id.clone(), new.thread_id.clone());
        self.write(move |tx| {
            if let Some(existing) = read::item(tx, &new.id)? {
                Ok((existing, false))
            } else {
                let created_at = tx.created_at();
                tx.save_item(fresh_item(new, created_at), true)
                    .map(|item| (item, true))
            }
        })
        .await
        .with_context(|| format!("insert user message {id} in thread {thread_id}"))
    }

    pub async fn upsert_run(&self, run: Run) -> Result<Run> {
        let (id, thread_id) = (run.id.clone(), run.thread_id.clone());
        self.write(move |tx| tx.save_run(run))
            .await
            .with_context(|| format!("save run {id} of thread {thread_id}"))
    }

    pub async fn set_setting(&self, key: String, value: serde_json::Value) -> Result<()> {
        self.write(move |tx| tx.save_setting(key, value)).await
    }

    /// Saves a new version of the global prompt and makes it the
    /// `global_prompt` setting, if `base` is still the latest version.
    /// Returns the new version, or `Err(latest)` when `base` is stale.
    pub async fn save_prompt_version(
        &self,
        base: u64,
        text: String,
    ) -> Result<std::result::Result<u64, u64>> {
        self.write(move |tx| {
            let latest = read::prompt_version(tx)?;
            if latest != base {
                return Ok(Err(latest));
            }
            let version = latest + 1;
            tx.execute(
                "INSERT INTO prompt_versions (version, text, saved_at) VALUES (?1, ?2, ?3)",
                params![
                    i64::try_from(version).unwrap_or(i64::MAX),
                    text,
                    artisan_api::now()
                ],
            )?;
            tx.save_setting("global_prompt".to_owned(), serde_json::Value::String(text))?;
            Ok(Ok(version))
        })
        .await
        .context("save the global prompt")
    }

    /// Takes over an old Forge's global prompt: every saved version, then
    /// `current` as the `global_prompt` setting. Leaves a prompt this Forge
    /// already has alone; returns whether it imported.
    pub(crate) async fn import_prompt(
        &self,
        versions: Vec<(u64, String, artisan_api::Timestamp)>,
        current: Option<String>,
    ) -> Result<bool> {
        self.write(move |tx| {
            if read::prompt_version(tx)? > 0 || read::setting(tx, "global_prompt")?.is_some() {
                return Ok(false);
            }
            for (version, text, saved_at) in versions {
                tx.execute(
                    "INSERT INTO prompt_versions (version, text, saved_at) VALUES (?1, ?2, ?3)",
                    params![i64::try_from(version).unwrap_or(i64::MAX), text, saved_at],
                )
                .with_context(|| format!("save global prompt version {version}"))?;
            }
            if let Some(text) = current {
                tx.save_setting("global_prompt".to_owned(), serde_json::Value::String(text))?;
            }
            Ok(true)
        })
        .await
        .context("import the old global prompt")
    }

    /// Sets a setting unless it exists; returns whether it was set.
    pub async fn set_setting_if_absent(
        &self,
        key: String,
        value: serde_json::Value,
    ) -> Result<bool> {
        self.write(move |tx| {
            if read::setting(tx, &key)?.is_some() {
                return Ok(false);
            }
            tx.save_setting(key, value)?;
            Ok(true)
        })
        .await
        .context("set a setting unless present")
    }

    /// The latest saved version of the global prompt; `0` before the first.
    pub async fn prompt_version(&self) -> Result<u64> {
        self.read(read::prompt_version)
            .await
            .context("read the global prompt's version")
    }

    /// Stores a value only Forge reads (its identity), unless one is there
    /// already; returns whichever is stored. Not synced to clients.
    pub async fn secret_or_insert(&self, key: &str, value: String) -> Result<String> {
        let key = key.to_owned();
        self.write(move |tx| {
            tx.execute(
                "INSERT INTO secrets (key, value) VALUES (?1, ?2) ON CONFLICT (key) DO NOTHING",
                params![key, value],
            )?;
            Ok(
                tx.query_row("SELECT value FROM secrets WHERE key = ?1", [&key], |row| {
                    row.get(0)
                })?,
            )
        })
        .await
        .context("save secret")
    }

    /// Drafts are private to the client that writes them: no seq, no event.
    /// `null` deletes the draft.
    pub async fn set_draft(&self, scope: String, value: serde_json::Value) -> Result<()> {
        self.write(move |tx| {
            if value.is_null() {
                tx.execute("DELETE FROM drafts WHERE scope = ?1", [&scope])?;
            } else {
                tx.execute(
                    "INSERT INTO drafts (scope, body) VALUES (?1, ?2)
                     ON CONFLICT (scope) DO UPDATE SET body = excluded.body",
                    params![scope, read::to_body(&value)?],
                )?;
            }
            Ok(())
        })
        .await
        .context("save draft")
    }

    /// Pairing the same key again renames the device and keeps its id.
    pub async fn add_device(&self, name: String, public_key: String) -> Result<DeviceId> {
        self.write(move |tx| {
            let id: String = tx.query_row(
                "INSERT INTO authorized_keys (id, name, public_key, created_at)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (public_key) DO UPDATE SET name = excluded.name
                 RETURNING id",
                params![DeviceId::new().as_str(), name, public_key, now()],
                |row| row.get(0),
            )?;
            Ok(DeviceId(id))
        })
        .await
        .context("add device")
    }

    /// Whether a device was revoked.
    pub async fn revoke_device(&self, id: &DeviceId) -> Result<bool> {
        let id = id.clone();
        self.write(move |tx| {
            Ok(tx.execute("DELETE FROM authorized_keys WHERE id = ?1", [id.as_str()])? > 0)
        })
        .await
    }

    pub async fn touch_device(&self, id: &DeviceId) -> Result<()> {
        let id = id.clone();
        self.write(move |tx| {
            tx.execute(
                "UPDATE authorized_keys SET last_seen = ?2 WHERE id = ?1",
                params![id.as_str(), now()],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn add_pair_code(&self, code_sha256: String, expires_at: Timestamp) -> Result<()> {
        self.write(move |tx| {
            tx.execute(
                "INSERT OR REPLACE INTO pair_codes (code_sha256, expires_at) VALUES (?1, ?2)",
                params![code_sha256, expires_at],
            )?;
            Ok(())
        })
        .await
    }

    /// Consumes the code: true once, if it exists and has not expired.
    pub async fn take_pair_code(&self, code_sha256: String, now: Timestamp) -> Result<bool> {
        self.write(move |tx| {
            let taken = tx.execute(
                "DELETE FROM pair_codes WHERE code_sha256 = ?1 AND expires_at > ?2",
                params![code_sha256, now],
            )? > 0;
            tx.execute("DELETE FROM pair_codes WHERE expires_at <= ?1", [now])?;
            Ok(taken)
        })
        .await
    }

    // Reads.

    pub async fn state(&self) -> Result<Snapshot> {
        // Head first: everything up to it is committed, so the snapshot holds
        // at least that much and events after it fill in the rest.
        let head = self.head();
        self.read(move |conn| {
            let tx = conn.unchecked_transaction()?;
            let projects: Vec<Project> =
                read::many(&tx, "SELECT body FROM projects ORDER BY rowid", [])?;
            let threads = read::many(&tx, "SELECT body FROM threads ORDER BY rowid", [])?;
            let runs = read::many(
                &tx,
                "SELECT body FROM runs WHERE status = 'running'
                    OR rowid IN (SELECT max(rowid) FROM runs GROUP BY thread_id)
                 ORDER BY rowid",
                [],
            )?;
            let mut settings = serde_json::Map::new();
            let mut statement = tx.prepare("SELECT key, value FROM kv")?;
            let mut rows = statement.query([])?;
            while let Some(row) = rows.next()? {
                let key: String = row.get(0)?;
                let value: String = row.get(1)?;
                let value =
                    serde_json::from_str(&value).with_context(|| format!("parse setting {key}"))?;
                settings.insert(key, value);
            }
            Ok(Snapshot {
                head,
                projects: projects
                    .into_iter()
                    .filter(|project| !project.removed)
                    .collect(),
                threads,
                runs,
                settings,
            })
        })
        .await
        .context("read state")
    }

    /// Items by position. Without `after`, the page is the newest items
    /// before `before` (or the thread's tail); `more` says items exist past
    /// the far end of the page.
    pub async fn items_page(&self, thread_id: &ThreadId, query: ItemsQuery) -> Result<ItemsPage> {
        let thread_id = thread_id.clone();
        self.read(move |conn| {
            read::items_page(conn, &thread_id, &query)
                .with_context(|| format!("page items of thread {thread_id}"))
        })
        .await
    }

    /// Every stored entity changed after `since`, in seq order, or `None` when
    /// there are more than `limit` (the client should refetch the state).
    pub async fn changes_since(&self, since: Seq, limit: usize) -> Result<Option<Vec<Envelope>>> {
        self.read(move |conn| {
            read::changes_since(conn, since, limit)
                .with_context(|| format!("read changes since {since}"))
        })
        .await
    }

    pub async fn project(&self, id: &ProjectId) -> Result<Option<Project>> {
        let id = id.clone();
        self.read(move |conn| {
            read::project(conn, &id).with_context(|| format!("read project {id}"))
        })
        .await
    }

    pub async fn thread(&self, id: &ThreadId) -> Result<Option<Thread>> {
        let id = id.clone();
        self.read(move |conn| read::thread(conn, &id).with_context(|| format!("read thread {id}")))
            .await
    }

    pub async fn item(&self, id: &ItemId) -> Result<Option<Item>> {
        let id = id.clone();
        self.read(move |conn| read::item(conn, &id).with_context(|| format!("read item {id}")))
            .await
    }

    /// The item an engine knows as `native_id`, within `run` when given.
    pub async fn item_by_native(
        &self,
        thread_id: &ThreadId,
        run: Option<&RunId>,
        native_id: &str,
    ) -> Result<Option<Item>> {
        let (thread_id, run, native_id) = (thread_id.clone(), run.cloned(), native_id.to_owned());
        self.read(move |conn| {
            read::item_by_native(conn, &thread_id, run.as_ref(), &native_id)
                .with_context(|| format!("read item {native_id:?} of thread {thread_id}"))
        })
        .await
    }

    /// Approvals and questions waiting on the user, across threads.
    pub async fn items_awaiting_user(&self) -> Result<Vec<Item>> {
        self.read(|conn| read::items_awaiting_user(conn).context("read items awaiting the user"))
            .await
    }

    /// User messages still `running`: sent but not yet taken by an engine.
    pub async fn undelivered_messages(&self, thread_id: &ThreadId) -> Result<Vec<Item>> {
        let thread_id = thread_id.clone();
        self.read(move |conn| {
            read::undelivered_messages(conn, &thread_id)
                .with_context(|| format!("read undelivered messages of thread {thread_id}"))
        })
        .await
    }

    pub async fn run(&self, id: &RunId) -> Result<Option<Run>> {
        let id = id.clone();
        self.read(move |conn| read::run(conn, &id).with_context(|| format!("read run {id}")))
            .await
    }

    pub async fn latest_run(&self, thread_id: &ThreadId) -> Result<Option<Run>> {
        let thread_id = thread_id.clone();
        self.read(move |conn| {
            read::latest_run(conn, &thread_id)
                .with_context(|| format!("read latest run of thread {thread_id}"))
        })
        .await
    }

    pub async fn setting(&self, key: &str) -> Result<Option<serde_json::Value>> {
        let key = key.to_owned();
        self.read(move |conn| {
            read::setting(conn, &key).with_context(|| format!("read setting {key}"))
        })
        .await
    }

    pub async fn draft(&self, scope: &str) -> Result<Option<serde_json::Value>> {
        let scope = scope.to_owned();
        self.read(move |conn| {
            read::draft(conn, &scope).with_context(|| format!("read draft {scope}"))
        })
        .await
    }

    pub async fn devices(&self) -> Result<Vec<Device>> {
        self.read(|conn| {
            let mut statement = conn.prepare(
                "SELECT id, name, public_key, created_at, last_seen FROM authorized_keys
                 ORDER BY created_at",
            )?;
            let devices = statement
                .query_map([], |row| {
                    Ok(Device {
                        id: DeviceId(row.get(0)?),
                        name: row.get(1)?,
                        public_key: row.get(2)?,
                        created_at: row.get(3)?,
                        last_seen: row.get(4)?,
                    })
                })?
                .collect::<rusqlite::Result<_>>()?;
            Ok(devices)
        })
        .await
        .context("list devices")
    }

    pub async fn is_authorized(&self, public_key: &str) -> Result<Option<DeviceId>> {
        let public_key = public_key.to_owned();
        self.read(move |conn| {
            Ok(conn
                .query_row(
                    "SELECT id FROM authorized_keys WHERE public_key = ?1",
                    [public_key],
                    |row| row.get(0),
                )
                .optional()?
                .map(DeviceId))
        })
        .await
        .context("look up device key")
    }

    // Blobs.

    /// Stores WebP bytes by content. Rejected input fails with a
    /// [`BlobRejected`] inside the error.
    pub async fn put_blob(&self, bytes: Vec<u8>) -> Result<BlobRef> {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || blobs::put(&inner.blob_dir, &bytes))
            .await
            .context("blob write task")?
    }

    /// Stores any common image format, converted to WebP first.
    pub async fn put_image(&self, bytes: Vec<u8>) -> Result<BlobRef> {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || blobs::put(&inner.blob_dir, &blobs::to_webp(bytes)?))
            .await
            .context("image write task")?
    }

    /// Where a stored blob lives; `None` if the hash is malformed or unknown.
    #[must_use]
    pub fn blob_path(&self, hash: &BlobHash) -> Option<PathBuf> {
        blobs::path(&self.inner.blob_dir, hash).filter(|path| path.is_file())
    }

    /// Deletes blob files no item or draft references. Returns how many.
    pub async fn gc_blobs(&self) -> Result<usize> {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || inner.gc_blobs())
            .await
            .context("blob collection task")?
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
            None => self.database.reader()?,
        };
        let result = query(&conn);
        let mut readers = self.readers.lock().unwrap_or_else(PoisonError::into_inner);
        if readers.len() < READERS_KEPT {
            readers.push(conn);
        }
        result
    }

    fn gc_blobs(&self) -> Result<usize> {
        let referenced = self.with_reader(|conn| {
            let mut hashes = HashSet::new();
            let bodies: Vec<serde_json::Value> = read::many(
                conn,
                "SELECT body FROM items
                    WHERE json_extract(body, '$.kind.type') IN ('user_message', 'image')
                 UNION ALL SELECT body FROM drafts",
                [],
            )?;
            for body in &bodies {
                blobs::collect_hashes(body, &mut hashes);
            }
            Ok(hashes)
        })?;
        blobs::gc(&self.blob_dir, &referenced)
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
        native_id: new.native_id,
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
