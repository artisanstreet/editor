//! Reports when the Forge's runtime, or one of its workers, stops making
//! progress.
//!
//! A task that blocks (a process spawn waiting for exec, a file read on a
//! saturated disk) holds its worker for as long as the call takes. The other
//! workers keep the Forge running, but the blocked task, and anything queued
//! behind it, waits; when every worker is blocked, every timer stalls at once:
//! dispatch lease heartbeats, request deadlines and conversation delivery.
//!
//! A plain OS thread watches two things. A ticker task stamps progress, so a
//! stale stamp means the whole runtime is stuck. And each worker's scheduler
//! metrics show a worker that is neither parked nor finishing tasks, so a
//! single stuck worker is reported even while the rest carry on. Each report
//! names what the worker threads wait on in the kernel, which tells a process
//! spawn (`kernel_clone`) from a disk read or an fsync. When the watcher thread
//! itself oversleeps, the whole process was paused (host or VM suspend), which
//! is reported separately so the causes are never confused.

#![forbid(unsafe_code)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use tokio::runtime::RuntimeMetrics;

/// How often the ticker stamps progress.
const TICK: Duration = Duration::from_millis(250);
/// How often the watcher thread inspects the stamp and the workers.
const CHECK: Duration = Duration::from_millis(500);
/// Lag past which the runtime or a worker counts as blocked.
const STALL: Duration = Duration::from_secs(2);
/// Name of the runtime's worker threads, as the kernel reports it.
pub(crate) const WORKER_THREAD_NAME: &str = "forge-worker";

/// Stops the watcher thread when dropped.
pub(crate) struct EventLoopWatchdog {
    running: Arc<AtomicBool>,
}

impl Drop for EventLoopWatchdog {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
    }
}

/// Starts the ticker on `runtime` and the watcher thread. Returns `None`
/// when the watcher thread cannot be spawned; the Forge runs unwatched.
pub(crate) fn start(runtime: &tokio::runtime::Runtime) -> Option<EventLoopWatchdog> {
    start_reporting(runtime, |line| eprintln!("{line}"))
}

fn start_reporting(
    runtime: &tokio::runtime::Runtime,
    report: impl FnMut(String) + Send + 'static,
) -> Option<EventLoopWatchdog> {
    let origin = Instant::now();
    let stamp = Arc::new(AtomicU64::new(0));
    let running = Arc::new(AtomicBool::new(true));

    let ticker_stamp = Arc::clone(&stamp);
    let ticker_running = Arc::clone(&running);
    runtime.spawn(async move {
        let mut ticks = tokio::time::interval(TICK);
        ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        while ticker_running.load(Ordering::Relaxed) {
            ticks.tick().await;
            ticker_stamp.store(elapsed_millis(origin), Ordering::Relaxed);
        }
    });

    let metrics = runtime.metrics();
    let watcher_running = Arc::clone(&running);
    let spawned = std::thread::Builder::new()
        .name("forge-event-loop-watchdog".to_owned())
        .spawn(move || watch(origin, &stamp, &metrics, &watcher_running, report));
    match spawned {
        Ok(_) => Some(EventLoopWatchdog { running }),
        Err(error) => {
            eprintln!(
                "Forge event-loop watchdog not started: spawning its thread failed: {}",
                artisan_domain::ErrorChain(&error)
            );
            None
        }
    }
}

/// What the watcher last saw of one worker.
#[derive(Clone, Copy)]
struct WorkerProgress {
    /// Even while the worker runs tasks, odd while it is parked.
    park_unpark: u64,
    /// Grows each time a running worker hands in its scheduler statistics.
    busy: Duration,
    /// When either value last changed.
    since: Instant,
    /// Whether the stall was already reported.
    reported: bool,
}

