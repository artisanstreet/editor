use std::sync::LazyLock;
use std::time::Instant;

static EPOCH: LazyLock<Instant> = LazyLock::new(Instant::now);

pub fn pulse_ms() -> u32 {
    EPOCH.elapsed().as_millis() as u32
}
