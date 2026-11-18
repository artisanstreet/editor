use artisan_api::Timestamp;
use std::sync::Arc;
use std::time::Instant;

pub trait TimeSource: Send + Sync {
    fn instant(&self) -> Instant;
    fn timestamp(&self) -> Timestamp;
}

struct SystemTime;

impl TimeSource for SystemTime {
    fn instant(&self) -> Instant {
        Instant::now()
    }

    fn timestamp(&self) -> Timestamp {
        artisan_api::now()
    }
}

#[derive(Clone)]
pub struct Clock(Arc<dyn TimeSource>);

impl Clock {
    #[must_use]
    pub fn system() -> Self {
        Self(Arc::new(SystemTime))
    }

    #[must_use]
    pub fn from_source(source: Arc<dyn TimeSource>) -> Self {
        Self(source)
    }

    #[must_use]
    pub fn now(&self) -> Instant {
        self.0.instant()
    }

    #[must_use]
    pub fn timestamp(&self) -> Timestamp {
        self.0.timestamp()
    }
}
