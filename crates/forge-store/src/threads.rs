//! Projects, threads and runs, and the state a client starts from.

use crate::{Snapshot, Store, read};
use anyhow::{Context, Result};
use artisan_api::{
    CreateProject, CreateThread, Envelope, Project, ProjectId, Run, RunId, Seq, Thread, ThreadId,
    UpdateThread,
};

impl Store {
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
        self.write(move |tx| tx.update_thread(&id, update)).await
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

    pub async fn upsert_run(&self, run: Run) -> Result<Run> {
        let (id, thread_id) = (run.id.clone(), run.thread_id.clone());
        self.write(move |tx| tx.upsert_run(run))
            .await
            .with_context(|| format!("save run {id} of thread {thread_id}"))
    }

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

    pub async fn inspector(&self, thread: &ThreadId) -> Result<artisan_api::InspectorSummary> {
        let thread = thread.clone();
        self.read(move |conn| read::inspector(conn, &thread)).await
    }

    /// Every titled thread's id and title, the most recently active first:
    /// what `@title` mentions resolve against.
    pub async fn thread_titles(&self) -> Result<Vec<(ThreadId, String)>> {
        self.read(move |conn| {
            let mut statement = conn.prepare_cached(
                "SELECT id, json_extract(body,'$.title') FROM threads
                    WHERE json_extract(body,'$.title') IS NOT NULL
                    ORDER BY json_extract(body,'$.activity_seq') DESC, seq DESC",
            )?;
            let rows = statement.query_map([], |row| {
                Ok((ThreadId::from(row.get::<_, String>(0)?), row.get(1)?))
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .context("read the thread titles")
        })
        .await
    }

    /// The threads running agents a thread started (`spawned_by` set).
    pub async fn agent_threads(&self, parent: &ThreadId) -> Result<Vec<Thread>> {
        let parent = parent.clone();
        self.read(move |conn| {
            read::many(
                conn,
                "SELECT body FROM threads WHERE json_extract(body,'$.parent_thread_id')=?1
                    AND json_extract(body,'$.spawned_by') IS NOT NULL ORDER BY seq",
                [parent.as_str()],
            )
        })
        .await
    }

    /// Every agent item still marked working, across threads.
    pub async fn working_agents(&self) -> Result<Vec<artisan_api::Item>> {
        self.read(|conn| {
            read::items(
                conn,
                "SELECT body FROM items WHERE kind_type='agent' AND status IN ('running','awaiting_user')
                    AND json_extract(body,'$.kind.thread') IS NOT NULL",
                [],
            )
        })
        .await
    }

    /// The thread's latest top-level reply, resolved.
    pub async fn last_reply(&self, thread: &ThreadId) -> Result<Option<String>> {
        let thread = thread.clone();
        let item = self
            .read(move |conn| {
                Ok(read::items(
                    conn,
                    "SELECT body FROM items WHERE thread_id=?1 AND kind_type='assistant_message'
                        AND parent_id IS NULL ORDER BY position DESC LIMIT 1",
                    [thread.as_str()],
                )?
                .pop())
            })
            .await?;
        match item.map(|item| item.kind) {
            Some(artisan_api::ItemKind::AssistantMessage { text, .. }) => {
                Ok(Some(self.resolve_text(&text).await?))
            }
            _ => Ok(None),
        }
    }

    /// Whether anything in the thread waits on the user.
    pub async fn awaits_user(&self, thread: &ThreadId) -> Result<bool> {
        let thread = thread.clone();
        self.read(move |conn| {
            Ok(conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM items WHERE thread_id=?1 AND status='awaiting_user')",
                [thread.as_str()],
                |row| row.get(0),
            )?)
        })
        .await
    }
}

#[cfg(test)]
mod activity_time_tests {
    use super::*;
    use crate::Acceptance;
    use crate::writer::Stamp;
    use artisan_api::{EngineConfig, EngineId, ItemId, RunStatus, SendMessage, Usage};

    #[tokio::test]
    #[expect(clippy::too_many_lines, reason = "one scenario walked step by step")]
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
                    spawned_by: None,
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
                    config: None,
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
                    continuation: None,
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
            .send_message(
                thread.id.clone(),
                input.clone(),
                false,
                Acceptance::default(),
            )
            .await
            .unwrap();
        assert!(accepted);
        let accepted_thread = store.thread(&thread.id).await.unwrap().unwrap();
        assert_eq!(accepted_thread.activity_seq, item.seq);
        assert_eq!(accepted_thread.activity_at, accepted_thread.updated_at);
        assert!(accepted_thread.activity_at > 30);
        assert!(
            !store
                .send_message(thread.id.clone(), input, false, Acceptance::default())
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
