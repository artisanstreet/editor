//! Reports when the Forge's single event-loop thread stops making progress.
//!
//! The Forge runs on one current-thread runtime, so blocking work on it
//! stalls every timer at once: dispatch lease heartbeats, request deadlines
//! and conversation delivery. A ticker task stamps progress; a plain OS
//! thread watches the stamp and reports a stall while it is happening and
//! again once it ends. When the watcher thread itself oversleeps, the whole
//! process was paused (host or VM suspend), which is reported separately so
//! the two causes are never confused.

#![forbid(unsafe_code)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// How often the ticker stamps progress.
const TICK: Duration = Duration::from_millis(250);
/// How often the watcher thread inspects the stamp.
const CHECK: Duration = Duration::from_millis(500);
/// Lag past which the event loop counts as blocked.
const STALL: Duration = Duration::from_secs(2);

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

    let watcher_running = Arc::clone(&running);
    let spawned = std::thread::Builder::new()
        .name("forge-event-loop-watchdog".to_owned())
        .spawn(move || watch(origin, &stamp, &watcher_running, report));
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

fn watch(origin: Instant, stamp: &AtomicU64, running: &AtomicBool, mut report: impl FnMut(String)) {
    let mut blocked_since: Option<u64> = None;
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
            continue;
        }
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
                    "Forge event loop blocked for {lag} ms: lease heartbeats, request deadlines and conversation delivery are stalled"
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

    use super::start_reporting;

    #[test]
    fn a_blocked_event_loop_is_reported_while_blocked_and_on_resume() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let lines = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&lines);
        let watchdog = start_reporting(&runtime, move |line| {
            sink.lock().expect("lines").push(line);
        })
        .expect("watchdog");
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
}
