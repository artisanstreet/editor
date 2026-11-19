use crate::content::ContentError;
use crate::error::StorageError;
use crate::writer::{Durability, Job, Writer};
use artisan_api::Item;
use rusqlite::{Connection, OptionalExtension};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot, watch};

pub(crate) const INDEX_VERSION: i64 = 2;
const BATCH_ITEMS: i64 = 256;
const REPORT_EVERY: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecallIndex {
    Building,
    Built,
    Incomplete,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum IndexState {
    Stale { built: Option<i64> },
    Building { after: i64 },
    Built,
}

#[derive(Debug, thiserror::Error)]
pub enum PrepareIndexError {
    #[error("read which recall index the database holds")]
    State(#[source] rusqlite::Error),
    #[error("start rebuilding the recall index at version {version}")]
    Reset {
        version: i64,
        #[source]
        source: rusqlite::Error,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum BuildIndexError {
    #[error("read how far the recall index is built")]
    State(#[source] StorageError),
    #[error("read the items after row {after} to index")]
    Items {
        after: i64,
        #[source]
        source: StorageError,
    },
    #[error("index item {item} for recall")]
    Index {
        item: artisan_api::ItemId,
        #[source]
        source: StorageError,
    },
    #[error("record that the recall index is built through row {through}")]
    Advance {
        through: i64,
        #[source]
        source: StorageError,
    },
}

impl From<StorageError> for BuildIndexError {
    fn from(source: StorageError) -> Self {
        Self::State(source)
    }
}

pub(crate) fn index_state(conn: &Connection) -> rusqlite::Result<IndexState> {
    let present: bool = conn.query_row(
        include_str!("../sql/recall/has_index_version.sql"),
        [],
        |row| row.get(0),
    )?;
    if !present {
        return Ok(IndexState::Stale { built: None });
    }
    let built: Option<i64> = conn
        .query_row(include_str!("../sql/recall/index_version.sql"), [], |row| {
            row.get(0)
        })
        .optional()?;
    if built != Some(INDEX_VERSION) {
        return Ok(IndexState::Stale { built });
    }
    let after: Option<i64> =
        conn.query_row(include_str!("../sql/recall/index_state.sql"), [], |row| {
            row.get(0)
        })?;
    Ok(after.map_or(IndexState::Built, |after| IndexState::Building { after }))
}

pub(crate) fn prepare_index(conn: &mut Connection) -> Result<IndexState, PrepareIndexError> {
    match index_state(conn).map_err(PrepareIndexError::State)? {
        IndexState::Stale { built } => {
            tracing::info!(
                built,
                version = INDEX_VERSION,
                "rebuilding the recall index while Forge serves"
            );
            let reset = |source| PrepareIndexError::Reset {
                version: INDEX_VERSION,
                source,
            };
            let tx = conn.transaction().map_err(reset)?;
            tx.execute_batch(include_str!("../sql/recall/rebuild.sql"))
                .map_err(reset)?;
            tx.execute(
                include_str!("../sql/recall/set_index_version.sql"),
                [INDEX_VERSION],
            )
            .map_err(reset)?;
            tx.commit().map_err(reset)?;
            Ok(IndexState::Building { after: 0 })
        }
        current @ (IndexState::Building { .. } | IndexState::Built) => Ok(current),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Batch {
    Indexed { items: u64, skipped: u64 },
    Finished { items: u64, skipped: u64 },
    Abandoned,
}

pub(crate) fn build_batch(conn: &Connection) -> Result<Batch, BuildIndexError> {
    let after = match index_state(conn).map_err(StorageError::from)? {
        IndexState::Building { after } => after,
        IndexState::Built | IndexState::Stale { .. } => return Ok(Batch::Abandoned),
    };
    let read = |source: rusqlite::Error| BuildIndexError::Items {
        after,
        source: source.into(),
    };
    let rows: Vec<(i64, String)> = conn
        .prepare_cached(include_str!("../sql/recall/items.sql"))
        .map_err(read)?
        .query_map([after, BATCH_ITEMS], |row| Ok((row.get(0)?, row.get(1)?)))
        .map_err(read)?
        .collect::<rusqlite::Result<_>>()
        .map_err(read)?;
    let (mut items, mut skipped) = (0_u64, 0_u64);
    let mut through = after;
    for (rowid, body) in &rows {
        through = *rowid;
        let item = match serde_json::from_str::<Item>(body) {
            Ok(item) => item,
            Err(error) => {
                tracing::warn!(row = rowid, %error, "left an item whose body doesn't read out of recall");
                skipped += 1;
                continue;
            }
        };
        match super::index(conn, &item) {
            Ok(()) => items += 1,
            Err(ContentError::Storage(source)) => {
                return Err(BuildIndexError::Index {
                    item: item.id,
                    source,
                });
            }
            Err(damage) => {
                tracing::warn!(
                    item = %item.id,
                    error = %diagnostics::chain(&damage),
                    "left a damaged item out of recall"
                );
                skipped += 1;
            }
        }
    }
    let advance = |source: rusqlite::Error| BuildIndexError::Advance {
        through,
        source: source.into(),
    };
    if i64::try_from(rows.len()).is_ok_and(|count| count < BATCH_ITEMS) {
        conn.execute(include_str!("../sql/recall/finish_index.sql"), [])
            .map_err(advance)?;
        return Ok(Batch::Finished { items, skipped });
    }
    conn.execute(include_str!("../sql/recall/advance_index.sql"), [through])
        .map_err(advance)?;
    Ok(Batch::Indexed { items, skipped })
}

pub(crate) struct Builder {
    stop: Arc<AtomicBool>,
    thread: std::thread::JoinHandle<()>,
}

impl Builder {
    pub(crate) fn spawn(
        jobs: mpsc::Sender<Job>,
        status: watch::Sender<RecallIndex>,
    ) -> std::io::Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = Arc::clone(&stop);
        let thread = std::thread::Builder::new()
            .name("forge-store-recall".into())
            .spawn(move || {
                let reached = build(&jobs, &stopping);
                status.send_replace(reached);
            })?;
        Ok(Self { stop, thread })
    }

    pub(crate) fn stop(self) -> std::thread::Result<()> {
        self.stop.store(true, Ordering::Release);
        self.thread.join()
    }
}

enum Reply {
    Built(Result<Batch, BuildIndexError>),
    WriterStopped,
}

fn batch(jobs: &mpsc::Sender<Job>) -> Reply {
    let (reply, outcome) = oneshot::channel();
    let job: Job = Box::new(move |writer: &mut Writer| {
        let built = writer.transact(Durability::Background, |tx| build_batch(tx));
        if reply.send(built).is_err() {
            tracing::debug!("the recall index builder stopped before its batch finished");
        }
    });
    if jobs.blocking_send(job).is_err() {
        return Reply::WriterStopped;
    }
    match outcome.blocking_recv() {
        Ok(built) => Reply::Built(built),
        Err(_writer_stopped) => Reply::WriterStopped,
    }
}

fn build(jobs: &mpsc::Sender<Job>, stop: &AtomicBool) -> RecallIndex {
    let started = Instant::now();
    let mut reported = started;
    let (mut items, mut skipped) = (0_u64, 0_u64);
    while !stop.load(Ordering::Acquire) {
        match batch(jobs) {
            Reply::WriterStopped | Reply::Built(Ok(Batch::Abandoned)) => {
                return RecallIndex::Incomplete;
            }
            Reply::Built(Ok(Batch::Indexed {
                items: indexed,
                skipped: left_out,
            })) => {
                items += indexed;
                skipped += left_out;
                if reported.elapsed() >= REPORT_EVERY {
                    reported = Instant::now();
                    tracing::info!(items, skipped, "building the recall index");
                }
            }
            Reply::Built(Ok(Batch::Finished {
                items: indexed,
                skipped: left_out,
            })) => {
                tracing::info!(
                    items = items + indexed,
                    skipped = skipped + left_out,
                    elapsed_s = started.elapsed().as_secs(),
                    "the recall index is built"
                );
                return RecallIndex::Built;
            }
            Reply::Built(Err(error)) => {
                tracing::warn!(
                    items,
                    error = %diagnostics::chain(&error),
                    "the recall index build stopped; search covers what was indexed and the build resumes when Forge next starts"
                );
                return RecallIndex::Incomplete;
            }
        }
    }
    RecallIndex::Incomplete
}
