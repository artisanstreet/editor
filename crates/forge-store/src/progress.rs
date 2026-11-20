use rusqlite::Connection;
use std::sync::Arc;
use std::time::{Duration, Instant};

const OPERATIONS_PER_CHECK: i32 = 10_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Step {
    Backup { version: usize },
    SetAside { found: usize },
    ListImages,
    Migration { next: usize },
    RecallIndex,
}

impl Step {
    const fn upgrades(self) -> bool {
        match self {
            Self::Backup { .. }
            | Self::SetAside { .. }
            | Self::ListImages
            | Self::Migration { .. } => true,
            Self::RecallIndex => false,
        }
    }
}

impl std::fmt::Display for Step {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Backup { version } => write!(f, "backing up schema {version}"),
            Self::SetAside { found } => write!(f, "setting schema {found} aside"),
            Self::ListImages => f.write_str("listing the images a kept database uses"),
            Self::Migration { next } => write!(f, "upgrading to schema {next}"),
            Self::RecallIndex => f.write_str("preparing the recall index"),
        }
    }
}

#[derive(Clone)]
pub struct Progress {
    every: Duration,
    heartbeat: Arc<dyn Fn() + Send + Sync>,
}

impl Progress {
    #[must_use]
    pub fn every(every: Duration) -> Self {
        Self {
            every,
            heartbeat: Arc::new(|| {}),
        }
    }

    #[must_use]
    pub fn with_heartbeat(self, heartbeat: impl Fn() + Send + Sync + 'static) -> Self {
        Self {
            heartbeat: Arc::new(heartbeat),
            ..self
        }
    }
}

impl std::fmt::Debug for Progress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Progress")
            .field("every", &self.every)
            .finish_non_exhaustive()
    }
}

pub(crate) fn watch(conn: &mut Connection, step: Step, pace: &Progress) -> rusqlite::Result<()> {
    if step.upgrades() {
        (pace.heartbeat)();
    }
    let started = Instant::now();
    let mut reported = started;
    let every = pace.every;
    let heartbeat = Arc::clone(&pace.heartbeat);
    conn.progress_handler(
        OPERATIONS_PER_CHECK,
        Some(move || {
            if reported.elapsed() >= every {
                reported = Instant::now();
                tracing::info!(
                    elapsed_s = started.elapsed().as_secs(),
                    "still {step} before Forge serves"
                );
                heartbeat();
            }
            false
        }),
    )
}

pub(crate) fn unwatch(conn: &mut Connection) -> rusqlite::Result<()> {
    conn.progress_handler(0, None::<fn() -> bool>)
}