fn watch(
    origin: Instant,
    stamp: &AtomicU64,
    metrics: &RuntimeMetrics,
    running: &AtomicBool,
    mut report: impl FnMut(String),
) {
    let mut blocked_since: Option<u64> = None;
    let mut workers: Vec<Option<WorkerProgress>> = vec![None; metrics.num_workers()];
    while running.load(Ordering::Relaxed) {
        let before = Instant::now();
        std::thread::sleep(CHECK);
        let slept = before.elapsed();
        if slept > STALL {
            // The watcher itself was frozen, so the whole process was.
            report(format!(
                "Forge process was paused for {} ms (host or VM suspend); every timer fired late",
                slept.saturating_sub(CHECK).as_millis()
            ));
            blocked_since = None;
            workers.fill(None);
            continue;
        }
        watch_workers(metrics, &mut workers, &mut report);
        let now = elapsed_millis(origin);
        let last = stamp.load(Ordering::Relaxed);
        let lag = now.saturating_sub(last);
        match blocked_since {
            None if lag > millis(STALL) => {
                #[cfg(feature = "flight-recorder")]
                artisan_tracing::instant!("watchdog", "forge.event_loop_stalled", "lag_ms" => lag);
                #[cfg(feature = "flight-recorder")]
                artisan_tracing::incident!("forge.event_loop_stalled");
                report(format!(
                    "Forge event loop blocked for {lag} ms: lease heartbeats, request deadlines and conversation delivery are stalled; {}",
                    worker_threads_waiting()
                ));
                blocked_since = Some(last);
            }
            Some(since) if lag <= millis(STALL) => {
                #[cfg(feature = "flight-recorder")]
                artisan_tracing::instant!("watchdog", "forge.event_loop_resumed", "duration_ms" => last.saturating_sub(since));
                #[cfg(feature = "flight-recorder")]
                artisan_tracing::incident!("forge.event_loop_resumed");
                report(format!(
                    "Forge event loop resumed after being blocked for {} ms",
                    last.saturating_sub(since)
                ));
                blocked_since = None;
            }
            _ => {}
        }
    }
}

/// Reports each worker that has spent longer than [`STALL`] inside tasks
/// without parking or handing in statistics, which means one poll is
/// blocked, and reports it again once it moves.
fn watch_workers(
    metrics: &RuntimeMetrics,
    workers: &mut [Option<WorkerProgress>],
    report: &mut impl FnMut(String),
) {
    let now = Instant::now();
    for (worker, seen) in workers.iter_mut().enumerate() {
        let park_unpark = metrics.worker_park_unpark_count(worker);
        let busy = metrics.worker_total_busy_duration(worker);
        let moved = seen.is_none_or(|seen| seen.park_unpark != park_unpark || seen.busy != busy);
        if moved {
            if let Some(seen) = seen.filter(|seen| seen.reported) {
                let blocked = millis(now.duration_since(seen.since));
                #[cfg(feature = "flight-recorder")]
                artisan_tracing::instant!("watchdog", "forge.worker_resumed", "worker" => worker, "duration_ms" => blocked);
                report(format!(
                    "Forge worker {worker} resumed after being blocked for about {blocked} ms"
                ));
            }
            *seen = Some(WorkerProgress {
                park_unpark,
                busy,
                since: now,
                reported: false,
            });
            continue;
        }
        let Some(seen) = seen.as_mut() else {
            continue;
        };
        let parked = park_unpark % 2 == 1;
        let blocked = now.duration_since(seen.since);
        if parked || seen.reported || blocked <= STALL {
            continue;
        }
        seen.reported = true;
        let blocked = millis(blocked);
        #[cfg(feature = "flight-recorder")]
        artisan_tracing::instant!("watchdog", "forge.worker_stalled", "worker" => worker, "lag_ms" => blocked);
        #[cfg(feature = "flight-recorder")]
        artisan_tracing::incident!("forge.worker_stalled");
        report(format!(
            "Forge worker {worker} blocked in one task for {blocked} ms: work queued behind it waits; {}",
            worker_threads_waiting()
        ));
    }
}

/// Describes the worker threads that are inside the kernel on something
/// other than waiting for work, e.g. `tid 41 D kernel_clone`.
fn worker_threads_waiting() -> String {
    let waiting = worker_thread_states()
        .into_iter()
        .filter(|(_, state, wchan)| !is_idle_wait(*state, wchan))
        .map(|(tid, state, wchan)| format!("tid {tid} {state} {wchan}"))
        .collect::<Vec<_>>();
    if waiting.is_empty() {
        "no worker thread waits in the kernel, so the blocked task is computing".to_owned()
    } else {
        format!("worker threads in the kernel: {}", waiting.join(", "))
    }
}

