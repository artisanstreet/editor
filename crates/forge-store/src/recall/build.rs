use crate::content::ContentError;
use crate::error::AtStep as _;
use crate::error::StorageError;
use crate::writer::{Attempt, Durability, Job, Writer};
use artisan_api::Item;
use rusqlite::{Connection, OptionalExtension};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot, watch};

pub(crate) const INDEX_VERSION: i64 = 2;
const BATCH_ITEMS: i64 = 256;
const STEP: Duration = Duration::from_millis(100);
const REPORT_EVERY: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RetryPace {
    pub first: Duration,
    pub last: Duration,
}

impl RetryPace {
    const SYSTEM: Self = Self {
        first: Duration::from_secs(1),
        last: Duration::from_secs(300),
    };
}

#[derive(Clone)]
pub(crate) struct StepClock(Arc<dyn Fn() -> Instant + Send + Sync>);

impl StepClock {
    fn system() -> Self {
        Self::new(Instant::now)
    }

    pub(crate) fn new(now: impl Fn() -> Instant + Send + Sync + 'static) -> Self {
        Self(Arc::new(now))
    }

    fn now(&self) -> Instant {
        (self.0)()
    }
}

impl std::fmt::Debug for StepClock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StepClock").finish_non_exhaustive()
    }
}

#[derive(Clone, Debug)]
pub struct Pacing {
    pub(crate) retry: RetryPace,
    pub(crate) step: Duration,
    pub(crate) clock: StepClock,
}

impl Pacing {
    #[must_use]
    pub fn system() -> Self {
        Self {
            retry: RetryPace::SYSTEM,
            step: STEP,
            clock: StepClock::system(),
        }
    }

    #[must_use]
    pub fn retrying(self, retry: RetryPace) -> Self {
        Self { retry, ..self }
    }

    pub(crate) fn budget(&self, commit_reserve: Duration) -> Budget {
        let started = self.clock.now();
        Budget {
            started,
            step: self.step,
            clock: self.clock.clone(),
            commit_reserve,
            row_started: started,
            slowest_row: Duration::ZERO,
            work_ended: started,
        }
    }
}

pub(crate) struct Budget {
    started: Instant,
    step: Duration,
    clock: StepClock,
    commit_reserve: Duration,
    row_started: Instant,
    slowest_row: Duration,
    work_ended: Instant,
}

impl Budget {
    fn start_rows(&mut self) {
        self.row_started = self.clock.now();
    }

    fn another_row_fits(&mut self) -> bool {
        let now = self.clock.now();
        self.slowest_row = self.slowest_row.max(now.duration_since(self.row_started));
        self.row_started = now;
        now.duration_since(self.started) + self.slowest_row + self.commit_reserve < self.step
    }

    fn end_work(&mut self) {
        self.work_ended = self.clock.now();
    }

    fn commit_took(&self) -> Duration {
        self.clock.now().duration_since(self.work_ended)
    }
}

#[derive(Clone, Copy)]
enum WriteLock {
    Free,
    HeldElsewhereSince(Instant),
}

