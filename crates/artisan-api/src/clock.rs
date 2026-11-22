use crate::Timestamp;
use std::sync::Arc;

#[derive(Clone)]
pub struct WallClock(Arc<dyn Fn() -> Timestamp + Send + Sync>);

impl WallClock {
    #[must_use]
    pub fn system() -> Self {
        Self::new(crate::now)
    }

    pub fn new(read: impl Fn() -> Timestamp + Send + Sync + 'static) -> Self {
        Self(Arc::new(read))
    }

    #[must_use]
    pub fn now(&self) -> Timestamp {
        (self.0)()
    }
}

impl std::fmt::Debug for WallClock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("WallClock").field(&self.now()).finish()
    }
}
