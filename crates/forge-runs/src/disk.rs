//! The disk guard: a run watches the free space where it works.
//!
//! A full disk slows every build on it long before writes fail. Below
//! [`WARN`] the agent is told, once, to free space itself; below [`STOP`]
//! Forge stops the run. A run started below the stop line (the user sent it,
//! maybe to clean up) stops only once it loses another [`MARGIN`].

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

const GIB: u64 = 1 << 30;
pub const WARN: u64 = 10 * GIB;
pub const STOP: u64 = 3 * GIB;
/// Above this the warning is armed again.
const REARM: u64 = 12 * GIB;
const MARGIN: u64 = GIB;

/// Free bytes on the volume holding a path.
pub type FreeSpace = Arc<dyn Fn(&Path) -> std::io::Result<u64> + Send + Sync>;

#[derive(Clone)]
pub struct Probe {
    pub(crate) free_space: FreeSpace,
    pub(crate) every: Duration,
}

impl Probe {
    pub(crate) fn system() -> Self {
        Self {
            free_space: Arc::new(|path| fs2::available_space(path)),
            every: Duration::from_secs(15),
        }
    }
}

/// What a check found that the run acts on.
pub enum Alarm {
    Low { path: PathBuf, free: u64 },
    Full { path: PathBuf, free: u64 },
}

/// One run's watch over the volumes it writes to.
pub struct Watch {
    probe: Probe,
    paths: Vec<PathBuf>,
    interval: tokio::time::Interval,
    stop_below: Option<u64>,
    warned: bool,
}

impl Watch {
    /// Watches the run's folder and Forge's own data.
    pub(crate) fn new(probe: Probe, paths: Vec<PathBuf>) -> Self {
        let first = tokio::time::Instant::now() + probe.every;
        let mut interval = tokio::time::interval_at(first, probe.every);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        Self {
            probe,
            paths,
            interval,
            stop_below: None,
            warned: false,
        }
    }

    /// Waits for the next check. Cancel-safe: the schedule survives a lost race.
    pub(crate) async fn due(&mut self) {
        self.interval.tick().await;
    }

    /// Checks now. A volume that can't be read is skipped, never an alarm.
    pub(crate) async fn check(&mut self) -> Option<Alarm> {
        let (probe, paths) = (Arc::clone(&self.probe.free_space), self.paths.clone());
        let lowest = tokio::task::spawn_blocking(move || {
            paths
                .into_iter()
                .filter_map(|path| match probe(&path) {
                    Ok(free) => Some((path, free)),
                    Err(error) => {
                        tracing::debug!(path = %path.display(), "read free disk space: {error}");
                        None
                    }
                })
                .min_by_key(|(_, free)| *free)
        })
        .await
        .ok()
        .flatten()?;
        let (path, free) = lowest;
        let stop_below = *self
            .stop_below
            .get_or_insert_with(|| STOP.min(free.saturating_sub(MARGIN)));
        if free < stop_below {
            return Some(Alarm::Full { path, free });
        }
        if free >= REARM {
            self.warned = false;
        }
        if free < WARN && !self.warned {
            return Some(Alarm::Low { path, free });
        }
        None
    }

    /// The agent was told; quiet until space recovers.
    pub(crate) const fn warned(&mut self) {
        self.warned = true;
    }
}

/// What the agent reads when the disk runs low.
pub fn steer(path: &Path, free: u64) -> String {
    format!(
        "Disk low: {} has {} free. Forge stops runs on this disk below {}. Free space \
         before you continue. Delete only build outputs, caches and dependency folders \
         that can be recreated, and skip the ones the project you're working on needs \
         right now. A worktree goes only if it's clean and its branch is merged. Never \
         delete source, uncommitted or unmerged work, or data directories. If you're \
         unsure about something, leave it and say so.",
        path.display(),
        gib(free),
        gib(STOP),
    )
}

/// The line the user sees in the thread when the agent was told.
pub fn notice(path: &Path, free: u64) -> String {
    format!(
        "Disk low: {} free on {}. Asked the agent to free space; runs stop below {}.",
        gib(free),
        path.display(),
        gib(STOP),
    )
}

/// Why the run stopped, on its error card.
pub fn stopped(path: &Path, free: u64) -> String {
    format!(
        "Stopped: only {} free on {}. Free space, then send a message to carry on.",
        gib(free),
        path.display(),
    )
}

fn gib(bytes: u64) -> String {
    if bytes.is_multiple_of(GIB) {
        return format!("{} GiB", bytes / GIB);
    }
    #[expect(clippy::cast_precision_loss, reason = "a size shown to one decimal")]
    let gib = bytes as f64 / GIB as f64;
    format!("{gib:.1} GiB")
}
