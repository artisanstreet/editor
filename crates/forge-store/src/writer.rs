//! The one writer: a thread with its own connection that runs each job in a
//! transaction and publishes the resulting envelopes in commit order.

use crate::read::{self, tag, to_body};
use crate::{ProjectNotFound, project_name};
use anyhow::{Context, Result};
use artisan_api::{
    CreateProject, CreateThread, Envelope, Event, Item, Project, Run, RunId, RunStatus, Seq,
    Status, Thread, ThreadId, Timestamp, UpdateThread, now,
};
use rusqlite::{Connection, Transaction, params};
use std::ops::Deref;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::{broadcast, mpsc};

pub(crate) type Job = Box<dyn FnOnce(&mut Writer) + Send>;

pub(crate) struct Writer {
    conn: Connection,
    seq: Seq,
    head: Arc<AtomicU64>,
    events: broadcast::Sender<Envelope>,
}

impl Writer {
    pub(crate) fn new(
        conn: Connection,
        head: Arc<AtomicU64>,
        events: broadcast::Sender<Envelope>,
    ) -> Result<Self> {
        let seq = read::max_seq(&conn)?;
        head.store(seq, Ordering::Release);
        Ok(Self {
            conn,
            seq,
            head,
            events,
        })
    }

    pub(crate) fn run(mut self, mut jobs: mpsc::Receiver<Job>) {
        while let Some(job) = jobs.blocking_recv() {
            job(&mut self);
        }
    }

    /// Runs `job` in a transaction. Seqs it took become visible (head, events)
    /// only after the commit; a failed job rolls back and leaves no gap.
    pub(crate) fn transact<R>(&mut self, job: impl FnOnce(&mut Tx<'_>) -> Result<R>) -> Result<R> {
        let seq = self.seq;
        let mut tx = Tx {
            sql: self.conn.transaction().context("begin write transaction")?,
            seq,
            events: Vec::new(),
            #[cfg(any(feature = "legacy-import", test))]
            stamp: None,
        };
        let result = job(&mut tx)?;
        let Tx {
            sql, seq, events, ..
        } = tx;
        sql.commit().context("commit write transaction")?;
        self.seq = seq;
        self.head.store(seq, Ordering::Release);
        for envelope in events {
            // No subscribers is fine.
            let _ = self.events.send(envelope);
        }
        Ok(result)
    }
}

pub(crate) struct Tx<'c> {
    sql: Transaction<'c>,
    seq: Seq,
    events: Vec<Envelope>,
    #[cfg(any(feature = "legacy-import", test))]
    stamp: Option<Stamp>,
}

