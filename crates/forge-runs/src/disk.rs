use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

const GIB: u64 = 1 << 30;
pub(crate) const WARN: u64 = 10 * GIB;
pub(crate) const STOP: u64 = 3 * GIB;
const REARM: u64 = 12 * GIB;
const MARGIN: u64 = GIB;
const EXHAUSTED: u64 = GIB / 4;

pub(crate) type FreeSpace = Arc<dyn Fn(&Path) -> std::io::Result<u64> + Send + Sync>;

#[derive(Clone)]
pub(crate) struct Probe {
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

pub(crate) enum Alarm {
    Low { path: PathBuf, free: u64 },
    Full { path: PathBuf, free: u64 },
}

pub(crate) struct Watch {
    probe: Probe,
    paths: Vec<PathBuf>,
    interval: tokio::time::Interval,
    stop_below: Option<u64>,
    warned: bool,
}

impl Watch {
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

    pub(crate) async fn due(&mut self) {
        self.interval.tick().await;
    }

    pub(crate) async fn check(&mut self) -> Option<Alarm> {
        let (probe, paths) = (Arc::clone(&self.probe.free_space), self.paths.clone());
        let measured = tokio::task::spawn_blocking(move || {
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
        .await;
        let (path, free) = match measured {
            Ok(lowest) => lowest?,
            Err(error) => {
                tracing::warn!(paths = ?self.paths, "read free disk space task: {error}");
                return None;
            }
        };
        let stop_below = match self.stop_below {
            None => STOP.min(free.saturating_sub(MARGIN)),
            Some(_) if free >= STOP => STOP,
            Some(line) => line,
        }
        .max(EXHAUSTED);
        self.stop_below = Some(stop_below);
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

    pub(crate) const fn warned(&mut self) {
        self.warned = true;
    }
}

pub(crate) fn steer(path: &Path, free: u64) -> String {
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

pub(crate) fn notice(path: &Path, free: u64) -> String {
    format!(
        "Disk low: {} free on {}. Asked the agent to free space; runs stop below {}.",
        gib(free),
        path.display(),
        gib(STOP),
    )
}

pub(crate) fn stopped(path: &Path, free: u64) -> String {
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
