use crate::error::AtStep as _;
use crate::error::{Staged, StorageError, operation_error};
use crate::writer::{UpdateThreadFailure, WriteFailure};
use crate::{CreateThreadError, Snapshot, Store, read};
use artisan_api::{
    CreateProject, CreateThread, Envelope, Project, ProjectId, Run, RunId, Seq, Thread, ThreadId,
    UpdateThread,
};

pub const MAX_LIVE_PROJECTS: usize = 1000;
pub const MAX_PROJECT_PATH_BYTES: usize = 4096;
pub const MAX_PROJECT_NAME_BYTES: usize = 256;
pub const STATE_BYTES: usize = 24 * 1024 * 1024;
const STATE_THREADS: u32 = 200;
const PINNED_THREADS: usize = 200;
const LIVE_RUNS: usize = 400;

#[derive(Debug, thiserror::Error)]
pub enum CreateProjectFailure {
    #[error("Forge already has {limit} projects")]
    TooMany { limit: usize },
    #[error("the project path is {bytes} bytes; the limit is 4096")]
    PathTooLong { bytes: usize },
    #[error("the project name is {bytes} bytes; the limit is 256")]
    NameTooLong { bytes: usize },
    #[error(transparent)]
    Storage(#[from] StorageError),
}

operation_error!(CreateProjectError(CreateProjectFailure), "create project {project}", project: ProjectId);
operation_error!(RemoveProjectError(StorageError), "remove project {project}", project: ProjectId);
operation_error!(UpdateThreadError(UpdateThreadFailure), "update thread {thread}", thread: ThreadId);
operation_error!(ReplaceTitleError(WriteFailure), "replace the title of thread {thread}", thread: ThreadId);
operation_error!(
    ThreadsPageError(read::ThreadsPageFailure),
    "page the threads"
);
operation_error!(MarkUnreadError(WriteFailure), "mark thread {thread} unread", thread: ThreadId);
operation_error!(SettleThreadError(WriteFailure), "settle thread {thread} at seq {seq}", thread: ThreadId, seq: Seq);
operation_error!(UpsertRunError(WriteFailure), "save run {run} of thread {thread}", run: RunId, thread: ThreadId);
operation_error!(
    StateError(StorageError),
    "read the state a client starts from"
);
operation_error!(ChangesSinceError(StorageError), "read the changes since seq {since}", since: Seq);
operation_error!(ReadProjectError(StorageError), "read project {project}", project: ProjectId);
operation_error!(ReadThreadError(StorageError), "read thread {thread}", thread: ThreadId);
operation_error!(ReadRunError(StorageError), "read run {run}", run: RunId);
operation_error!(LatestRunError(StorageError), "read the latest run of thread {thread}", thread: ThreadId);
operation_error!(InspectorError(crate::ContentError), "summarize thread {thread} for the inspector", thread: ThreadId);
operation_error!(ThreadTitlesError(StorageError), "read the thread titles");
operation_error!(AgentThreadsError(StorageError), "read the agent threads of thread {thread}", thread: ThreadId);
operation_error!(WorkingAgentsError(StorageError), "read the working agents");
operation_error!(LastReplyError(crate::ContentError), "read the last reply of thread {thread}", thread: ThreadId);
operation_error!(AwaitsUserError(StorageError), "read whether thread {thread} waits on the user", thread: ThreadId);

impl Store {
    pub async fn create_project(
        &self,
        request: CreateProject,
    ) -> Result<Project, CreateProjectError> {
        let project = request.id.clone();
        self.write_acknowledged(move |tx| {
            if request.path.len() > MAX_PROJECT_PATH_BYTES {
                return Err(CreateProjectFailure::PathTooLong {
                    bytes: request.path.len(),
                });
            }
            if let Some(name) = request
                .name
                .as_ref()
                .filter(|name| name.len() > MAX_PROJECT_NAME_BYTES)
            {
                return Err(CreateProjectFailure::NameTooLong { bytes: name.len() });
            }
            if read::project(tx, &request.id)?.is_none() {
                let live: usize = tx
                    .query_row(include_str!("sql/state/live_projects.sql"), [], |row| {
                        row.get(0)
                    })
                    .at("state/live_projects.sql")?;
                if live >= MAX_LIVE_PROJECTS {
                    return Err(CreateProjectFailure::TooMany {
                        limit: MAX_LIVE_PROJECTS,
                    });
                }
            }
            Ok(tx.create_project(request)?)
        })
        .await
        .map_err(|cause| CreateProjectError {
            project,
            cause: Box::new(cause),
        })
    }

