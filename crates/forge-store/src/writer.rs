use crate::content::ContentError;
use crate::error::StorageError;
use crate::read::{self, tag, to_body, to_value};
use crate::{
    CONTINUE_AFTER_RESTART_KEY, CreateThreadError, MAX_RESTART_CONTINUATIONS, RunMoved,
    SendRejected, project_name,
};
use artisan_api::{
    Continuation, CreateProject, CreateThread, Envelope, Event, Item, ItemId, ItemKind, Project,
    Run, RunId, RunOutcome, RunStatus, Seq, Status, Thread, ThreadId, Timestamp, UpdateThread, now,
};
use forge_content::ItemTexts as _;
use rusqlite::{Connection, OptionalExtension as _, Transaction, TransactionBehavior, params};
use std::ops::Deref;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::mpsc;

pub type Job = Box<dyn FnOnce(&mut Writer) + Send>;

pub const SETTINGS_BYTES: u64 = 8 * 1024 * 1024;

type Result<T, E = StorageError> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
pub enum WriteFailure {
    #[error("thread {thread} does not exist")]
    ThreadNotFound { thread: ThreadId },
    #[error("item {item} belongs to thread {stored}, not {requested}")]
    ItemMoved {
        item: ItemId,
        stored: ThreadId,
        requested: ThreadId,
    },
    #[error("item {item} is not a user message")]
    NotAnInput { item: ItemId },
    #[error("saving setting {key} would make the settings {bytes} bytes; the limit is 8 MiB")]
    SettingsFull { key: String, bytes: u64 },
    #[error(transparent)]
    RunMoved(#[from] RunMoved),
    #[error(transparent)]
    Content(#[from] ContentError),
    #[error(transparent)]
    Storage(#[from] StorageError),
}

impl From<rusqlite::Error> for WriteFailure {
    fn from(error: rusqlite::Error) -> Self {
        Self::Storage(error.into())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum InputFailure {
    #[error("the message was refused")]
    Rejected(#[source] SendRejected),
    #[error("read the metadata of image {}", path.display())]
    ImageMetadata {
        path: std::path::PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error(transparent)]
    Write(#[from] WriteFailure),
}

impl From<StorageError> for InputFailure {
    fn from(error: StorageError) -> Self {
        Self::Write(error.into())
    }
}

impl From<ContentError> for InputFailure {
    fn from(error: ContentError) -> Self {
        Self::Write(error.into())
    }
}

impl From<rusqlite::Error> for InputFailure {
    fn from(error: rusqlite::Error) -> Self {
        Self::Write(error.into())
    }
}

impl From<SendRejected> for InputFailure {
    fn from(rejected: SendRejected) -> Self {
        Self::Rejected(rejected)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum UpdateThreadFailure {
    #[error("the config sets a context window its engine does not offer")]
    InvalidConfig,
    #[error(transparent)]
    Storage(#[from] StorageError),
}

impl From<rusqlite::Error> for UpdateThreadFailure {
    fn from(error: rusqlite::Error) -> Self {
        Self::Storage(error.into())
    }
}

pub fn valid_config(config: &artisan_api::EngineConfig) -> bool {
    !config.options.contains_key("context_window")
        || (matches!(
            config.engine,
            artisan_api::EngineId::Claude | artisan_api::EngineId::Codex
        ) && config.context_window_override().is_some())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Durability {
    Acknowledged,
    Background,
}

impl Durability {
    const fn synchronous(self) -> &'static str {
        match self {
            Self::Acknowledged => "FULL",
            Self::Background => "NORMAL",
        }
    }
}

pub(crate) struct Commits {
    pub(crate) count: Arc<AtomicU64>,
    pub(crate) committed: std::sync::mpsc::SyncSender<()>,
}

impl Commits {
    fn note(&self) {
        self.count.fetch_add(1, Ordering::AcqRel);
        match self.committed.try_send(()) {
            Ok(()) | Err(std::sync::mpsc::TrySendError::Full(())) => {}
            Err(std::sync::mpsc::TrySendError::Disconnected(())) => {
                tracing::warn!("the store checkpointer has stopped; the WAL grows until restart");
            }
        }
    }
}

pub struct Writer {
    conn: Connection,
    durability: Durability,
    head: Arc<AtomicU64>,
    events: Option<crate::feed::Feed>,
    hot: Arc<crate::hot::HotPages>,
    commits: Commits,
}

impl Writer {
    pub(crate) fn new(
        conn: Connection,
        head: Arc<AtomicU64>,
        events: Option<crate::feed::Feed>,
        hot: Arc<crate::hot::HotPages>,
        commits: Commits,
    ) -> Result<Self> {
        let seq = read::max_seq(&conn)?;
        head.store(seq, Ordering::Release);
        let durability = Durability::Background;
        conn.pragma_update(None, "synchronous", durability.synchronous())?;
        Ok(Self {
            conn,
            durability,
            head,
            events,
            hot,
            commits,
        })
    }

    pub(crate) fn run(mut self, mut jobs: mpsc::Receiver<Job>) {
        while let Some(job) = jobs.blocking_recv() {
            job(&mut self);
        }
    }

    pub(crate) fn transact<R, E: From<StorageError>>(
        &mut self,
        durability: Durability,
        job: impl FnOnce(&mut Tx<'_>) -> Result<R, E>,
    ) -> Result<R, E> {
        if self.durability != durability {
            self.conn
                .pragma_update(None, "synchronous", durability.synchronous())
                .map_err(StorageError::from)?;
            self.durability = durability;
        }
        let sql = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StorageError::from)?;
        let seq = read::max_seq(&sql)?;
        let mut tx = Tx {
            sql,
            seq,
            events: Vec::new(),
            #[cfg(test)]
            stamp: None,
        };
        let result = job(&mut tx)?;
        let Tx {
            sql,
            seq,
            mut events,
            ..
        } = tx;
        events.sort_by_key(|envelope| envelope.seq);
        #[cfg(feature = "test-support")]
        crate::faults::before_commit(sql.path()).map_err(StorageError::from)?;
        sql.commit().map_err(StorageError::from)?;
        self.commits.note();
        self.hot.committed(&events);
        self.head.fetch_max(seq, Ordering::AcqRel);
        if let Some(feed) = &self.events {
            for envelope in events {
                feed.send(envelope);
            }
        }
        Ok(result)
    }
}

pub struct Tx<'c> {
    pub(crate) sql: Transaction<'c>,
    seq: Seq,
    events: Vec<Envelope>,
    #[cfg(test)]
    stamp: Option<Stamp>,
}

#[cfg(test)]
#[derive(Clone, Copy, Debug)]
pub(crate) struct Stamp {
    pub(crate) created_at: Timestamp,
    pub(crate) updated_at: Timestamp,
}

impl Deref for Tx<'_> {
    type Target = Connection;

    fn deref(&self) -> &Connection {
        &self.sql
    }
}

impl Tx<'_> {
    #[cfg(test)]
    pub(crate) fn with_stamp<R, E>(
        &mut self,
        stamp: Stamp,
        job: impl FnOnce(&mut Self) -> Result<R, E>,
    ) -> Result<R, E> {
        let previous = self.stamp.replace(stamp);
        let result = job(self);
        self.stamp = previous;
        result
    }

    pub(crate) fn create_project(&mut self, request: CreateProject) -> Result<Project> {
        if let Some(existing) = read::project(self, &request.id)? {
            return Ok(existing);
        }
        let name = request.name.unwrap_or_else(|| project_name(&request.path));
        self.save_project(Project {
            id: request.id,
            path: request.path,
            name,
            seq: 0,
            created_at: self.created_at(),
            removed: false,
        })
    }

    pub(crate) fn create_thread(
        &mut self,
        request: CreateThread,
    ) -> Result<Thread, CreateThreadError> {
        let storage = |source| CreateThreadError::Storage {
            thread: request.id.clone(),
            source,
        };
        if let Some(existing) = read::thread(self, &request.id).map_err(storage)? {
            return Ok(existing);
        }
        if !valid_config(&request.config) {
            return Err(CreateThreadError::InvalidConfig { thread: request.id });
        }
        let project = read::project(self, &request.project_id)
            .map_err(storage)?
            .ok_or_else(|| CreateThreadError::ProjectNotFound {
                thread: request.id.clone(),
                project: request.project_id.clone(),
            })?;
        if project.removed {
            return Err(CreateThreadError::ProjectRemoved {
                thread: request.id,
                project: request.project_id,
            });
        }
        self.check_lineage(&request)?;
        let created_at = self.created_at();
        let thread = request.id.clone();
        self.save_thread(Thread {
            id: request.id,
            project_id: request.project_id,
            title: request.title,
            config: request.config,
            activity_seq: 0,
            activity_at: created_at,
            read_seq: 0,
            marked_unread: false,
            settled_seq: 0,
            seq: 0,
            created_at,
            updated_at: created_at,
            archived: false,
            parent_thread_id: request.parent_thread_id,
            spawned_by: request.spawned_by,
            parked_until: None,
        })
        .map_err(|source| CreateThreadError::Storage { thread, source })
    }

    fn check_lineage(&self, request: &CreateThread) -> Result<(), CreateThreadError> {
        let thread = &request.id;
        let storage = |source| CreateThreadError::Storage {
            thread: thread.clone(),
            source,
        };
        let Some(parent) = &request.parent_thread_id else {
            return match &request.spawned_by {
                None => Ok(()),
                Some(agent) => Err(CreateThreadError::AgentWithoutParent {
                    thread: thread.clone(),
                    agent: agent.clone(),
                }),
            };
        };
        if parent == thread || read::thread(self, parent).map_err(storage)?.is_none() {
            return Err(CreateThreadError::ParentNotFound {
                thread: thread.clone(),
                parent: parent.clone(),
            });
        }
        let Some(agent) = &request.spawned_by else {
            return Ok(());
        };
        let runs_this_thread = read::item(self, agent)
            .map_err(storage)?
            .is_some_and(|item| {
                item.thread_id == *parent
                    && matches!(&item.kind, ItemKind::Agent { thread: runs, .. }
                    if runs.as_ref().is_none_or(|runs| runs == thread))
            });
        if runs_this_thread {
            Ok(())
        } else {
            Err(CreateThreadError::AgentNotInParent {
                thread: thread.clone(),
                parent: parent.clone(),
                agent: agent.clone(),
            })
        }
    }

    pub(crate) fn update_thread(
        &mut self,
        id: &ThreadId,
        update: UpdateThread,
    ) -> Result<Option<Thread>, UpdateThreadFailure> {
        if update
            .config
            .as_ref()
            .is_some_and(|config| !valid_config(config))
        {
            return Err(UpdateThreadFailure::InvalidConfig);
        }
        let Some(mut thread) = read::thread(self, id)? else {
            return Ok(None);
        };
        if let Some(title) = update.title {
            thread.title = Some(title);
        }
        if let Some(archived) = update.archived {
            thread.archived = archived;
            if archived && thread.parked_until.take().is_some() {
                self.unpark(id)?;
            }
        }
        if let Some(config) = update.config {
            thread.config = config;
        }
        if let Some(read_seq) = update.read_seq {
            let read_seq = read_seq.min(thread.activity_seq);
            thread.read_seq = thread.read_seq.max(read_seq);
            if read_seq >= thread.activity_seq {
                thread.marked_unread = false;
            }
        }
        Ok(Some(self.save_thread(thread)?))
    }

    pub(crate) fn upsert_run(&mut self, run: Run) -> Result<Run, WriteFailure> {
        let changed_status =
            read::run(self, &run.id)?.is_none_or(|old| old.status() != run.status());
        let run = self.save_run(run)?;
        if changed_status {
            self.mark_activity(&run.thread_id, run.seq)?;
        }
        Ok(run)
    }

    #[cfg_attr(
        not(test),
        expect(
            clippy::unused_self,
            reason = "the stamp override exists only for tests"
        )
    )]
    pub(crate) fn created_at(&self) -> Timestamp {
        #[cfg(test)]
        if let Some(stamp) = self.stamp {
            return stamp.created_at;
        }
        now()
    }

    #[cfg_attr(
        not(test),
        expect(
            clippy::unused_self,
            reason = "the stamp override exists only for tests"
        )
    )]
    fn updated_at(&self) -> Timestamp {
        #[cfg(test)]
        if let Some(stamp) = self.stamp {
            return stamp.updated_at;
        }
        now()
    }

    pub(crate) const fn next_seq(&mut self) -> Seq {
        self.seq += 1;
        self.seq
    }

    pub(crate) fn emit(&mut self, seq: Seq, event: Event) {
        self.events.push(Envelope {
            seq: Some(seq),
            live_revision: 0,
            event,
        });
    }

    pub(crate) fn save_project(&mut self, mut project: Project) -> Result<Project> {
        if let Some(old) = read::project(self, &project.id)? {
            project.seq = old.seq;
            if project == old {
                return Ok(old);
            }
        }
        project.seq = self.next_seq();
        self.sql
            .prepare_cached(include_str!("sql/state/save_project.sql"))?
            .execute(params![
                project.id.as_str(),
                project.seq,
                to_body(&project)?
            ])?;
        self.emit(
            project.seq,
            Event::Project {
                project: project.clone(),
            },
        );
        Ok(project)
    }

    pub(crate) fn save_thread(&mut self, mut thread: Thread) -> Result<Thread> {
        let old = read::thread(self, &thread.id)?;
        let is_new = old.is_none();
        let advances_activity = old
            .as_ref()
            .is_none_or(|old| thread.activity_seq > old.activity_seq);
        if let Some(old) = old {
            thread.activity_at = old.activity_at;
            thread.seq = old.seq;
            thread.updated_at = old.updated_at;
            if thread == old {
                return Ok(old);
            }
        }
        thread.seq = self.next_seq();
        if is_new && thread.activity_seq == 0 {
            thread.activity_seq = thread.seq;
            thread.read_seq = thread.seq;
        }
        thread.updated_at = self.updated_at();
        if advances_activity {
            thread.activity_at = thread.updated_at;
        }
        self.sql
            .prepare_cached(include_str!("sql/state/save_thread.sql"))?
            .execute(params![thread.id.as_str(), thread.seq, to_body(&thread)?])?;
        self.emit(
            thread.seq,
            Event::Thread {
                thread: thread.clone(),
            },
        );
        Ok(thread)
    }

    pub(crate) fn set_native_id(&self, id: &ItemId, native_id: Option<&str>) -> Result<()> {
        if let Some(native_id) = native_id {
            self.sql
                .prepare_cached(include_str!("sql/items/set_native_id.sql"))?
                .execute(params![id.as_str(), native_id])?;
        }
        Ok(())
    }

    pub(crate) fn save_item(
        &mut self,
        mut item: Item,
        old: Option<Item>,
    ) -> Result<Item, WriteFailure> {
        crate::content::normalize(self, &mut item, old.as_ref())?;
        let new = old.is_none();
        if let Some(old) = old {
            item.seq = old.seq;
            item.updated_at = old.updated_at;
            if item == old {
                return Ok(old);
            }
        } else {
            let exists: bool = self.sql.query_row(
                include_str!("sql/state/thread_exists.sql"),
                [item.thread_id.as_str()],
                |row| row.get(0),
            )?;
            if !exists {
                return Err(WriteFailure::ThreadNotFound {
                    thread: item.thread_id,
                });
            }
        }
        item.seq = self.next_seq();
        item.updated_at = self.updated_at();
        if new {
            item.position = item.seq;
        }
        let edits = match &item.kind {
            artisan_api::ItemKind::Tool { tool, .. } if item.status == Status::Done => {
                crate::content::edit_totals(self, tool)?
            }
            artisan_api::ItemKind::UserMessage { .. }
            | artisan_api::ItemKind::AssistantMessage { .. }
            | artisan_api::ItemKind::Tool { .. }
            | artisan_api::ItemKind::Agent { .. }
            | artisan_api::ItemKind::Question { .. }
            | artisan_api::ItemKind::Plan { .. }
            | artisan_api::ItemKind::Image { .. }
            | artisan_api::ItemKind::Compaction { .. }
            | artisan_api::ItemKind::Notice { .. }
            | artisan_api::ItemKind::Error { .. } => None,
        };
        let outline = read::outline_entry(self, &item)?;
        self.sql
            .prepare_cached(include_str!("sql/items/save.sql"))?
            .execute(params![
                item.id.as_str(),
                item.thread_id.as_str(),
                item.position,
                item.seq,
                item.run_id.as_ref().map(RunId::as_str),
                tag(&item.status),
                to_body(&item)?,
                edits.as_ref().map(|stats| stats.added),
                edits.as_ref().map(|stats| stats.removed),
                item.parent_id.as_ref().map(artisan_api::ItemId::as_str),
                item.folds_into_work(),
                outline,
            ])?;
        crate::content::own(self, &item)?;
        crate::recall::index(self, &item)?;
        self.emit(item.seq, Event::Item { item: item.clone() });

        Ok(item)
    }

    pub(crate) fn insert_input(
        &mut self,
        mut new: crate::NewItem,
        blob_dir: &std::path::Path,
    ) -> Result<(Item, bool), InputFailure> {
        new.kind
            .try_visit_texts_mut(|text| crate::content::normalize_text(self, text))?;
        if let Some(existing) = read::item(self, &new.id)? {
            let same = existing.thread_id == new.thread_id
                && matches!((&existing.kind, &new.kind),
                (artisan_api::ItemKind::UserMessage { text: a, images: ai, .. },
                 artisan_api::ItemKind::UserMessage { text: b, images: bi, .. }) if a == b && ai == bi);
            if !same {
                return Err(SendRejected::IdConflict(new.id).into());
            }
            return Ok((existing, false));
        }
        if let artisan_api::ItemKind::UserMessage { text, images, .. } = &new.kind {
            if text.byte_len() > 32 * 1024 * 1024 {
                return Err(SendRejected::TextTooLarge {
                    bytes: text.byte_len(),
                }
                .into());
            }
            crate::blobs::input_images(blob_dir, images)?;
        }
        let item = self.save_item(crate::fresh_item(new, self.created_at()), None)?;
        Ok((item, true))
    }

    pub(crate) fn accept_command(
        &mut self,
        mut thread: Thread,
        seq: Seq,
        remember_config: bool,
        draft: Option<artisan_api::DraftRevision>,
    ) -> Result<(), WriteFailure> {
        thread.activity_seq = seq;
        if remember_config {
            self.save_setting("default_engine_config".into(), to_value(&thread.config)?)?;
            self.remember_model_config(&thread.config)?;
        }
        self.save_thread(thread)?;
        if let Some(draft) = draft {
            self.sql.execute(
                include_str!("sql/settings/clear_draft.sql"),
                params![draft.scope, draft.revision],
            )?;
        }
        Ok(())
    }

    pub(crate) fn mark_activity(
        &mut self,
        id: &artisan_api::ThreadId,
        seq: Seq,
    ) -> Result<(), WriteFailure> {
        let mut thread = read::thread(self, id)?
            .ok_or_else(|| WriteFailure::ThreadNotFound { thread: id.clone() })?;
        thread.activity_seq = thread.activity_seq.max(seq);
        self.save_thread(thread)?;
        Ok(())
    }

    pub(crate) fn save_run(&mut self, mut run: Run) -> Result<Run, WriteFailure> {
        if let Some(old) = read::run(self, &run.id)? {
            if old.thread_id != run.thread_id {
                return Err(RunMoved {
                    run: run.id,
                    stored: old.thread_id,
                    requested: run.thread_id,
                }
                .into());
            }
            run.seq = old.seq;
            if run == old {
                return Ok(old);
            }
        }
        run.seq = self.next_seq();
        self.sql
            .prepare_cached(include_str!("sql/runs/save.sql"))?
            .execute(params![
                run.id.as_str(),
                run.thread_id.as_str(),
                run.seq,
                tag(&run.status()),
                to_body(&run)?,
            ])?;
        self.emit(run.seq, Event::Run { run: run.clone() });
        Ok(run)
    }

    pub(crate) fn remember_model_config(
        &mut self,
        config: &artisan_api::EngineConfig,
    ) -> Result<(), WriteFailure> {
        let mut harnesses = match read::setting(self, artisan_api::HARNESS_CONFIGS_KEY)? {
            Some(serde_json::Value::Object(configs)) => configs,
            _ => serde_json::Map::new(),
        };
        harnesses.insert(config.engine.to_string(), to_value(config)?);
        self.save_setting(
            artisan_api::HARNESS_CONFIGS_KEY.into(),
            serde_json::Value::Object(harnesses),
        )?;
        let Some(model) = &config.model else {
            return Ok(());
        };
        let mut configs = match read::setting(self, artisan_api::MODEL_CONFIGS_KEY)? {
            Some(serde_json::Value::Object(configs)) => configs,
            _ => serde_json::Map::new(),
        };
        configs.retain(|_, value| {
            serde_json::from_value::<artisan_api::EngineConfig>(value.clone())
                .map_or(true, |saved| {
                    saved.engine != config.engine || saved.model.as_ref() != Some(model)
                })
        });
        configs.insert(
            artisan_api::model_config_key(config.engine, model),
            to_value(config)?,
        );
        self.save_setting(
            artisan_api::MODEL_CONFIGS_KEY.into(),
            serde_json::Value::Object(configs),
        )
    }

    pub(crate) fn save_setting(
        &mut self,
        key: String,
        value: serde_json::Value,
    ) -> Result<(), WriteFailure> {
        let old = read::setting(self, &key)?;
        if old.as_ref() == Some(&value) {
            return Ok(());
        }
        let body = to_body(&value)?;
        let old_bytes = old
            .as_ref()
            .map(to_body)
            .transpose()?
            .map_or(0, |old| old.len());
        if body.len() > old_bytes {
            let stored: u64 =
                self.query_row(include_str!("sql/settings/bytes.sql"), [], |row| row.get(0))?;
            let bytes = (stored + body.len() as u64).saturating_sub(old_bytes as u64);
            if bytes > SETTINGS_BYTES {
                return Err(WriteFailure::SettingsFull { key, bytes });
            }
        }
        let seq = self.next_seq();
        self.sql
            .prepare_cached(include_str!("sql/settings/save.sql"))?
            .execute(params![key, seq, body])?;
        if value == serde_json::Value::Bool(false) {
            for continuation in [Continuation::Overloaded, Continuation::UsageLimit] {
                if crate::park_setting(continuation) == Some(key.as_str()) {
                    self.unpark_all(continuation)?;
                }
            }
        }
        self.emit(seq, Event::Setting { key, value });
        Ok(())
    }

    pub(crate) fn recover_threads(
        &mut self,
        threads: Option<Vec<ThreadId>>,
    ) -> Result<(), WriteFailure> {
        let scope = threads.map(|threads| to_body(&threads)).transpose()?;
        let ended_at = now();
        let runs: Vec<Run> = read::many(self, include_str!("sql/recovery/runs.sql"), [&scope])?;
        let interrupted = runs.len();
        let mut continued = 0;
        for mut run in runs {
            let error = run.error().map(str::to_owned);
            run.finish(RunOutcome::Interrupted, error, ended_at);
            let run = self.save_run(run)?;
            if self.continue_after_restart(&run)? {
                continued += 1;
            }
        }
        let items: Vec<Item> = read::items(self, include_str!("sql/recovery/items.sql"), [&scope])?;
        let mut cancelled = 0;
        for mut item in items {
            if matches!(
                item.kind,
                artisan_api::ItemKind::Question {
                    blocking: false,
                    ..
                }
            ) && item.status == Status::AwaitingUser
            {
                continue;
            }
            if matches!(
                item.kind,
                artisan_api::ItemKind::Agent {
                    thread: Some(_),
                    ..
                }
            ) {
                continue;
            }
            let old = item.clone();
            item.status = item.status.advance(Status::Cancelled);
            self.save_item(item, Some(old))?;
            cancelled += 1;
        }
        if interrupted + cancelled > 0 {
            tracing::info!(
                interrupted,
                cancelled,
                continued,
                "recovered runs left live by the last Forge"
            );
        }
        Ok(())
    }

    pub(crate) fn continue_after_restart(&self, run: &Run) -> Result<bool> {
        let enabled = read::setting(self, CONTINUE_AFTER_RESTART_KEY)?
            .and_then(|value| value.as_bool())
            .unwrap_or(true);
        if !enabled || !self.may_carry_on(run)? {
            return Ok(false);
        }
        self.request_continue(&run.thread_id, Continuation::Restart)?;
        Ok(true)
    }

    fn may_carry_on(&self, run: &Run) -> Result<bool> {
        if run.session_id.is_none() {
            return Ok(false);
        }
        let Some(thread) = read::thread(self, &run.thread_id)? else {
            return Ok(false);
        };
        if thread.archived {
            return Ok(false);
        }
        let pending: bool = self.query_row(
            include_str!("sql/items/has_running_input.sql"),
            [run.thread_id.as_str()],
            |row| row.get(0),
        )?;
        if pending {
            return Ok(false);
        }
        if run.status() != RunStatus::Interrupted {
            return Ok(true);
        }
        let chain = self.continuation_chain(
            &run.thread_id,
            Continuation::Restart,
            MAX_RESTART_CONTINUATIONS,
        )?;
        Ok(chain < MAX_RESTART_CONTINUATIONS)
    }

    pub(crate) fn request_continue(
        &self,
        thread: &artisan_api::ThreadId,
        continuation: Continuation,
    ) -> Result<()> {
        self.sql
            .prepare_cached(include_str!("sql/continues/request.sql"))?
            .execute(params![thread.as_str(), self.seq, tag(&continuation)])?;
        Ok(())
    }

    pub(crate) fn park(&mut self, run: &Run, park: crate::Park) -> Result<bool> {
        if !self.may_carry_on(run)? {
            return Ok(false);
        }
        self.sql
            .prepare_cached(include_str!("sql/continues/park.sql"))?
            .execute(params![
                run.thread_id.as_str(),
                self.seq,
                tag(&park.continuation),
                park.due_at
            ])?;
        self.set_parked(&run.thread_id, Some(park.due_at))?;
        Ok(true)
    }

    pub(crate) fn unpark(&mut self, thread: &artisan_api::ThreadId) -> Result<bool> {
        let dropped = self
            .sql
            .prepare_cached(include_str!("sql/continues/unpark.sql"))?
            .execute([thread.as_str()])?;
        self.set_parked(thread, None)?;
        Ok(dropped > 0)
    }

    fn unpark_all(&mut self, continuation: Continuation) -> Result<()> {
        let threads: Vec<String> = self
            .sql
            .prepare_cached(include_str!("sql/continues/parked_for.sql"))?
            .query_map([tag(&continuation)], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        for thread in threads {
            self.unpark(&artisan_api::ThreadId(thread))?;
        }
        Ok(())
    }

    fn set_parked(
        &mut self,
        thread: &artisan_api::ThreadId,
        until: Option<artisan_api::Timestamp>,
    ) -> Result<()> {
        if let Some(mut saved) = read::thread(self, thread)?
            && saved.parked_until != until
        {
            saved.parked_until = until;
            self.save_thread(saved)?;
        }
        Ok(())
    }

    pub(crate) fn continuation_chain(
        &self,
        thread: &artisan_api::ThreadId,
        continuation: Continuation,
        limit: usize,
    ) -> Result<usize> {
        let recent: Vec<Run> = read::many(
            self,
            include_str!("sql/runs/chain.sql"),
            params![thread.as_str(), limit],
        )?;
        Ok(recent
            .iter()
            .take_while(|run| run.continuation == Some(continuation))
            .count())
    }

    pub(crate) fn take_continue(
        &mut self,
        thread: &artisan_api::ThreadId,
    ) -> Result<Option<Continuation>> {
        let reason: Option<String> = self
            .sql
            .prepare_cached(include_str!("sql/continues/take.sql"))?
            .query_row([thread.as_str()], |row| row.get(0))
            .optional()?;
        self.set_parked(thread, None)?;
        reason.map(read::untag).transpose()
    }
}
