use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug)]
pub(crate) struct Stopwatch(Instant);

impl Stopwatch {
    pub(crate) fn start() -> Self {
        Self(Instant::now())
    }

    pub(crate) fn measured(self) -> Duration {
        self.0.elapsed()
    }
}
