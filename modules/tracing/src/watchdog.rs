use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use crate::recorder::{incident, instant, micros};

/// Tick on the thread being monitored; a separate OS thread observes it.
/// Starts armed only after the first tick, excluding application startup.
pub struct Heartbeat {
    origin: Instant,
    stamp: Arc<AtomicU64>,
    running: Arc<AtomicBool>,
}

impl Heartbeat {
    #[must_use]
    pub fn new(name: &'static str) -> Self {
        let origin = Instant::now();
        let stamp = Arc::new(AtomicU64::new(u64::MAX));
        let running = Arc::new(AtomicBool::new(true));
        let watched = Arc::clone(&stamp);
        let active = Arc::clone(&running);
        let _ = std::thread::Builder::new()
            .name("artisan-trace-watchdog".into())
            .spawn(move || {
                let mut stalled = false;
                while active.load(Ordering::Relaxed) {
                    let before = Instant::now();
                    std::thread::sleep(Duration::from_millis(100));
                    if before.elapsed() > Duration::from_secs(1) {
                        instant(
                            "watchdog",
                            "process.paused",
                            serde_json::json!({"monitor":name}),
                        );
                        stalled = false;
                        continue;
                    }
                    let last = watched.load(Ordering::Relaxed);
                    if last == u64::MAX {
                        continue;
                    }
                    let lag = micros(origin.elapsed()).saturating_sub(last);
                    if lag > 500_000 && !stalled {
                        instant(
                            "watchdog",
                            "heartbeat.stalled",
                            serde_json::json!({"monitor":name, "lag_ms":lag / 1000}),
                        );
                        incident("heartbeat.stalled");
                        stalled = true;
                    } else if lag <= 500_000 && stalled {
                        instant(
                            "watchdog",
                            "heartbeat.resumed",
                            serde_json::json!({"monitor":name}),
                        );
                        incident("heartbeat.resumed");
                        stalled = false;
                    }
                }
            });
        Self {
            origin,
            stamp,
            running,
        }
    }

    pub fn tick(&self) {
        self.stamp
            .store(micros(self.origin.elapsed()), Ordering::Relaxed);
    }
}

impl Drop for Heartbeat {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
    }
}
