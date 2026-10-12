//! The one writer: a thread with its own connection that runs each job in a
//! transaction and publishes the resulting envelopes in commit order.

use crate::read::{self, tag, to_body};
use anyhow::{Context, Result};
use artisan_api::{
    Envelope, Event, Item, Project, Run, RunId, RunStatus, Seq, Status, Thread, Timestamp, now,
};
use rusqlite::{Connection, Transaction, params};
use std::ops::Deref;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use tokio::sync::broadcast;

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

    pub(crate) fn run(mut self, jobs: &mpsc::Receiver<Job>) {
        while let Ok(job) = jobs.recv() {
            job(&mut self);
        }
    }

    /// Runs `job` in a transaction. Seqs it took become visible (head, events)
    /// only after the commit; a failed job rolls back and leaves no gap.
    pub(crate) fn transact<R>(&mut self, job: impl FnOnce(&mut Tx<'_>) -> Result<R>) -> Result<R> {
        self.transact_at(None, job)
    }

    /// [`Self::transact`] with the job's timestamps fixed to `stamp`.
    pub(crate) fn transact_at<R>(
        &mut self,
        stamp: Option<Stamp>,
        job: impl FnOnce(&mut Tx<'_>) -> Result<R>,
    ) -> Result<R> {
        let seq = self.seq;
        let mut tx = Tx {
            sql: self.conn.transaction().context("begin write transaction")?,
            seq,
            events: Vec::new(),
            stamp,
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
    stamp: Option<Stamp>,
}

/// Timestamps a write uses instead of the clock: the legacy import keeps the
/// old database's times this way.
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
    pub(crate) fn created_at(&self) -> Timestamp {
        self.stamp.map_or_else(now, |stamp| stamp.created_at)
    }

    fn updated_at(&self) -> Timestamp {
        self.stamp.map_or_else(now, |stamp| stamp.updated_at)
    }

    fn next_seq(&mut self) -> Seq {
        self.seq += 1;
        self.seq
    }

    fn emit(&mut self, seq: Seq, event: Event) {
        self.events.push(Envelope {
            seq: Some(seq),
            event,
        });
    }

    pub(crate) fn save_project(&mut self, mut project: Project) -> Result<Project> {
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
        thread.seq = self.next_seq();
        thread.updated_at = self.updated_at();
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

    /// Saves the item, then marks its thread active with the item's seq.
    /// A new item's position is the seq it was created at.
    pub(crate) fn save_item(&mut self, mut item: Item, new: bool) -> Result<Item> {
        item.seq = self.next_seq();
        item.updated_at = self.updated_at();
        if new {
            item.position = item.seq;
        }
        self.sql
            .prepare_cached(
                "INSERT INTO items (id, thread_id, position, seq, run_id, native_id, status, body)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT (id) DO UPDATE SET seq = excluded.seq, run_id = excluded.run_id,
                     native_id = excluded.native_id, status = excluded.status, body = excluded.body",
            )?
            .execute(params![
                item.id.as_str(),
                item.thread_id.as_str(),
                item.position,
                item.seq,
                item.run_id.as_ref().map(RunId::as_str),
                item.native_id,
                tag(&item.status),
                to_body(&item)?,
            ])
            .with_context(|| format!("save item {} in thread {}", item.id, item.thread_id))?;
        self.emit(item.seq, Event::Item { item: item.clone() });

        let mut thread = read::thread(self, &item.thread_id)?
            .with_context(|| format!("thread {} of item {} not found", item.thread_id, item.id))?;
        thread.activity_seq = item.seq;
        self.save_thread(thread)?;
        Ok(item)
    }

    pub(crate) fn save_run(&mut self, mut run: Run) -> Result<Run> {
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
                tag(&run.status),
                to_body(&run)?,
            ])
            .with_context(|| format!("save run {} of thread {}", run.id, run.thread_id))?;
        self.emit(run.seq, Event::Run { run: run.clone() });
        Ok(run)
    }

    pub(crate) fn save_setting(&mut self, key: String, value: serde_json::Value) -> Result<()> {
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

    /// Nothing runs after a restart: live runs become interrupted and items
    /// still waiting on an engine are cancelled. Bodies are re-saved through
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
        let items: Vec<Item> = read::many(
            self,
            "SELECT body FROM items WHERE status IN ('running', 'awaiting_user') ORDER BY position",
            [],
        )?;
        let cancelled = items.len();
        for mut item in items {
            item.status = item.status.advance(Status::Cancelled);
            self.save_item(item, false)?;
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