    pub async fn remove_project(
        &self,
        id: &ProjectId,
    ) -> Result<Option<Project>, RemoveProjectError> {
        let project = id.clone();
        let id = id.clone();
        self.write_acknowledged(move |tx| {
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
        .map_err(|cause| RemoveProjectError {
            project,
            cause: Box::new(cause),
        })
    }

    pub async fn create_thread(&self, request: CreateThread) -> Result<Thread, CreateThreadError> {
        let thread = request.id.clone();
        self.write_acknowledged(move |tx| tx.create_thread(request).map_err(Staged::Failed))
            .await
            .map_err(|staged| match staged {
                Staged::Failed(error) => error,
                Staged::Storage(source) => CreateThreadError::Storage { thread, source },
            })
    }

    pub async fn update_thread(
        &self,
        id: &ThreadId,
        update: UpdateThread,
    ) -> Result<Option<Thread>, UpdateThreadError> {
        let thread = id.clone();
        let id = id.clone();
        self.write_acknowledged(move |tx| tx.update_thread(&id, update))
            .await
            .map_err(|cause| UpdateThreadError {
                thread,
                cause: Box::new(cause),
            })
    }

    pub async fn replace_thread_title(
        &self,
        id: &ThreadId,
        expected: Option<String>,
        title: String,
    ) -> Result<Option<Thread>, ReplaceTitleError> {
        let thread = id.clone();
        let id = id.clone();
        self.write_acknowledged(move |tx| {
            let Some(mut thread) = read::thread(tx, &id)? else {
                return Ok(None);
            };
            if thread.title != expected {
                return Ok(None);
            }
            thread.title = Some(title);
            Ok(Some(tx.save_thread(thread)?))
        })
        .await
        .map_err(|cause| ReplaceTitleError {
            thread,
            cause: Box::new(cause),
        })
    }

    pub async fn threads_page(
        &self,
        query: artisan_api::ThreadsQuery,
    ) -> Result<artisan_api::ThreadsPage, ThreadsPageError> {
        self.read(move |conn| {
            let tx = conn.unchecked_transaction().at("begin a read snapshot")?;
            let page = read::threads_page(&tx, &query)?;
            tx.commit().at("end the read snapshot")?;
            Ok(page)
        })
        .await
        .map_err(|cause| ThreadsPageError {
            cause: Box::new(cause),
        })
    }

    pub async fn mark_unread(&self, id: &ThreadId) -> Result<Option<Thread>, MarkUnreadError> {
        let thread = id.clone();
        let id = id.clone();
        self.write_acknowledged(move |tx| {
            let Some(mut thread) = read::thread(tx, &id)? else {
                return Ok(None);
            };
            thread.marked_unread = true;
            Ok(Some(tx.save_thread(thread)?))
        })
        .await
        .map_err(|cause| MarkUnreadError {
            thread,
            cause: Box::new(cause),
        })
    }

    pub async fn settle_thread(
        &self,
        id: &ThreadId,
        observed_seq: Seq,
    ) -> Result<Option<Thread>, SettleThreadError> {
        let thread = id.clone();
        let id = id.clone();
        self.write(move |tx| {
            let Some(mut thread) = read::thread(tx, &id)? else {
                return Ok(None);
            };
            thread.settled_seq = thread
                .settled_seq
                .max(observed_seq.min(thread.activity_seq));
            Ok(Some(tx.save_thread(thread)?))
        })
        .await
        .map_err(|cause| SettleThreadError {
            thread,
            seq: observed_seq,
            cause: Box::new(cause),
        })
    }

    pub async fn upsert_run(&self, run: Run) -> Result<Run, UpsertRunError> {
        let (id, thread) = (run.id.clone(), run.thread_id.clone());
        self.write(move |tx| tx.upsert_run(run))
            .await
            .map_err(|cause| UpsertRunError {
                run: id,
                thread,
                cause: Box::new(cause),
            })
    }

    pub async fn state(&self) -> Result<Snapshot, StateError> {
        let head = self.head();
        self.read(move |conn| {
            let tx = conn.unchecked_transaction().at("begin a read snapshot")?;
            let snapshot = snapshot(&tx, head)?;
            tx.commit().at("end the read snapshot")?;
            Ok(snapshot)
        })
        .await
        .map_err(|cause| StateError {
            cause: Box::new(cause),
        })
    }

    pub async fn changes_since(
        &self,
        since: Seq,
        limit: usize,
    ) -> Result<Option<Vec<Envelope>>, ChangesSinceError> {
        self.read(move |conn| read::changes_since(conn, since, limit, false, None))
            .await
            .map_err(|cause| ChangesSinceError {
                since,
                cause: Box::new(cause),
            })
    }

    pub async fn changes_since_scoped(
        &self,
        since: Seq,
        limit: usize,
        summary: bool,
        observed: Option<ThreadId>,
    ) -> Result<Option<Vec<Envelope>>, ChangesSinceError> {
        self.read(move |conn| read::changes_since(conn, since, limit, summary, observed.as_ref()))
            .await
            .map_err(|cause| ChangesSinceError {
                since,
                cause: Box::new(cause),
            })
    }

    pub async fn project(&self, id: &ProjectId) -> Result<Option<Project>, ReadProjectError> {
        let project = id.clone();
        let id = id.clone();
        self.read(move |conn| read::project(conn, &id))
            .await
            .map_err(|cause| ReadProjectError {
                project,
                cause: Box::new(cause),
            })
    }

    pub async fn thread(&self, id: &ThreadId) -> Result<Option<Thread>, ReadThreadError> {
        let thread = id.clone();
        let id = id.clone();
        self.read(move |conn| read::thread(conn, &id))
            .await
            .map_err(|cause| ReadThreadError {
                thread,
                cause: Box::new(cause),
            })
    }

    pub async fn run(&self, id: &RunId) -> Result<Option<Run>, ReadRunError> {
        let run = id.clone();
        let id = id.clone();
        self.read(move |conn| read::run(conn, &id))
            .await
            .map_err(|cause| ReadRunError {
                run,
                cause: Box::new(cause),
            })
    }

    pub async fn latest_run(&self, thread_id: &ThreadId) -> Result<Option<Run>, LatestRunError> {
        let thread = thread_id.clone();
        let thread_id = thread_id.clone();
        self.read(move |conn| read::latest_run(conn, &thread_id))
            .await
            .map_err(|cause| LatestRunError {
                thread,
                cause: Box::new(cause),
            })
    }

    pub async fn inspector(
        &self,
        thread: &ThreadId,
    ) -> Result<artisan_api::InspectorSummary, InspectorError> {
        self.inspector_since(thread, None).await
    }

    pub async fn inspector_since(
        &self,
        thread: &ThreadId,
        known_outline: Option<Seq>,
    ) -> Result<artisan_api::InspectorSummary, InspectorError> {
        let id = thread.clone();
        let thread = thread.clone();
        self.scan(move |conn| read::inspector(conn, &id, known_outline))
            .await
            .map_err(|cause| InspectorError {
                thread,
                cause: Box::new(cause),
            })
    }

    pub async fn thread_titles(&self) -> Result<Vec<(ThreadId, String)>, ThreadTitlesError> {
        self.read(move |conn| {
            let mut statement = conn
                .prepare_cached(include_str!("sql/state/titles.sql"))
                .at("state/titles.sql")?;
            let rows = statement
                .query_map([], |row| {
                    Ok((ThreadId::from(row.get::<_, String>(0)?), row.get(1)?))
                })
                .at("state/titles.sql")?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .at("state/titles.sql")
        })
        .await
        .map_err(|cause| ThreadTitlesError {
            cause: Box::new(cause),
        })
    }

    pub async fn agent_threads(&self, parent: &ThreadId) -> Result<Vec<Thread>, AgentThreadsError> {
        let thread = parent.clone();
        let parent = parent.clone();
        self.read(move |conn| {
            read::many(
                conn,
                "state/agent_threads.sql",
                include_str!("sql/state/agent_threads.sql"),
                [parent.as_str()],
            )
        })
        .await
        .map_err(|cause| AgentThreadsError {
            thread,
            cause: Box::new(cause),
        })
    }

    pub async fn working_agents(&self) -> Result<Vec<artisan_api::Item>, WorkingAgentsError> {
        self.read(|conn| {
            read::items(
                conn,
                "state/working_agents.sql",
                include_str!("sql/state/working_agents.sql"),
                [],
            )
        })
        .await
        .map_err(|cause| WorkingAgentsError {
            cause: Box::new(cause),
        })
    }

    pub async fn last_reply(&self, thread: &ThreadId) -> Result<Option<String>, LastReplyError> {
        let id = thread.clone();
        let thread = thread.clone();
        self.read(move |conn| {
            let item = read::items(
                conn,
                "state/last_reply.sql",
                include_str!("sql/state/last_reply.sql"),
                [id.as_str()],
            )?
            .pop();
            match item.map(|item| item.kind) {
                Some(artisan_api::ItemKind::AssistantMessage { text, .. }) => {
                    Ok(Some(crate::content::resolve(conn, &text)?))
                }
                _ => Ok(None),
            }
        })
        .await
        .map_err(|cause| LastReplyError {
            thread,
            cause: Box::new(cause),
        })
    }

    pub async fn awaits_user(&self, thread: &ThreadId) -> Result<bool, AwaitsUserError> {
        let id = thread.clone();
        let thread = thread.clone();
        self.read(move |conn| {
            conn.query_row(
                include_str!("sql/items/awaits_user.sql"),
                [id.as_str()],
                |row| row.get(0),
            )
            .at("items/awaits_user.sql")
        })
        .await
        .map_err(|cause| AwaitsUserError {
            thread,
            cause: Box::new(cause),
        })
    }
}

fn encoded_len<T: serde::Serialize + ?Sized>(value: &T) -> Result<usize, StorageError> {
    read::to_body(value).map(|body| body.len())
}

fn snapshot(conn: &rusqlite::Connection, head: Seq) -> Result<Snapshot, StorageError> {
    let projects: Vec<Project> = read::many(
        conn,
        "state/projects.sql",
        include_str!("sql/state/projects.sql"),
        [],
    )?;
    let mut settings = serde_json::Map::new();
    let mut statement = conn
        .prepare(include_str!("sql/state/settings.sql"))
        .at("state/settings.sql")?;
    let mut rows = statement.query([]).at("state/settings.sql")?;
    while let Some(row) = rows.next().at("state/settings.sql")? {
        let key: String = row.get(0).at("state/settings.sql")?;
        let value: String = row.get(1).at("state/settings.sql")?;
        settings.insert(key, read::from_body(&value)?);
    }
    drop(rows);
    drop(statement);
    let page = read::threads_page(
        conn,
        &artisan_api::ThreadsQuery {
            limit: Some(STATE_THREADS),
            ..Default::default()
        },
    )
    .map_err(|failure| match failure {
        read::ThreadsPageFailure::Storage(error) => error,
        read::ThreadsPageFailure::SearchTooLong { .. } => {
            unreachable!("invariant: the state's thread page has no search")
        }
    })?;
    let ids = read::to_body(
        &page
            .threads
            .iter()
            .map(|thread| &thread.id)
            .collect::<Vec<_>>(),
    )?;
    let pinned: Vec<Thread> = read::many(
        conn,
        "state/pinned_threads.sql",
        include_str!("sql/state/pinned_threads.sql"),
        rusqlite::params![ids, PINNED_THREADS],
    )?;
    let live: Vec<Run> = read::many(
        conn,
        "state/live_runs.sql",
        include_str!("sql/state/live_runs.sql"),
        [LIVE_RUNS],
    )?;
    let mut used = encoded_len(&projects)?
        .saturating_add(encoded_len(&settings)?)
        .saturating_add(encoded_len(&pinned)?)
        .saturating_add(encoded_len(&live)?);
    let mut threads = Vec::with_capacity(page.threads.len() + pinned.len());
    let mut runs: Vec<Run> = Vec::with_capacity(page.runs.len() + live.len());
    let mut threads_next = page.next;
    let mut latest = page.runs;
    for thread in page.threads {
        let run = latest
            .iter()
            .position(|run| run.thread_id == thread.id)
            .map(|index| latest.swap_remove(index));
        let bytes = encoded_len(&thread)?
            .saturating_add(run.as_ref().map(encoded_len).transpose()?.unwrap_or(0));
        if used.saturating_add(bytes) > STATE_BYTES && !threads.is_empty() {
            threads_next = threads
                .last()
                .map(|kept: &Thread| artisan_api::ThreadCursor {
                    activity_seq: kept.activity_seq,
                    id: kept.id.clone(),
                });
            break;
        }
        used = used.saturating_add(bytes);
        threads.push(thread);
        runs.extend(run);
    }
    threads.extend(pinned);
    for run in live {
        if !runs.iter().any(|known| known.id == run.id) {
            runs.push(run);
        }
    }
    Ok(Snapshot {
        head,
        projects,
        threads,
        threads_next,
        runs,
        settings,
        database_rollback: crate::rollbacks::current(conn)?,
    })
}

#[cfg(test)]
#[path = "../tests/threads/activity_time.rs"]
mod activity_time_tests;
