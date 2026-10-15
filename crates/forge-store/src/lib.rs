//! Forge's durable state: one SQLite file plus content-addressed blobs.
//!
//! Every stored change (project, thread, item, run, setting) takes the next
//! value of one global seq. A single writer thread commits changes and
//! publishes them to [`Store::subscribe`] in commit order; reads go through a
//! small pool of read connections.

pub mod admin;
mod blobs;
mod collection;
mod content;
#[cfg(any(feature = "legacy-import", test))]
mod import_writes;
#[cfg(feature = "legacy-import")]
pub mod legacy;
mod read;
mod schema;
mod source_index;
mod writer;

use anyhow::{Context, Result, anyhow, ensure};
use artisan_api::{
    BlobHash, BlobRef, CreateProject, CreateThread, DeviceId, EngineId, Envelope, Event, Item,
    ItemId, ItemKind, ItemsPage, ItemsQuery, Project, ProjectId, Run, RunId, RunStatus,
    SendMessage, Seq, Status, Thread, ThreadId, Timestamp, UpdateThread, now,
};
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;
use tokio::sync::{Semaphore, broadcast, mpsc, oneshot};
use writer::{Job, Tx, Writer};

pub use blobs::{BlobRejected, MAX_BLOB_BYTES};

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
    revocations: broadcast::Sender<DeviceId>,
    head: Arc<AtomicU64>,
    live_revision: AtomicU64,
    readers: Mutex<Vec<Connection>>,
    read_slots: Arc<Semaphore>,
    source_index_slot: Arc<Semaphore>,
    database: PathBuf,
    blob_dir: PathBuf,
    blob_lock: Mutex<()>,
    inspector_cache:
        Mutex<std::collections::HashMap<ThreadId, (artisan_api::InspectorSummary, usize)>>,
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
                source_index_slot: Arc::new(Semaphore::new(1)),
                database,
                blob_dir,
                blob_lock: Mutex::new(()),
                inspector_cache: Mutex::new(std::collections::HashMap::new()),
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

    // Writes.

    /// Creating an existing id returns the stored project unchanged.
    pub async fn create_project(&self, request: CreateProject) -> Result<Project> {
        let id = request.id.clone();
        self.write(move |tx| tx.create_project(request))
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
        let id = request.id.clone();
        self.write(move |tx| tx.create_thread(request))
            .await
            .with_context(|| format!("create thread {id}"))
    }

    /// `None` if the thread does not exist.
    pub async fn update_thread(
        &self,
        id: &ThreadId,
        update: UpdateThread,
    ) -> Result<Option<Thread>> {
        let id = id.clone();
        self.write(move |tx| tx.update_thread(id, update)).await
    }

    pub async fn threads_page(
        &self,
        query: artisan_api::ThreadsQuery,
    ) -> Result<artisan_api::ThreadsPage> {
        self.read(move |conn| {
            let tx = conn.unchecked_transaction()?;
            let page = read::threads_page(&tx, &query)?;
            tx.commit()?;
            Ok(page)
        })
        .await
    }

    pub async fn inspector(&self, thread: &ThreadId) -> Result<artisan_api::InspectorSummary> {
        let thread = thread.clone();
        let inner = self.inner.clone();
        self.read(move |conn| read::inspector(conn, &thread, &inner.inspector_cache))
            .await
    }

    pub async fn mark_unread(&self, id: &ThreadId) -> Result<Option<Thread>> {
        let id = id.clone();
        self.write(move |tx| {
            let Some(mut thread) = read::thread(tx, &id)? else {
                return Ok(None);
            };
            thread.marked_unread = true;
            tx.save_thread(thread).map(Some)
        })
        .await
    }

    pub async fn cancel_pending_inputs(&self, thread: ThreadId, through: Seq) -> Result<()> {
        self.write(move |tx| {
            let inputs: Vec<Item> = read::items(
                tx,
                "SELECT body,native_id FROM items WHERE thread_id=?1 AND position<=?2 AND status='running'
                    AND json_extract(body,'$.kind.type')='user_message'",
                params![thread.as_str(), through],
            )?;
            let mut activity = None;
            for mut input in inputs {
                input.status = Status::Cancelled;
                activity = Some(tx.save_item(input, false)?.seq);
            }
            if let Some(seq) = activity {
                tx.mark_activity(&thread, seq)?;
            }
            Ok(())
        })
        .await
    }

    pub fn subscribe_revocations(&self) -> broadcast::Receiver<DeviceId> {
        self.inner.revocations.subscribe()
    }

    pub async fn settle_thread(
        &self,
        id: &ThreadId,
        observed_seq: artisan_api::Seq,
    ) -> Result<Option<Thread>> {
        let id = id.clone();
        self.write(move |tx| {
            let Some(mut thread) = read::thread(tx, &id)? else {
                return Ok(None);
            };
            thread.settled_seq = thread
                .settled_seq
                .max(observed_seq.min(thread.activity_seq));
            tx.save_thread(thread).map(Some)
        })
        .await
    }

    /// Inserts the item or updates it by id. Position and creation time never
    /// change, and a terminal status never reopens.
    pub async fn upsert_item(&self, new: NewItem) -> Result<Item> {
        let (id, thread_id) = (new.id.clone(), new.thread_id.clone());
        self.write(move |tx| {
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
                        context_used,
                        model,
                        ..
                    },
                    ItemKind::Agent {
                        label: old_label,
                        prompt: old_prompt,
                        agent_id: old_agent_id,
                        context_used: old_context,
                        model: old_model,
                        ..
                    },
                ) = (&mut kind, &old.kind)
                {
                    for (slot, old) in [
                        (label, old_label),
                        (agent_id, old_agent_id),
                        (model, old_model),
                    ] {
                        if slot.is_none() {
                            slot.clone_from(old);
                        }
                    }
                    if prompt.is_none() {
                        prompt.clone_from(old_prompt);
                    }
                    if context_used.is_none() {
                        *context_used = *old_context;
                    }
                }
                let status = old.next_status(new.status);
                let item = Item {
                    // An item stays in the run that made it: a resumed agent
                    // reports from a later run, but its row stays where it
                    // was launched, in its own turn.
                    run_id: old.run_id.or(new.run_id),
                    parent_id: new.parent_id.or(old.parent_id),
                    native_id: new.native_id.or(old.native_id),
                    status,
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

    /// Inserts accepted input once and records its thread activity in the same commit.
    pub async fn insert_user_message(&self, new: NewItem) -> Result<(Item, bool)> {
        let blob_dir = self.inner.blob_dir.clone();
        self.write(move |tx| {
            let (item, created) = tx.insert_input(new, &blob_dir)?;
            if created {
                tx.mark_activity(&item.thread_id, item.seq)?;
            }
            Ok((item, created))
        })
        .await
    }

    /// Accepts one idempotent command, including config/title/activity changes.
    pub async fn send_message(
        &self,
        thread_id: ThreadId,
        message: SendMessage,
        steer: bool,
    ) -> Result<(Item, bool, EngineId)> {
        let blob_dir = self.inner.blob_dir.clone();
        self.write(move |tx| {
            let mut thread = read::thread(tx, &thread_id)?
                .ok_or_else(|| SendRejected::ThreadNotFound(thread_id.clone()))?;
            let engine = thread.config.engine;
            let (item, created) = tx.insert_input(NewItem {
                id: message.id, thread_id, run_id: None, parent_id: None, native_id: None,
                status: Status::Running,
                kind: ItemKind::UserMessage { text: message.text.clone().into(), images: message.images, steer },
            }, &blob_dir)?;
            // A replay has no config, title, activity or delivery side effect.
            if created {
                if let Some(config) = message.config { thread.config = config; }
                if thread.title.is_none() {
                    let title: String = message.text.lines().next().unwrap_or_default().trim().chars().take(80).collect();
                    if !title.is_empty() { thread.title = Some(title); }
                }
                thread.activity_seq = item.seq;
                tx.save_setting("default_engine_config".into(), serde_json::to_value(&thread.config)?)?;
                tx.save_thread(thread)?;
                if let Some(draft) = message.draft {
                    // Keep the tombstone/revision so an older request cannot resurrect the draft.
                    tx.execute("UPDATE drafts SET body='null', revision=revision+1 WHERE scope=?1 AND revision=?2",
                        params![draft.scope, draft.revision])?;
                }
            }
            Ok((item, created, engine))
        }).await
    }

    /// Accepts a local thread setting once, without creating pending model input.
    pub async fn thread_option_command(
        &self,
        thread_id: ThreadId,
        message: SendMessage,
        option: String,
        label: String,
        enabled: Option<bool>,
        live: bool,
    ) -> Result<Item> {
        self.write(move |tx| {
            let mut thread = read::thread(tx, &thread_id)?
                .ok_or_else(|| SendRejected::ThreadNotFound(thread_id.clone()))?;
            anyhow::ensure!(message.images.is_empty(), SendRejected::Payload(format!("{label} setting commands cannot include attachments")));
            let prefix = format!("{}\n\n{label} ", message.text);
            if let Some(existing) = read::item(tx, &message.id)? {
                let same = existing.thread_id == thread_id && matches!(&existing.kind,
                    ItemKind::Notice { kind: artisan_api::NoticeKind::Info, text }
                    if text.inline().is_some_and(|text| text.starts_with(&prefix)));
                anyhow::ensure!(same, SendRejected::IdConflict(message.id));
                return Ok(existing);
            }
            if let Some(config) = message.config { thread.config = config; }
            let current = thread.config.session_options.get(&option).and_then(serde_json::Value::as_bool).unwrap_or(false);
            let enabled = enabled.unwrap_or(!current);
            thread.config.session_options.insert(option, serde_json::json!(enabled));
            let state = if enabled { "requested on" } else { "requested off" };
            let when = if live { "Applies to the next run; the current run is unchanged." } else { "Applies to the next message." };
            let text = format!("{}{}. {} Effort is unchanged.", prefix, state, when);
            let new = NewItem { id: message.id, thread_id, run_id: None, parent_id: None,
                native_id: None, status: Status::Done,
                kind: ItemKind::Notice { kind: artisan_api::NoticeKind::Info, text: text.into() }, };
            let item = tx.save_item(fresh_item(new, tx.created_at()), true)?;
            thread.activity_seq = item.seq;
            tx.save_setting("default_engine_config".into(), serde_json::to_value(&thread.config)?)?;
            tx.save_thread(thread)?;
            if let Some(draft) = message.draft {
                tx.execute("UPDATE drafts SET body='null', revision=revision+1 WHERE scope=?1 AND revision=?2",
                    params![draft.scope, draft.revision])?;
            }
            Ok(item)
        }).await
    }

    pub async fn upsert_run(&self, run: Run) -> Result<Run> {
        let (id, thread_id) = (run.id.clone(), run.thread_id.clone());
        self.write(move |tx| tx.upsert_run(run))
            .await
            .with_context(|| format!("save run {id} of thread {thread_id}"))
    }

    /// Delivery acknowledgment is atomic for the entire initial batch or one steer.
    pub async fn accept_inputs(
        &self,
        thread: ThreadId,
        run: RunId,
        ids: Vec<ItemId>,
    ) -> Result<()> {
        self.write(move |tx| {
            for id in ids {
                if let Some(mut item) = read::item(tx, &id)? {
                    ensure!(
                        item.thread_id == thread
                            && matches!(item.kind, ItemKind::UserMessage { .. }),
                        "input {id} does not belong to thread {thread}"
                    );
                    if item.status == Status::Running {
                        item.run_id = Some(run.clone());
                        item.status = Status::Done;
                        tx.save_item(item, false)?;
                    }
                }
            }
            Ok(())
        })
        .await
    }

    /// Status changes read the durable kind, instead of retaining another full copy in a run.
    pub async fn set_item_status(&self, id: ItemId, status: Status) -> Result<Option<Item>> {
        self.write(move |tx| {
            let Some(mut item) = read::item(tx, &id)? else {
                return Ok(None);
            };
            item.status = item.next_status(status);
            tx.save_item(item, false).map(Some)
        })
        .await
    }

    /// All remaining outputs and the final run commit together; activity changes once.
    pub async fn settle_run(&self, run: Run, open: Vec<ItemId>) -> Result<Run> {
        self.write(move |tx| {
            let status = if run.status == RunStatus::Done {
                Status::Done
            } else {
                Status::Cancelled
            };
            for id in open {
                if let Some(mut item) = read::item(tx, &id)?
                    && !item.status.is_terminal()
                {
                    ensure!(
                        item.thread_id == run.thread_id,
                        "output belongs to another thread"
                    );
                    if run.status != RunStatus::Cancelled
                        && item.status == Status::AwaitingUser
                        && matches!(
                            item.kind,
                            ItemKind::Question {
                                blocking: false,
                                ..
                            }
                        )
                    {
                        continue;
                    }
                    // A process exit closes any unanswered blocking interaction;
                    // completion of the run does not imply a callback was answered.
                    item.status = if item.status == Status::AwaitingUser {
                        Status::Cancelled
                    } else {
                        status
                    };
                    tx.save_item(item, false)?;
                }
            }
            let run = tx.save_run(run)?;
            tx.mark_activity(&run.thread_id, run.seq)?;
            Ok(run)
        })
        .await
    }

    /// Answers an asynchronous question after its process exits, as a durable next-turn input.
    pub async fn answer_question(
        &self,
        id: ItemId,
        answers: Vec<Option<artisan_api::QuestionAnswer>>,
    ) -> Result<Item> {
        let blob_dir = self.inner.blob_dir.clone();
        self.write(move |tx| {
            let mut item =
                read::item(tx, &id)?.with_context(|| format!("question {id} not found"))?;
            let ItemKind::Question {
                blocking: false,
                questions,
                answers: saved,
                ..
            } = &mut item.kind
            else {
                anyhow::bail!("question requires its live engine");
            };
            collection::validate_answers(tx, questions, &answers)?;
            let input_id = ItemId(format!("answer-{id}"));
            if item.status.is_terminal() {
                ensure!(
                    collection::answers_equal(tx, saved, &answers)?,
                    "question was already answered differently"
                );
                return read::item(tx, &input_id)?.context("question answer input is missing");
            }
            ensure!(
                item.status == Status::AwaitingUser,
                "question is not waiting for an answer"
            );
            let mut text = String::from("Answers to the agent's questions:\n");
            collection::visit_questions(tx, questions, |index, question| {
                text.push_str(&content::resolve(tx, &question.text)?);
                text.push_str("\nAnswer: ");
                match &answers[index] {
                    Some(answer) => text.push_str(&collection::answer_text(tx, question, answer)?),
                    None => text.push_str("Skipped"),
                }
                text.push_str("\n\n");
                Ok(())
            })?;
            *saved = Some(answers.into());
            item.status = Status::Done;
            tx.save_item(item.clone(), false)?;
            let (input, _) = tx.insert_input(
                NewItem {
                    id: input_id,
                    thread_id: item.thread_id,
                    run_id: None,
                    parent_id: None,
                    native_id: None,
                    status: Status::Running,
                    kind: ItemKind::UserMessage {
                        text: text.into(),
                        images: Vec::new(),
                        steer: false,
                    },
                },
                &blob_dir,
            )?;
            tx.mark_activity(&input.thread_id, input.seq)?;
            Ok(input)
        })
        .await
    }

    /// Compare and change one MCP entry inside the canonical setting transaction.
    pub async fn mutate_mcp(
        &self,
        mutation: artisan_api::McpMutation,
    ) -> Result<Vec<artisan_api::McpServer>> {
        self.write(move |tx| {
            use artisan_api::McpMutation;
            let mut servers: Vec<artisan_api::McpServer> = read::setting(tx, "mcp_servers")?
                .map(serde_json::from_value)
                .transpose()?
                .unwrap_or_default();
            let previous = match &mutation {
                McpMutation::Upsert { previous, .. } => previous.as_ref(),
                McpMutation::Remove { previous } => Some(previous),
            };
            if let Some(previous) = previous {
                ensure!(
                    servers.iter().find(|server| server.name == previous.name) == Some(previous),
                    "MCP entry changed; reload before saving"
                );
            }
            if let McpMutation::Upsert { server, .. } = &mutation {
                ensure!(!server.name.trim().is_empty(), "MCP server needs a name");
                ensure!(
                    !server.command.trim().is_empty()
                        || server
                            .url
                            .as_ref()
                            .is_some_and(|url| !url.trim().is_empty()),
                    "MCP server needs a command or URL"
                );
                ensure!(
                    !servers.iter().any(|other| other.name == server.name
                        && previous.is_none_or(|old| old.name != other.name)),
                    "MCP name already exists"
                );
            }
            if let Some(previous) = previous {
                servers.retain(|server| server.name != previous.name);
            }
            if let McpMutation::Upsert { server, .. } = mutation {
                servers.push(server);
            }
            servers.sort_by(|a, b| a.name.cmp(&b.name));
            tx.save_setting("mcp_servers".into(), serde_json::to_value(&servers)?)?;
            Ok(servers)
        })
        .await
    }

    /// Changes the model against the latest config instead of a client's stale full snapshot.
    pub async fn set_default_model(
        &self,
        engine: EngineId,
        model: artisan_api::Model,
    ) -> Result<artisan_api::EngineConfig> {
        self.write(move |tx| {
            let mut config = read::setting(tx, "default_engine_config")?
                .map(serde_json::from_value::<artisan_api::EngineConfig>)
                .transpose()?
                .filter(|config| config.engine == engine)
                .unwrap_or_else(|| artisan_api::EngineConfig::new(engine));
            model.normalize_context_window(&mut config);
            config.session_options.clear();
            config.model = Some(model.id);
            if config
                .effort
                .as_ref()
                .is_some_and(|effort| !model.efforts.contains(effort))
            {
                config.effort = None;
            }
            if config
                .speed
                .as_ref()
                .is_some_and(|speed| !model.speeds.iter().any(|option| &option.id == speed))
            {
                config.speed = None;
            }
            if config
                .variant
                .as_ref()
                .is_some_and(|variant| !model.variants.iter().any(|option| &option.id == variant))
            {
                config.variant = None;
            }
            tx.save_setting(
                "default_engine_config".into(),
                serde_json::to_value(&config)?,
            )?;
            Ok(config)
        })
        .await
    }

    pub async fn set_skill_enabled(&self, id: String, enabled: bool) -> Result<()> {
        self.write(move |tx| {
            let mut disabled: std::collections::BTreeSet<String> =
                read::setting(tx, "disabled_skills")?
                    .map(serde_json::from_value)
                    .transpose()?
                    .unwrap_or_default();
            if enabled {
                disabled.remove(&id);
            } else {
                disabled.insert(id);
            }
            tx.save_setting("disabled_skills".into(), serde_json::to_value(disabled)?)
        })
        .await
    }

    /// Metadata invalidation is small and durable, independent of a text value changing.
    pub async fn bump_revision(&self, key: String) -> Result<u64> {
        self.write(move |tx| {
            let revision = read::setting(tx, &key)?
                .and_then(|value| value.as_u64())
                .unwrap_or(0)
                .checked_add(1)
                .context("revision overflow")?;
            tx.save_setting(key, serde_json::json!(revision))?;
            Ok(revision)
        })
        .await
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
            tx.save_setting("global_prompt_version".into(), serde_json::json!(version))?;
            Ok(Ok(version))
        })
        .await
        .context("save the global prompt")
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

    pub async fn secret(&self, key: &str) -> Result<Option<String>> {
        let key = key.to_owned();
        self.read(move |conn| {
            Ok(conn
                .query_row("SELECT value FROM secrets WHERE key = ?1", [&key], |row| {
                    row.get(0)
                })
                .optional()?)
        })
        .await
    }

    /// Consuming the invitation and authorizing its device are one decision.
    pub async fn pair_device(
        &self,
        code: String,
        name: String,
        certificate: String,
    ) -> Result<Option<DeviceId>> {
        self.write(move |tx| {
            if tx.execute(
                "DELETE FROM pair_codes WHERE code_sha256 = ?1 AND expires_at > ?2",
                params![code, now()],
            )? == 0
            {
                return Ok(None);
            }
            let id: String = tx.query_row(
                "INSERT INTO authorized_keys (id, name, public_key, created_at)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(public_key) DO UPDATE SET name = excluded.name RETURNING id",
                params![DeviceId::new().as_str(), name, certificate, now()],
                |row| row.get(0),
            )?;
            Ok(Some(DeviceId(id)))
        })
        .await
        .context("pair device")
    }

    /// At most `limit` eligible threads; the pending Item index is the queue.
    pub async fn pending_threads(
        &self,
        limit: usize,
        excluded: Vec<ThreadId>,
    ) -> Result<Vec<ThreadId>> {
        let limit = limit.min(8);
        self.read(move |conn| read::pending_threads(conn, limit, &excluded))
            .await
    }

    /// Compare-and-swap prevents delayed writes and clears from overwriting newer input.
    pub async fn set_draft(
        &self,
        scope: String,
        request: artisan_api::WriteDraft,
    ) -> Result<artisan_api::DraftWriteResult> {
        self.write(move |tx| {
            let current = read::draft(tx, &scope)?;
            if current.revision != request.expected_revision {
                return Ok(artisan_api::DraftWriteResult {
                    saved: false,
                    current,
                });
            }
            if current.body == request.body {
                return Ok(artisan_api::DraftWriteResult {
                    saved: true,
                    current,
                });
            }
            let current = artisan_api::DraftSnapshot {
                revision: current
                    .revision
                    .checked_add(1)
                    .context("draft revision exhausted")?,
                body: request.body,
            };
            tx.execute(
                "INSERT INTO drafts (scope, body, revision) VALUES (?1, ?2, ?3)
                ON CONFLICT (scope) DO UPDATE SET body=excluded.body, revision=excluded.revision",
                params![scope, read::to_body(&current.body)?, current.revision],
            )?;
            Ok(artisan_api::DraftWriteResult {
                saved: true,
                current,
            })
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
        let key = id.clone();
        let revoked = self
            .write(move |tx| {
                Ok(tx.execute("DELETE FROM authorized_keys WHERE id = ?1", [key.as_str()])? > 0)
            })
            .await?;
        if revoked {
            let _ = self.inner.revocations.send(id.clone());
        }
        Ok(revoked)
    }

    pub async fn touch_device(&self, id: &DeviceId) -> Result<()> {
        let id = id.clone();
        self.write(move |tx| {
            tx.execute(
                "UPDATE authorized_keys SET last_seen = ?2 WHERE id = ?1
                 AND (last_seen IS NULL OR last_seen < ?2 - 60000)",
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
            let page = read::threads_page(
                &tx,
                &artisan_api::ThreadsQuery {
                    limit: Some(200),
                    ..Default::default()
                },
            )?;
            let mut threads = page.threads;
            let ids = serde_json::to_string(
                &threads.iter().map(|thread| &thread.id).collect::<Vec<_>>(),
            )?;
            let pinned: Vec<Thread> = read::many(
                &tx,
                "SELECT body FROM threads WHERE id IN (
                    SELECT thread_id FROM runs WHERE status='running'
                    UNION SELECT thread_id FROM items WHERE status='awaiting_user')
                  AND id NOT IN (SELECT value FROM json_each(?1))",
                [ids],
            )?;
            threads.extend(pinned);
            let mut runs = page.runs;
            let live: Vec<Run> =
                read::many(&tx, "SELECT body FROM runs WHERE status='running'", [])?;
            for run in live {
                if !runs.iter().any(|known| known.id == run.id) {
                    runs.push(run);
                }
            }
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
                threads_next: page.next,
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

    /// A bounded root transcript or one agent's direct children, never the
    /// other agent transcripts that happen to share this thread's positions.
    pub async fn transcript_page(
        &self,
        thread_id: &ThreadId,
        parent: Option<&ItemId>,
        query: ItemsQuery,
    ) -> Result<ItemsPage> {
        let thread = thread_id.clone();
        let parent = parent.cloned();
        self.read(move |conn| read::scoped_items_page(conn, &thread, &query, Some(parent.as_ref())))
            .await
    }

    /// Every stored entity changed after `since`, in seq order, or `None` when
    /// there are more than `limit` (the client should refetch the state).
    pub async fn changes_since(&self, since: Seq, limit: usize) -> Result<Option<Vec<Envelope>>> {
        self.read(move |conn| {
            read::changes_since(conn, since, limit, false, None)
                .with_context(|| format!("read changes since {since}"))
        })
        .await
    }

    pub async fn changes_since_scoped(
        &self,
        since: Seq,
        limit: usize,
        summary: bool,
        observed: Option<ThreadId>,
    ) -> Result<Option<Vec<Envelope>>> {
        self.read(move |conn| read::changes_since(conn, since, limit, summary, observed.as_ref()))
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

    /// Bounded immutable source range; does not load or parse an Item body.
    pub async fn content_range(
        &self,
        id: &artisan_api::ContentId,
        query: artisan_api::ContentQuery,
    ) -> Result<Option<artisan_api::ContentRange>> {
        let id = id.clone();
        self.read(move |conn| content::range(conn, &id, query))
            .await
    }

    /// Pure immutable Markdown topology. Only this bounded range enters the wire.
    pub async fn source_index(
        &self,
        id: &artisan_api::ContentId,
        query: artisan_api::SourceIndexQuery,
    ) -> Result<Option<artisan_api::SourceIndexRange>> {
        ensure!(id.is_valid(), "invalid source identity");
        ensure!(
            query
                .limit
                .is_none_or(|n| (1..=artisan_api::SOURCE_INDEX_PAGE_LEAVES).contains(&n)),
            "invalid source index page limit"
        );
        let read_id = id.clone();
        let read_query = query.clone();
        if let Some(range) = self
            .read(move |conn| source_index::range(conn, &read_id, read_query))
            .await?
        {
            return Ok(Some(range));
        }
        // Permit survives cancellation inside the blocking worker and queued writer.
        let permit = Arc::new(self.inner.source_index_slot.clone().acquire_owned().await?);
        let read_id = id.clone();
        let read_query = query.clone();
        if let Some(range) = self
            .read(move |conn| source_index::range(conn, &read_id, read_query))
            .await?
        {
            return Ok(Some(range));
        }
        let read_id = id.clone();
        let worker_permit = permit.clone();
        let format = query.format;
        let prepared = self
            .read(move |conn| {
                let _permit = worker_permit;
                source_index::prepare(conn, &read_id, format)
            })
            .await?;
        let Some(prepared) = prepared else {
            return Ok(None);
        };
        self.write(move |tx| {
            let _permit = permit;
            source_index::persist(tx, prepared)
        })
        .await?;
        let id = id.clone();
        self.read(move |conn| source_index::range(conn, &id, query))
            .await
    }

    /// Indexed canonical collection records. Transcript reads never hydrate the
    /// complete collection, and each page has count and encoded-byte limits.
    pub async fn collection_range<T: serde::de::DeserializeOwned + Send + 'static>(
        &self,
        id: &artisan_api::ContentId,
        query: artisan_api::CollectionQuery,
    ) -> Result<Option<artisan_api::CollectionRange<T>>> {
        let id = id.clone();
        self.read(move |conn| collection::range(conn, &id, query))
            .await
    }

    /// Validate selected opaque IDs through the immutable record-key index.
    /// A native callback never needs to load every question option.
    pub async fn validate_question_answers(
        &self,
        id: &ItemId,
        answers: &[Option<artisan_api::QuestionAnswer>],
    ) -> Result<()> {
        let id = id.clone();
        let answers = answers.to_vec();
        self.read(move |conn| {
            let item = read::item(conn, &id)?.context("question item is missing")?;
            let ItemKind::Question { questions, .. } = item.kind else {
                anyhow::bail!("item is not a question")
            };
            collection::validate_answers(conn, &questions, &answers)
        })
        .await
    }

    /// Native callbacks use complete admitted canonical records. Rendering and
    /// state/bootstrap endpoints must use collection_range instead.
    pub async fn resolve_item_collections(&self, mut item: Item) -> Result<Item> {
        self.read(move |conn| {
            collection::hydrate_item(conn, &mut item.kind)?;
            Ok(item)
        })
        .await
    }

    /// Resolve only an already-admitted input or explicit execution/history read.
    /// Transcript pages, events and bootstrap never call this method.
    pub async fn resolve_text(&self, text: &artisan_api::TextContent) -> Result<String> {
        let text = text.clone();
        self.read(move |conn| content::resolve(conn, &text)).await
    }

    pub async fn resolve_item_text(&self, mut item: Item) -> Result<Item> {
        self.read(move |conn| {
            collection::hydrate_item(conn, &mut item.kind)?;
            item.kind.try_visit_texts_mut(|text| {
                *text = content::resolve(conn, text)?.into();
                Ok::<_, anyhow::Error>(())
            })?;
            Ok(item)
        })
        .await
    }

    pub async fn gc_content(&self) -> Result<usize> {
        self.write(|tx| content::collect(tx, now())).await
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
    pub async fn items_awaiting_user(&self) -> Result<Vec<artisan_api::PendingItem>> {
        self.read(|conn| read::items_awaiting_user(conn).context("read items awaiting the user"))
            .await
    }

    /// Reads a bounded initial batch and all observed pending input's sequence boundary in one snapshot.
    pub async fn pending_inputs(&self, thread_id: &ThreadId) -> Result<PendingInputs> {
        let thread = thread_id.clone();
        let blob_dir = self.inner.blob_dir.clone();
        self.read(move |conn| read::pending_inputs(conn, &thread, &blob_dir))
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

    pub async fn draft(&self, scope: &str) -> Result<artisan_api::DraftSnapshot> {
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
        tokio::task::spawn_blocking(move || {
            let _guard = inner
                .blob_lock
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            blobs::put(&inner.blob_dir, &bytes)
        })
        .await
        .context("blob write task")?
    }

    /// Stores any common image format, converted to WebP first.
    pub async fn put_image(&self, bytes: Vec<u8>) -> Result<BlobRef> {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            let bytes = blobs::to_webp(bytes)?;
            let _guard = inner
                .blob_lock
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            blobs::put(&inner.blob_dir, &bytes)
        })
        .await
        .context("image write task")?
    }

    pub async fn input_image_paths(&self, images: Vec<BlobHash>) -> Result<Vec<PathBuf>> {
        let dir = self.inner.blob_dir.clone();
        tokio::task::spawn_blocking(move || {
            blobs::input_images(&dir, &images).map(|(paths, _)| paths)
        })
        .await
        .context("read input image metadata task")?
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
            None => open_database(&self.database)?,
        };
        let result = query(&conn);
        let mut readers = self.readers.lock().unwrap_or_else(PoisonError::into_inner);
        if readers.len() < READERS_KEPT {
            readers.push(conn);
        }
        result
    }

    fn gc_blobs(&self) -> Result<usize> {
        let _guard = self
            .blob_lock
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        self.with_reader(|conn| {
            // Hold SQLite's standard write exclusion through reference discovery and deletion.
            let tx = rusqlite::Transaction::new_unchecked(
                conn,
                rusqlite::TransactionBehavior::Immediate,
            )?;
            let mut hashes = HashSet::new();
            // Only typed image ownership fields; prose and arbitrary 64-hex strings
            // are never decoded or mistaken for references.
            let mut statement = conn.prepare_cached(
                "SELECT image.value FROM items i, json_each(i.body, '$.kind.images') image
                    WHERE json_extract(i.body, '$.kind.type') = 'user_message'
                 UNION SELECT json_extract(body, '$.kind.blob') FROM items
                    WHERE json_extract(body, '$.kind.type') = 'image'
                 UNION SELECT image.value FROM drafts d, json_each(d.body, '$.images') image",
            )?;
            for hash in statement.query_map([], |row| row.get::<_, Option<String>>(0))? {
                if let Some(hash) = hash?
                    && BlobHash(hash.clone()).is_valid()
                {
                    hashes.insert(hash);
                }
            }
            drop(statement);
            let removed = blobs::gc(&self.blob_dir, &hashes)?;
            tx.commit()?;
            Ok(removed)
        })
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

#[cfg(test)]
mod activity_time_tests {
    use super::*;
    use crate::writer::Stamp;
    use artisan_api::{EngineConfig, Usage};

    #[tokio::test]
    async fn only_accepted_activity_changes_thread_recency() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let project = store
            .create_project(CreateProject {
                id: ProjectId::new(),
                path: "/activity-test".into(),
                name: None,
            })
            .await
            .unwrap();
        let thread = store
            .create_thread_at(
                CreateThread {
                    id: ThreadId::new(),
                    project_id: project.id,
                    title: None,
                    config: EngineConfig::new(EngineId::Codex),
                    parent_thread_id: None,
                },
                Stamp {
                    created_at: 10,
                    updated_at: 10,
                },
            )
            .await
            .unwrap();
        assert_eq!(thread.activity_at, 10);
        assert!(!thread.is_unread());
        let run = store
            .upsert_run_at(
                Run {
                    id: RunId::new(),
                    thread_id: thread.id.clone(),
                    engine: EngineId::Codex,
                    model: None,
                    input_cutoff: thread.activity_seq,
                    status: RunStatus::Running,
                    usage: Usage::default(),
                    session_id: None,
                    error: None,
                    seq: 0,
                    started_at: 20,
                    ended_at: None,
                },
                Stamp {
                    created_at: 20,
                    updated_at: 20,
                },
            )
            .await
            .unwrap();
        let running = store.thread(&thread.id).await.unwrap().unwrap();
        assert_eq!((running.activity_seq, running.activity_at), (run.seq, 20));
        let mut usage = run.clone();
        usage.usage.input = 100;
        store
            .upsert_run_at(
                usage.clone(),
                Stamp {
                    created_at: 20,
                    updated_at: 25,
                },
            )
            .await
            .unwrap();
        let unchanged = store.thread(&thread.id).await.unwrap().unwrap();
        assert_eq!(unchanged.activity_at, 20, "usage alone is not activity");
        usage.status = RunStatus::Done;
        usage.ended_at = Some(30);
        let done = store
            .upsert_run_at(
                usage,
                Stamp {
                    created_at: 20,
                    updated_at: 30,
                },
            )
            .await
            .unwrap();
        let finished = store.thread(&thread.id).await.unwrap().unwrap();
        assert_eq!(
            (finished.activity_seq, finished.activity_at),
            (done.seq, 30)
        );
        let edited = store
            .update_thread_at(
                &thread.id,
                UpdateThread {
                    title: Some("Renamed".into()),
                    archived: Some(true),
                    config: Some(EngineConfig::new(EngineId::Claude)),
                    read_seq: Some(done.seq),
                },
                Stamp {
                    created_at: 10,
                    updated_at: 80,
                },
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(edited.updated_at, 80);
        assert_eq!((edited.activity_seq, edited.activity_at), (done.seq, 30));
        assert!(!edited.is_unread());
        let unread = store.mark_unread(&thread.id).await.unwrap().unwrap();
        assert!(unread.is_unread());
        assert_eq!(unread.activity_at, 30);
        let settled = store
            .settle_thread(&thread.id, done.seq)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(settled.activity_at, 30);
        let input = SendMessage {
            id: ItemId::new(),
            text: "Next".into(),
            images: Vec::new(),
            config: None,
            draft: None,
        };
        let (item, accepted, _) = store
            .send_message(thread.id.clone(), input.clone(), false)
            .await
            .unwrap();
        assert!(accepted);
        let accepted_thread = store.thread(&thread.id).await.unwrap().unwrap();
        assert_eq!(accepted_thread.activity_seq, item.seq);
        assert_eq!(accepted_thread.activity_at, accepted_thread.updated_at);
        assert!(accepted_thread.activity_at > 30);
        assert!(
            !store
                .send_message(thread.id.clone(), input, false)
                .await
                .unwrap()
                .1
        );
        let replayed = store.thread(&thread.id).await.unwrap().unwrap();
        assert_eq!(
            replayed, accepted_thread,
            "replayed command does not reorder the thread"
        );
    }
}

#[cfg(test)]
mod blob_gc_race_tests {
    use super::*;
    use artisan_api::{EngineConfig, EngineId, ItemKind};

    fn image_bytes() -> Vec<u8> {
        webp::Encoder::from_rgb(&[21, 72, 131], 1, 1)
            .encode_lossless()
            .to_vec()
    }
    fn age(path: &Path) {
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(std::time::SystemTime::now() - std::time::Duration::from_hours(2))
            .unwrap();
    }
    async fn thread(store: &Store) -> ThreadId {
        let project = store
            .create_project(CreateProject {
                id: ProjectId::new(),
                path: "/temporary-blob-race".into(),
                name: None,
            })
            .await
            .unwrap();
        store
            .create_thread(CreateThread {
                id: ThreadId::new(),
                project_id: project.id,
                config: EngineConfig::new(EngineId::Claude),
                title: None,
                parent_thread_id: None,
            })
            .await
            .unwrap()
            .id
    }

    #[tokio::test]
    async fn reusing_an_old_blob_refreshes_grace_until_the_new_reference_commits() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(directory.path()).unwrap();
        let thread = thread(&store).await;
        let bytes = image_bytes();
        let blob = store.put_blob(bytes.clone()).await.unwrap();
        let path = store.blob_path(&blob.hash).unwrap();
        age(&path);
        assert_eq!(store.put_blob(bytes).await.unwrap(), blob);
        assert_eq!(
            store.gc_blobs().await.unwrap(),
            0,
            "reuse must protect an old blob before its new Item is persisted"
        );
        store
            .upsert_item(NewItem {
                id: ItemId::new(),
                thread_id: thread,
                run_id: None,
                parent_id: None,
                native_id: None,
                status: Status::Done,
                kind: ItemKind::Image {
                    blob: blob.hash.clone(),
                    caption: None,
                },
            })
            .await
            .unwrap();
        age(&path);
        assert_eq!(
            store.gc_blobs().await.unwrap(),
            0,
            "committed typed reference protects the blob after grace ends"
        );
        assert_eq!(std::fs::read(path).unwrap(), image_bytes());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn gc_waits_for_a_concurrent_sqlite_reference_transaction_before_discovery() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(directory.path()).unwrap();
        let thread = thread(&store).await;
        let blob = store.put_blob(image_bytes()).await.unwrap();
        let path = store.blob_path(&blob.hash).unwrap();
        age(&path);
        // Another writer already owns SQLite's normal write transaction. GC
        // must wait and discover its committed reference, rather than scan an
        // old snapshot while the new reference is still being attached.
        let mut connection = open_database(&store.inner.database).unwrap();
        let tx = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .unwrap();
        let gc_store = store.clone();
        let gc = tokio::spawn(async move { gc_store.gc_blobs().await });
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            let waiting = match store.inner.blob_lock.try_lock() {
                Ok(guard) => {
                    drop(guard);
                    false
                }
                Err(std::sync::TryLockError::WouldBlock) => true,
                Err(error) => panic!("blob GC lock failed: {error}"),
            };
            if waiting {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "GC did not reach its SQLite write exclusion"
            );
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
        assert!(!gc.is_finished());
        let item = fresh_item(
            NewItem {
                id: ItemId::new(),
                thread_id: thread,
                run_id: None,
                parent_id: None,
                native_id: None,
                status: Status::Done,
                kind: ItemKind::Image {
                    blob: blob.hash.clone(),
                    caption: None,
                },
            },
            artisan_api::now(),
        );
        tx.execute(
            "INSERT INTO items(id,thread_id,position,seq,status,body) VALUES(?1,?2,1,1,'done',?3)",
            rusqlite::params![
                item.id.0,
                item.thread_id.0,
                serde_json::to_string(&item).unwrap()
            ],
        )
        .unwrap();
        tx.commit().unwrap();
        assert_eq!(
            gc.await.unwrap().unwrap(),
            0,
            "reference committed while GC waited must be discovered before deletion"
        );
        assert_eq!(std::fs::read(path).unwrap(), image_bytes());
    }
}

#[cfg(test)]
mod context_window_tests {
    use super::*;
    use artisan_api::{ContextWindow, EngineConfig, Model};

    #[tokio::test]
    async fn saved_context_choice_survives_roundtrip_and_clears_on_incompatible_model() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(directory.path()).unwrap();
        let mut config = EngineConfig::new(EngineId::Codex);
        config
            .options
            .insert("context_window".to_owned(), serde_json::json!(1_000_000));
        store
            .set_setting(
                "default_engine_config".into(),
                serde_json::to_value(config).unwrap(),
            )
            .await
            .unwrap();
        let extended = Model {
            id: "extended".to_owned(),
            context_windows: vec![
                ContextWindow {
                    tokens: 272_000,
                    override_tokens: None,
                },
                ContextWindow {
                    tokens: 1_000_000,
                    override_tokens: Some(1_000_000),
                },
            ],
            ..Model::default()
        };
        let saved = store
            .set_default_model(EngineId::Codex, extended)
            .await
            .unwrap();
        assert_eq!(saved.context_window_override(), Some(1_000_000));
        let reread: EngineConfig = serde_json::from_value(
            store
                .setting("default_engine_config")
                .await
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(reread, saved);
        let standard = Model {
            id: "standard".to_owned(),
            context_windows: vec![ContextWindow {
                tokens: 400_000,
                override_tokens: None,
            }],
            ..Model::default()
        };
        let saved = store
            .set_default_model(EngineId::Codex, standard)
            .await
            .unwrap();
        assert!(!saved.options.contains_key("context_window"));
    }

    #[tokio::test]
    async fn invalid_context_config_is_rejected_before_thread_is_saved() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(directory.path()).unwrap();
        let project = store
            .create_project(CreateProject {
                id: ProjectId::new(),
                path: "/context-validation".into(),
                name: None,
            })
            .await
            .unwrap();
        for (engine, value) in [
            (EngineId::Codex, serde_json::json!(0)),
            (EngineId::Claude, serde_json::json!(100_000_001_u64)),
            (EngineId::Codex, serde_json::json!("1000000")),
            (EngineId::OpenCode, serde_json::json!(1_000_000)),
        ] {
            let mut config = EngineConfig::new(engine);
            config.options.insert("context_window".to_owned(), value);
            let result = store
                .create_thread(CreateThread {
                    id: ThreadId::new(),
                    project_id: project.id.clone(),
                    title: None,
                    config,
                    parent_thread_id: None,
                })
                .await;
            assert!(result.is_err());
        }
    }
}
