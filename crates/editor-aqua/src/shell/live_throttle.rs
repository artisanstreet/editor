use std::time::{Duration, Instant};

pub const INTERVAL: Duration = Duration::from_micros(16_667);

#[derive(Default)]
pub struct LiveThrottle {
    applied: Option<Instant>,
    deferred: bool,
}

impl LiveThrottle {
    pub fn arrive(&mut self, now: Instant) -> bool {
        match self.applied {
            Some(at) if now.saturating_duration_since(at) < INTERVAL => {
                self.deferred = true;
                false
            }
            _ => {
                self.applied = Some(now);
                self.deferred = false;
                true
            }
        }
    }

    pub fn due(&mut self, now: Instant) -> bool {
        match (self.deferred, self.applied) {
            (true, Some(at)) if now.saturating_duration_since(at) >= INTERVAL => {
                self.applied = Some(now);
                self.deferred = false;
                true
            }
            _ => false,
        }
    }

    pub fn flushed(&mut self, now: Instant) {
        self.applied = Some(now);
        self.deferred = false;
    }

    pub fn remaining(&self, now: Instant) -> Option<Duration> {
        match (self.deferred, self.applied) {
            (true, Some(at)) => Some(INTERVAL.saturating_sub(now.saturating_duration_since(at))),
            _ => None,
        }
    }
}