enum StepEnd {
    RowsRunOut,
    OutOfTime,
}

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
    State(#[source] StorageError),
    #[error("start rebuilding the recall index at version {version}")]
    Reset {
        version: i64,
        #[source]
        source: StorageError,
    },
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum BuildIndexError {
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
    #[error("begin or commit the batch's write transaction")]
    Transaction(#[from] StorageError),
}

pub(crate) fn index_state(conn: &Connection) -> Result<IndexState, StorageError> {
    let present: bool = conn
        .query_row(
            include_str!("../sql/recall/has_index_version.sql"),
            [],
            |row| row.get(0),
        )
        .at("recall/has_index_version.sql")?;
    if !present {
        return Ok(IndexState::Stale { built: None });
    }
    let built: Option<i64> = conn
        .query_row(include_str!("../sql/recall/index_version.sql"), [], |row| {
            row.get(0)
        })
        .optional()
        .at("recall/index_version.sql")?;
    if built != Some(INDEX_VERSION) {
        return Ok(IndexState::Stale { built });
    }
    let after: Option<i64> = conn
        .query_row(include_str!("../sql/recall/index_state.sql"), [], |row| {
            row.get(0)
        })
        .at("recall/index_state.sql")?;
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
            let tx = conn
                .transaction()
                .at("begin a transaction")
                .map_err(reset)?;
            tx.execute_batch(include_str!("../sql/recall/rebuild.sql"))
                .at("recall/rebuild.sql")
                .map_err(reset)?;
            tx.execute(
                include_str!("../sql/recall/set_index_version.sql"),
                [INDEX_VERSION],
            )
            .at("recall/set_index_version.sql")
            .map_err(reset)?;
            tx.commit().at("commit").map_err(reset)?;
            Ok(IndexState::Building { after: 0 })
        }
        current @ (IndexState::Building { .. } | IndexState::Built) => Ok(current),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Batch {
    Indexed { items: u64, skipped: u64 },
    Finished { items: u64, skipped: u64 },
    AlreadyBuilt,
    Stale,
}

pub(crate) fn build_batch(
    conn: &Connection,
    budget: &mut Budget,
) -> Result<Batch, BuildIndexError> {
    let after = match index_state(conn).map_err(BuildIndexError::State)? {
        IndexState::Building { after } => after,
        IndexState::Built => return Ok(Batch::AlreadyBuilt),
        IndexState::Stale { .. } => return Ok(Batch::Stale),
    };
    let read = |source| BuildIndexError::Items { after, source };
    let mut statement = conn
        .prepare_cached(include_str!("../sql/recall/items.sql"))
        .at("recall/items.sql")
        .map_err(read)?;
    let mut rows = statement
        .query([after, BATCH_ITEMS])
        .at("recall/items.sql")
        .map_err(read)?;
    let (mut items, mut skipped, mut read_rows) = (0_u64, 0_u64, 0_i64);
    let mut through = after;
    budget.start_rows();
    let end = loop {
        if read_rows > 0 && !budget.another_row_fits() {
            break StepEnd::OutOfTime;
        }
        let Some(row) = rows.next().at("recall/items.sql").map_err(read)? else {
            break StepEnd::RowsRunOut;
        };
        let (rowid, body): (i64, String) = (
            row.get(0).at("recall/items.sql").map_err(read)?,
            row.get(1).at("recall/items.sql").map_err(read)?,
        );
        read_rows += 1;
        through = rowid;
        let item = match serde_json::from_str::<Item>(&body) {
            Ok(item) => item,
            Err(error) => {
                tracing::warn!(row = rowid, %error, "left an item whose body doesn't read out of recall");
                skipped += 1;
                continue;
            }
        };
        match super::index(conn, &item) {
            Ok(()) => items += 1,
            Err(ContentError::Storage(StorageError::Decode { kind, source })) => {
                tracing::warn!(
                    item = %item.id,
                    kind,
                    error = %diagnostics::chain(&source),
                    "left an item whose stored data doesn't read out of recall"
                );
                skipped += 1;
            }
            Err(ContentError::Storage(source)) => {
                return Err(BuildIndexError::Index {
                    item: item.id,
                    source,
                });
            }
            Err(
                damage @ (ContentError::TextTooLarge { .. }
                | ContentError::FieldTooLong { .. }
                | ContentError::TooManyEntries { .. }
                | ContentError::MetadataTooLarge { .. }
                | ContentError::DataTooLarge { .. }
                | ContentError::InvalidIdentity { .. }
                | ContentError::UnknownReference { .. }
                | ContentError::Missing { .. }
                | ContentError::RangeLimit { .. }
                | ContentError::OffsetPastEnd { .. }
                | ContentError::Damaged { .. }
                | ContentError::RecordTooLarge { .. }
                | ContentError::RecordIdentity
                | ContentError::CollectionTooLarge { .. }
                | ContentError::SelfDependency { .. }
                | ContentError::MissingDependency { .. }
                | ContentError::CollectionLimit { .. }
                | ContentError::RecordOffsetPastEnd { .. }
                | ContentError::AnswerDisplayTooLarge
                | ContentError::SavedAnswerCount { .. }
                | ContentError::Refused(_)),
            ) => {
                tracing::warn!(
                    item = %item.id,
                    error = %diagnostics::chain(&damage),
                    "left a damaged item out of recall"
                );
                skipped += 1;
            }
        }
    };
    budget.end_work();
    let advance = |source| BuildIndexError::Advance { through, source };
    drop(rows);
    if matches!(end, StepEnd::RowsRunOut) && read_rows < BATCH_ITEMS {
        conn.execute(include_str!("../sql/recall/finish_index.sql"), [])
            .at("recall/finish_index.sql")
            .map_err(advance)?;
        return Ok(Batch::Finished { items, skipped });
    }
    conn.execute(include_str!("../sql/recall/advance_index.sql"), [through])
        .at("recall/advance_index.sql")
        .map_err(advance)?;
    Ok(Batch::Indexed { items, skipped })
}

pub(crate) struct Builder {
    stop: std::sync::mpsc::Sender<()>,
    thread: std::thread::JoinHandle<()>,
}

impl Builder {
    pub(crate) fn spawn(
        jobs: mpsc::Sender<Job>,
        status: watch::Sender<RecallIndex>,
        pacing: Pacing,
    ) -> std::io::Result<Self> {
        let (stop, stopping) = std::sync::mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("forge-store-recall".into())
            .spawn(move || {
                let reached = build(&jobs, &stopping, &pacing);
                status.send_replace(reached);
            })?;
        Ok(Self { stop, thread })
    }

    pub(crate) fn stop(self) -> std::thread::Result<()> {
        drop(self.stop);
        self.thread.join()
    }

    pub(crate) fn stop_without_waiting(self) {
        drop(self.stop);
        drop(self.thread);
    }
}

enum Reply {
    Built {
        built: Result<Batch, BuildIndexError>,
        commit: Duration,
    },
    LockHeld(StorageError),
    WriterStopped,
}

fn batch(jobs: &mpsc::Sender<Job>, pacing: &Pacing, commit_reserve: Duration) -> Reply {
    let (reply, outcome) = oneshot::channel();
    let pacing = pacing.clone();
    let job: Job = Box::new(move |writer: &mut Writer| {
        let mut budget = pacing.budget(commit_reserve);
        let attempt = writer.transact_waiting_at_most(pacing.step, Durability::Background, |tx| {
            build_batch(tx, &mut budget)
        });
        let answer = match attempt {
            Ok(batch) => Reply::Built {
                built: Ok(batch),
                commit: budget.commit_took(),
            },
            Err(Attempt::LockHeld(held)) => Reply::LockHeld(held),
            Err(Attempt::Failed(failure)) => Reply::Built {
                built: Err(failure),
                commit: Duration::ZERO,
            },
        };
        if reply.send(answer).is_err() {
            tracing::debug!("the recall index builder stopped before its batch finished");
        }
    });
    if jobs.blocking_send(job).is_err() {
        return Reply::WriterStopped;
    }
    match outcome.blocking_recv() {
        Ok(answer) => answer,
        Err(_writer_stopped) => Reply::WriterStopped,
    }
}

fn stop_requested(stop: &std::sync::mpsc::Receiver<()>) -> bool {
    match stop.try_recv() {
        Err(std::sync::mpsc::TryRecvError::Empty) => false,
        Ok(()) | Err(std::sync::mpsc::TryRecvError::Disconnected) => true,
    }
}

fn stopped_while_waiting(stop: &std::sync::mpsc::Receiver<()>, wait: Duration) -> bool {
    match stop.recv_timeout(wait) {
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => false,
        Ok(()) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => true,
    }
}

impl WriteLock {
    fn held(self, items: u64, error: &StorageError) -> Self {
        match self {
            Self::Free => {
                tracing::info!(
                    items,
                    error = %diagnostics::chain(error),
                    "another connection holds the write lock; the recall index waits for it"
                );
                Self::HeldElsewhereSince(Instant::now())
            }
            held @ Self::HeldElsewhereSince(_) => held,
        }
    }

    fn free(self, items: u64) -> Self {
        match self {
            Self::Free => Self::Free,
            Self::HeldElsewhereSince(since) => {
                tracing::info!(
                    items,
                    waited_ms = since.elapsed().as_millis(),
                    "the write lock is free again; the recall index carries on"
                );
                Self::Free
            }
        }
    }
}

fn build(
    jobs: &mpsc::Sender<Job>,
    stop: &std::sync::mpsc::Receiver<()>,
    pacing: &Pacing,
) -> RecallIndex {
    let pace = pacing.retry;
    let started = Instant::now();
    let mut reported = started;
    let mut retry = pace.first;
    let mut lock = WriteLock::Free;
    let mut slowest_commit = Duration::ZERO;
    let (mut items, mut skipped) = (0_u64, 0_u64);
    while !stop_requested(stop) {
        let (built, commit) = match batch(jobs, pacing, slowest_commit) {
            Reply::WriterStopped => return RecallIndex::Incomplete,
            Reply::LockHeld(error) => {
                lock = lock.held(items, &error);
                if stopped_while_waiting(stop, pace.first) {
                    return RecallIndex::Incomplete;
                }
                continue;
            }
            Reply::Built { built, commit } => (built, commit),
        };
        lock = lock.free(items);
        match built {
            Ok(Batch::Stale) => return RecallIndex::Incomplete,
            Ok(Batch::AlreadyBuilt) => {
                tracing::info!(items, skipped, "another Forge finished the recall index");
                return RecallIndex::Built;
            }
            Ok(Batch::Indexed {
                items: indexed,
                skipped: left_out,
            }) => {
                retry = pace.first;
                slowest_commit = slowest_commit.max(commit);
                items += indexed;
                skipped += left_out;
                if reported.elapsed() >= REPORT_EVERY {
                    reported = Instant::now();
                    tracing::info!(items, skipped, "building the recall index");
                }
            }
            Ok(Batch::Finished {
                items: indexed,
                skipped: left_out,
            }) => {
                tracing::info!(
                    items = items + indexed,
                    skipped = skipped + left_out,
                    elapsed_s = started.elapsed().as_secs(),
                    "the recall index is built"
                );
                return RecallIndex::Built;
            }
            Err(error) => {
                tracing::warn!(
                    items,
                    retry_s = retry.as_secs(),
                    error = %diagnostics::chain(&error),
                    "a recall index batch failed; search covers what was indexed and the build carries on from there"
                );
                if stopped_while_waiting(stop, retry) {
                    return RecallIndex::Incomplete;
                }
                retry = (retry * 2).min(pace.last);
            }
        }
    }
    RecallIndex::Incomplete
}