/// Parked workers and idle threads wait in epoll or on a futex.
fn is_idle_wait(state: char, wchan: &str) -> bool {
    state == 'S' && (wchan == "do_epoll_wait" || wchan.starts_with("futex"))
}

/// Each worker thread's id, scheduler state and kernel wait channel, read
/// from `/proc`. Empty where `/proc` is unavailable.
fn worker_thread_states() -> Vec<(String, char, String)> {
    let Ok(tasks) = std::fs::read_dir("/proc/self/task") else {
        return Vec::new();
    };
    tasks
        .flatten()
        .filter_map(|task| {
            let path = task.path();
            let name = std::fs::read_to_string(path.join("comm")).ok()?;
            if name.trim_end() != WORKER_THREAD_NAME {
                return None;
            }
            let stat = std::fs::read_to_string(path.join("stat")).ok()?;
            // The state follows the parenthesised name, which may hold spaces.
            let state = stat.rsplit_once(')')?.1.trim_start().chars().next()?;
            let wchan = std::fs::read_to_string(path.join("wchan")).unwrap_or_default();
            let wchan = match wchan.trim() {
                "" | "0" => "running".to_owned(),
                wchan => wchan.to_owned(),
            };
            Some((
                task.file_name().to_string_lossy().into_owned(),
                state,
                wchan,
            ))
        })
        .collect()
}

fn elapsed_millis(origin: Instant) -> u64 {
    millis(origin.elapsed())
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use super::{WORKER_THREAD_NAME, start_reporting};

    fn collected(lines: &Arc<Mutex<Vec<String>>>) -> impl FnMut(String) + Send + 'static {
        let sink = Arc::clone(lines);
        move |line| sink.lock().expect("lines").push(line)
    }

    #[test]
    fn a_blocked_event_loop_is_reported_while_blocked_and_on_resume() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let lines = Arc::new(Mutex::new(Vec::new()));
        let watchdog = start_reporting(&runtime, collected(&lines)).expect("watchdog");
        runtime.block_on(async {
            tokio::time::sleep(Duration::from_millis(600)).await;
            // Blocking work on the only runtime thread.
            std::thread::sleep(Duration::from_secs(3));
            tokio::time::sleep(Duration::from_millis(1500)).await;
        });
        drop(watchdog);
        let lines = lines.lock().expect("lines");
        assert!(
            lines
                .iter()
                .any(|line| line.starts_with("Forge event loop blocked for")),
            "{lines:?}"
        );
        assert!(
            lines
                .iter()
                .any(|line| line.starts_with("Forge event loop resumed after")),
            "{lines:?}"
        );
    }

    #[test]
    fn one_blocked_worker_is_reported_while_the_others_run_on() {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name(WORKER_THREAD_NAME)
            .enable_all()
            .build()
            .expect("runtime");
        let lines = Arc::new(Mutex::new(Vec::new()));
        let watchdog = start_reporting(&runtime, collected(&lines)).expect("watchdog");
        runtime.block_on(async {
            tokio::time::sleep(Duration::from_millis(600)).await;
            let blocked = tokio::spawn(async {
                std::thread::sleep(Duration::from_secs(3));
            });
            // The other worker keeps timers running meanwhile.
            let started = std::time::Instant::now();
            tokio::time::sleep(Duration::from_millis(500)).await;
            assert!(started.elapsed() < Duration::from_secs(2));
            blocked.await.expect("blocked task");
            tokio::time::sleep(Duration::from_millis(1500)).await;
        });
        drop(watchdog);
        let lines = lines.lock().expect("lines");
        let blocked = lines
            .iter()
            .find(|line| line.contains("blocked in one task for"))
            .unwrap_or_else(|| panic!("{lines:?}"));
        // The sleeping worker waits in the kernel on a timer.
        assert!(
            blocked.contains("worker threads in the kernel: tid "),
            "{blocked}"
        );
        assert!(
            lines
                .iter()
                .any(|line| line.contains("resumed after being blocked for about")),
            "{lines:?}"
        );
        assert!(
            !lines
                .iter()
                .any(|line| line.starts_with("Forge event loop blocked for")),
            "{lines:?}"
        );
    }
}
