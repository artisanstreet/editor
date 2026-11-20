#[cfg(feature = "tokio")]
pub use paused::paused;

#[cfg(feature = "tokio")]
mod paused {
    pub async fn paused<F: Future>(work: F) -> F::Output {
        let _resume = Resume;
        tokio::time::pause();
        work.await
    }

    struct Resume;

    impl Drop for Resume {
        fn drop(&mut self) {
            tokio::time::resume();
        }
    }
}

#[cfg(feature = "clock")]
pub use manual::ManualClock;

#[cfg(feature = "clock")]
mod manual {
    use artisan_api::Timestamp;
    use editor_core::{Clock, TimeSource};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    #[derive(Debug)]
    pub struct ManualClock {
        start: Instant,
        start_timestamp: Timestamp,
        elapsed: Mutex<Duration>,
    }

    impl ManualClock {
        #[must_use]
        pub fn starting_at(start_timestamp: Timestamp) -> Arc<Self> {
            Arc::new(Self {
                start: Instant::now(),
                start_timestamp,
                elapsed: Mutex::new(Duration::ZERO),
            })
        }

        pub fn advance(&self, by: Duration) {
            *self
                .elapsed
                .lock()
                .expect("invariant: the manual clock lock is never poisoned") += by;
        }

        #[must_use]
        pub fn clock(self: &Arc<Self>) -> Clock {
            Clock::from_source(Arc::<Self>::clone(self))
        }

        fn elapsed(&self) -> Duration {
            *self
                .elapsed
                .lock()
                .expect("invariant: the manual clock lock is never poisoned")
        }
    }

    impl TimeSource for ManualClock {
        fn instant(&self) -> Instant {
            self.start + self.elapsed()
        }

        fn timestamp(&self) -> Timestamp {
            let elapsed = i64::try_from(self.elapsed().as_millis())
                .expect("invariant: a test clock advances less than i64::MAX milliseconds");
            self.start_timestamp + elapsed
        }
    }
}
