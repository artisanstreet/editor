use std::sync::atomic::{AtomicI64, Ordering};

static OFFSET_MS: AtomicI64 = AtomicI64::new(0);

pub fn step(by_ms: i64) {
    OFFSET_MS.fetch_add(by_ms, Ordering::AcqRel);
}

pub fn reset() {
    OFFSET_MS.store(0, Ordering::Release);
}

pub(crate) fn offset() -> i64 {
    OFFSET_MS.load(Ordering::Acquire)
}
