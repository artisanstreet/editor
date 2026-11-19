use std::sync::LazyLock;
use std::time::Instant;

static EPOCH: LazyLock<Instant> = LazyLock::new(Instant::now);

pub fn pulse_ms() -> u32 {
    u32::try_from(EPOCH.elapsed().as_millis() & u128::from(u32::MAX))
        .expect("invariant: masked to 32 bits, so the pulse wraps every ~49 days")
}
