use rusqlite::Connection;
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

pub(crate) fn watch(conn: &mut Connection, step: Step, every: Duration) -> rusqlite::Result<()> {
    let started = Instant::now();
    let mut reported = started;
    conn.progress_handler(
        OPERATIONS_PER_CHECK,
        Some(move || {
            if reported.elapsed() >= every {
                reported = Instant::now();
                tracing::info!(
                    elapsed_s = started.elapsed().as_secs(),
                    "still {step} before Forge serves"
                );
            }
            false
        }),
    )
}

pub(crate) fn unwatch(conn: &mut Connection) -> rusqlite::Result<()> {
    conn.progress_handler(0, None::<fn() -> bool>)
}
