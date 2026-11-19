use std::time::{Duration, Instant};

pub const LIVENESS: Duration = Duration::from_secs(60);

const FIRST_PAUSE: Duration = Duration::from_micros(50);
const LONGEST_PAUSE: Duration = Duration::from_millis(10);

#[track_caller]
pub fn wait_for<T>(what: &str, limit: Duration, mut poll: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + limit;
    let mut pause = FIRST_PAUSE;
    loop {
        if let Some(value) = poll() {
            return value;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        assert!(
            !remaining.is_zero(),
            "timed out after {limit:?} waiting for {what}"
        );
        std::thread::park_timeout(pause.min(remaining));
        pause = (pause * 2).min(LONGEST_PAUSE);
    }
}

#[track_caller]
pub fn wait_until(what: &str, limit: Duration, mut condition: impl FnMut() -> bool) {
    wait_for(what, limit, || condition().then_some(()));
}

#[cfg(feature = "tokio")]
pub async fn within<F: Future>(what: &str, limit: Duration, future: F) -> F::Output {
    let Ok(output) = tokio::time::timeout(limit, future).await else {
        panic!("timed out after {limit:?} waiting for {what}");
    };
    output
}

#[cfg(feature = "tokio")]
pub async fn eventually<F, C>(what: &str, limit: Duration, mut condition: C)
where
    C: FnMut() -> F,
    F: Future<Output = bool>,
{
    within(what, limit, async {
        let mut pause = FIRST_PAUSE;
        while !condition().await {
            tokio::time::sleep(pause).await;
            pause = (pause * 2).min(LONGEST_PAUSE);
        }
    })
    .await;
}

#[cfg(feature = "tokio")]
pub async fn while_progressing<T, F, C>(
    what: &str,
    stall: Duration,
    mut progress: impl FnMut() -> u64,
    mut ready: C,
) -> T
where
    C: FnMut() -> F,
    F: Future<Output = Option<T>>,
{
    let mut seen = progress();
    let mut moved_at = tokio::time::Instant::now();
    let mut pause = FIRST_PAUSE;
    loop {
        if let Some(value) = ready().await {
            return value;
        }
        let now = progress();
        if now != seen {
            seen = now;
            moved_at = tokio::time::Instant::now();
        }
        assert!(
            moved_at.elapsed() < stall,
            "nothing moved for {stall:?} while waiting for {what}"
        );
        tokio::time::sleep(pause).await;
        pause = (pause * 2).min(LONGEST_PAUSE);
    }
}
