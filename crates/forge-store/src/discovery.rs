use crate::error::{StorageError, operation_error};
use crate::{Store, coordination::Record};
use artisan_api::{Project, ProjectId, Run, Thread, ThreadCursor, ThreadId};
use rusqlite::params;
use serde::{Deserialize, Serialize};

type Result<T, E = StorageError> = std::result::Result<T, E>;

operation_error!(FileActivityError(StorageError), "discover file activity");
operation_error!(DiscoverWorkersError(StorageError), "discover workers");

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileCursor {
    row_id: i64,
    line: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct FileActivity {
    pub thread: ThreadId,
    pub title: Option<String>,
    pub item: artisan_api::ItemId,
    pub operation: String,
    pub path: String,
    pub created_at: artisan_api::Timestamp,
}

#[derive(Clone, Debug, Serialize)]
pub struct FilePage {
    pub activity: Vec<FileActivity>,
    pub next_cursor: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct Query {
    pub project: Option<ProjectId>,
    pub parent: Option<ThreadId>,
    pub include_agents: bool,
    pub active: bool,
    pub live_threads: Vec<ThreadId>,
    pub archived: Option<bool>,
    pub unread: Option<bool>,
    pub title: String,
    pub target: Option<String>,
    pub cursor: Option<ThreadCursor>,
    pub worktree: Option<String>,
    pub limit: u32,
}

#[derive(Clone, Debug, Serialize)]
pub struct Worker {
    pub thread: Thread,
    pub project: Project,
    pub latest_run: Option<Run>,
    pub awaiting_user: bool,
    pub work: Option<Record>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Page {
    pub workers: Vec<Worker>,
    pub next_cursor: Option<String>,
}

impl Store {
    pub async fn discover_file_activity(
        &self,
        project: Option<ProjectId>,
        paths: Vec<String>,
        contains: bool,
        cursor: Option<FileCursor>,
        limit: u32,
    ) -> Result<FilePage, FileActivityError> {
        self.read(move |conn| {
            let limit = limit.clamp(1, 200) as usize;
            let mut statement = conn.prepare(include_str!("sql/coordination/files.sql"))?;
            let rows = statement.query_map(
                params![
                    project.map(|id| id.to_string()),
                    crate::read::to_body(&paths)?,
                    contains,
                    cursor.as_ref().map(|c| c.row_id),
                    cursor.as_ref().map(|c| &c.line),
                    limit + 1
                ],
                |row| {
                    Ok((
                        FileCursor {
                            row_id: row.get(0)?,
                            line: row.get(1)?,
                        },
                        FileActivity {
                            thread: ThreadId::from(row.get::<_, String>(2)?),
                            title: row.get(3)?,
                            item: artisan_api::ItemId::from(row.get::<_, String>(4)?),
                            operation: row.get(5)?,
                            path: row.get(6)?,
                            created_at: row.get(7)?,
                        },
                    ))
                },
            )?;
            let mut entries = rows.collect::<rusqlite::Result<Vec<_>>>()?;
            let next_cursor = if entries.len() > limit {
                entries.truncate(limit);
                entries
                    .last()
                    .map(|entry| crate::read::to_body(&entry.0))
                    .transpose()?
            } else {
                None
            };
            Ok(FilePage {
                activity: entries.into_iter().map(|entry| entry.1).collect(),
                next_cursor,
            })
        })
        .await
        .map_err(|cause| FileActivityError {
            cause: Box::new(cause),
        })
    }

    pub async fn discover_workers(&self, query: Query) -> Result<Page, DiscoverWorkersError> {
        self.read(move |conn| {
            let limit = query.limit.clamp(1, 200) as usize;
            let mut statement = conn.prepare(include_str!("sql/coordination/discovery.sql"))?;
            let rows = statement.query_map(
                params![
                    query.project.map(|p| p.to_string()),
                    query.parent.map(|p| p.to_string()),
                    query.include_agents,
                    query.active,
                    crate::read::to_body(&query.live_threads)?,
                    query.archived,
                    query.title,
                    query.target,
                    query.cursor.as_ref().map(|c| c.activity_seq),
                    query.cursor.as_ref().map(|c| c.id.as_str()),
                    query.worktree,
                    query.unread,
                    limit + 1
                ],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, bool>(3)?,
                        row.get::<_, Option<String>>(4)?,
                    ))
                },
            )?;
            let mut workers = rows
                .map(|row| {
                    let (thread, project, run, awaiting_user, work) = row?;
                    Ok(Worker {
                        thread: crate::read::from_body(&thread)?,
                        project: crate::read::from_body(&project)?,
                        latest_run: run.map(|s| crate::read::from_body(&s)).transpose()?,
                        awaiting_user,
                        work: work.map(|s| crate::read::from_body(&s)).transpose()?,
                    })
                })
                .collect::<Result<Vec<_>, StorageError>>()?;
            let next_cursor = if workers.len() > limit {
                workers.truncate(limit);
                workers
                    .last()
                    .map(|w| {
                        crate::read::to_body(&ThreadCursor {
                            activity_seq: w.thread.activity_seq,
                            id: w.thread.id.clone(),
                        })
                    })
                    .transpose()?
            } else {
                None
            };
            Ok(Page {
                workers,
                next_cursor,
            })
        })
        .await
        .map_err(|cause| DiscoverWorkersError {
            cause: Box::new(cause),
        })
    }
}