/// Timestamps a write uses instead of the clock: the legacy import keeps the
/// old database's times this way.
#[cfg(any(feature = "legacy-import", test))]
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
    #[cfg(any(feature = "legacy-import", test))]
    pub(crate) fn with_stamp<R>(
        &mut self,
        stamp: Stamp,
        job: impl FnOnce(&mut Self) -> Result<R>,
    ) -> Result<R> {
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

    pub(crate) fn create_thread(&mut self, request: CreateThread) -> Result<Thread> {
        if let Some(existing) = read::thread(self, &request.id)? {
            return Ok(existing);
        }
        if read::project(self, &request.project_id)?.is_none() {
            return Err(ProjectNotFound(request.project_id).into());
        }
        let created_at = self.created_at();
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
        })
    }

    pub(crate) fn update_thread(
        &mut self,
        id: ThreadId,
        update: UpdateThread,
    ) -> Result<Option<Thread>> {
        let Some(mut thread) = read::thread(self, &id)? else {
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
            let read_seq = read_seq.min(thread.activity_seq);
            thread.read_seq = thread.read_seq.max(read_seq);
            if read_seq >= thread.activity_seq {
                thread.marked_unread = false;
            }
        }
        self.save_thread(thread).map(Some)
    }

    pub(crate) fn upsert_run(&mut self, run: Run) -> Result<Run> {
        let changed_status = read::run(self, &run.id)?.is_none_or(|old| old.status != run.status);
        let run = self.save_run(run)?;
        if changed_status {
            self.mark_activity(&run.thread_id, run.seq)?;
        }
        Ok(run)
    }

    pub(crate) fn created_at(&self) -> Timestamp {
        #[cfg(any(feature = "legacy-import", test))]
        if let Some(stamp) = self.stamp {
            return stamp.created_at;
        }
        now()
    }

    fn updated_at(&self) -> Timestamp {
        #[cfg(any(feature = "legacy-import", test))]
        if let Some(stamp) = self.stamp {
            return stamp.updated_at;
        }
        now()
    }

    fn next_seq(&mut self) -> Seq {
        self.seq += 1;
        self.seq
    }

    fn emit(&mut self, seq: Seq, event: Event) {
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
            .prepare_cached(
                "INSERT INTO projects (id, seq, body) VALUES (?1, ?2, ?3)
                 ON CONFLICT (id) DO UPDATE SET seq = excluded.seq, body = excluded.body",
            )?
            .execute(params![
                project.id.as_str(),
                project.seq,
                to_body(&project)?
            ])
            .with_context(|| format!("save project {}", project.id))?;
        self.emit(
            project.seq,
            Event::Project {
                project: project.clone(),
            },
        );
        Ok(project)
    }

    pub(crate) fn save_thread(&mut self, mut thread: Thread) -> Result<Thread> {
        anyhow::ensure!(
            !thread.config.options.contains_key("context_window")
                || (matches!(
                    thread.config.engine,
                    artisan_api::EngineId::Claude | artisan_api::EngineId::Codex
                ) && thread.config.context_window_override().is_some()),
            "invalid context window override"
        );
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
            .prepare_cached(
                "INSERT INTO threads (id, seq, body) VALUES (?1, ?2, ?3)
                 ON CONFLICT (id) DO UPDATE SET seq = excluded.seq, body = excluded.body",
            )?
            .execute(params![thread.id.as_str(), thread.seq, to_body(&thread)?])
            .with_context(|| format!("save thread {}", thread.id))?;
        self.emit(
            thread.seq,
            Event::Thread {
                thread: thread.clone(),
            },
        );
        Ok(thread)
    }

    /// Saves only this item: `old` is its stored version, `None` for a new
    /// item, whose position is its creation seq. Thread activity belongs to
    /// accepted input and run boundaries.
    pub(crate) fn save_item(&mut self, mut item: Item, old: Option<Item>) -> Result<Item> {
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
                "SELECT EXISTS(SELECT 1 FROM threads WHERE id = ?1)",
                [item.thread_id.as_str()],
                |row| row.get(0),
            )?;
            anyhow::ensure!(exists, "thread {} not found", item.thread_id);
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
            _ => None,
        };
        // The kind's serde tag, read from the body so it cannot drift from it.
        self.sql
            .prepare_cached(
                "INSERT INTO items (id, thread_id, position, seq, run_id, native_id, status, body,
                     edit_added, edit_removed, kind_type, parent_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, json_extract(?8, '$.kind.type'), ?11)
                 ON CONFLICT (id) DO UPDATE SET seq = excluded.seq, run_id = excluded.run_id,
                     native_id = excluded.native_id, status = excluded.status, body = excluded.body,
                     edit_added = excluded.edit_added, edit_removed = excluded.edit_removed,
                     kind_type = excluded.kind_type, parent_id = excluded.parent_id",
            )?
            .execute(params![
                item.id.as_str(),
                item.thread_id.as_str(),
                item.position,
                item.seq,
                item.run_id.as_ref().map(RunId::as_str),
                item.native_id,
                tag(&item.status)?,
                to_body(&item)?,
                edits.as_ref().map(|stats| stats.added),
                edits.as_ref().map(|stats| stats.removed),
                item.parent_id.as_ref().map(artisan_api::ItemId::as_str),
            ])
            .with_context(|| format!("save item {} in thread {}", item.id, item.thread_id))?;
        crate::content::own(self, &item)?;
        self.emit(item.seq, Event::Item { item: item.clone() });

        Ok(item)
    }

    pub(crate) fn insert_input(
        &mut self,
        mut new: crate::NewItem,
        blob_dir: &std::path::Path,
    ) -> Result<(Item, bool)> {
        // Normalize the retry before equality: Inline producer bytes and an
        // existing immutable reference represent the same idempotent input.
        new.kind
            .try_visit_texts_mut(|text| crate::content::normalize_text(self, text))?;
        if let Some(existing) = read::item(self, &new.id)? {
            let same = existing.thread_id == new.thread_id
                && matches!((&existing.kind, &new.kind),
                (artisan_api::ItemKind::UserMessage { text: a, images: ai, .. },
                 artisan_api::ItemKind::UserMessage { text: b, images: bi, .. }) if a == b && ai == bi);
            anyhow::ensure!(same, crate::SendRejected::IdConflict(new.id));
            return Ok((existing, false));
        }
        if let artisan_api::ItemKind::UserMessage { text, images, .. } = &new.kind {
            if text.byte_len() > 32 * 1024 * 1024 {
                return Err(
                    crate::SendRejected::Payload("message text exceeds 32 MiB".into()).into(),
                );
            }
            crate::blobs::input_images(blob_dir, images)?;
        }
        let item = self.save_item(crate::fresh_item(new, self.created_at()), None)?;
        Ok((item, true))
    }

    /// What an accepted command does to its thread in the command's commit:
    /// activity, optionally the default config, and the draft it consumed.
    pub(crate) fn accept_command(
        &mut self,
        mut thread: Thread,
        seq: Seq,
        remember_config: bool,
        draft: Option<artisan_api::DraftRevision>,
    ) -> Result<()> {
        thread.activity_seq = seq;
        if remember_config {
            self.save_setting(
                "default_engine_config".into(),
                serde_json::to_value(&thread.config)?,
            )?;
        }
        self.save_thread(thread)?;
        if let Some(draft) = draft {
            // Keep the tombstone and revision so an older write cannot
            // resurrect the draft.
            self.sql.execute(
                "UPDATE drafts SET body='null', revision=revision+1 WHERE scope=?1 AND revision=?2",
                params![draft.scope, draft.revision],
            )?;
        }
        Ok(())
    }

    pub(crate) fn mark_activity(&mut self, id: &artisan_api::ThreadId, seq: Seq) -> Result<()> {
        let mut thread =
            read::thread(self, id)?.with_context(|| format!("thread {id} not found"))?;
        thread.activity_seq = thread.activity_seq.max(seq);
        self.save_thread(thread)?;
        Ok(())
    }

    pub(crate) fn save_run(&mut self, mut run: Run) -> Result<Run> {
        if let Some(old) = read::run(self, &run.id)? {
            run.seq = old.seq;
            if run == old {
                return Ok(old);
            }
        }
        run.seq = self.next_seq();
        self.sql
            .prepare_cached(
                "INSERT INTO runs (id, thread_id, seq, status, body) VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT (id) DO UPDATE SET seq = excluded.seq, status = excluded.status,
                     body = excluded.body",
            )?
            .execute(params![
                run.id.as_str(),
                run.thread_id.as_str(),
                run.seq,
                tag(&run.status)?,
                to_body(&run)?,
            ])
            .with_context(|| format!("save run {} of thread {}", run.id, run.thread_id))?;
        self.emit(run.seq, Event::Run { run: run.clone() });
        Ok(run)
    }

    pub(crate) fn save_setting(&mut self, key: String, mut value: serde_json::Value) -> Result<()> {
        if key == "default_engine_config" {
            let config: artisan_api::EngineConfig = serde_json::from_value(value)?;
            value = serde_json::to_value(config.preference_config())?;
        }
        if read::setting(self, &key)?.as_ref() == Some(&value) {
            return Ok(());
        }
        let seq = self.next_seq();
        self.sql
            .prepare_cached(
                "INSERT INTO kv (key, seq, value) VALUES (?1, ?2, ?3)
                 ON CONFLICT (key) DO UPDATE SET seq = excluded.seq, value = excluded.value",
            )?
            .execute(params![key, seq, to_body(&value)?])
            .with_context(|| format!("save setting {key}"))?;
        self.emit(seq, Event::Setting { key, value });
        Ok(())
    }

    /// Live runs become interrupted and unfinished engine output is cancelled.
    /// Accepted user input stays pending so startup can deliver it. Bodies are re-saved through
    /// the normal path so columns, bodies and seqs stay consistent and
    /// resuming clients see the change.
    pub(crate) fn recover(&mut self) -> Result<()> {
        let ended_at = now();
        let runs: Vec<Run> =
            read::many(self, "SELECT body FROM runs WHERE status = 'running'", [])?;
        let interrupted = runs.len();
        for mut run in runs {
            run.status = RunStatus::Interrupted;
            run.ended_at.get_or_insert(ended_at);
            self.save_run(run)?;
        }
        let items: Vec<Item> = read::items(
            self,
            "SELECT body,native_id FROM items WHERE status IN ('running', 'awaiting_user')
             AND kind_type != 'user_message' ORDER BY position",
            [],
        )?;
        let cancelled = items.len();
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
            let old = item.clone();
            item.status = item.status.advance(Status::Cancelled);
            self.save_item(item, Some(old))?;
        }
        if interrupted + cancelled > 0 {
            tracing::info!(
                interrupted,
                cancelled,
                "recovered runs left live by the last Forge"
            );
        }
        Ok(())
    }
}
