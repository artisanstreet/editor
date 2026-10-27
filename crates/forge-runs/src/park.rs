//! Failed turns that pass by themselves wait, parked, and Forge picks them
//! up again: a provider at capacity is retried, waiting longer each time
//! and never giving up; a used-up plan carries on once its limit resets.
//! Each is a Forge setting, on unless turned off. The store keeps the
//! parked turn as a continue request with a due time, so it survives a
//! restart, and the thread shows when it goes; the dispatcher wakes for
//! the earliest one.

use crate::Hub;
use artisan_api::{Continuation, Run, Timestamp, UsageLimits};
use forge_engines::{Outcome, Retry};
use forge_store::{CONTINUE_AFTER_USAGE_LIMIT_KEY, Park, RETRY_WHEN_OVERLOADED_KEY};
use std::time::Duration;

/// The first retry after an overload; each next one waits twice as long.
/// Tests wait milliseconds where Forge waits seconds.
#[cfg(not(test))]
const FIRST_RETRY: Duration = Duration::from_secs(15);
#[cfg(test)]
const FIRST_RETRY: Duration = Duration::from_millis(40);
/// The longest wait between retries: 5 minutes.
const LONGEST_RETRY: Duration = FIRST_RETRY.saturating_mul(20);
/// When the reset is unknown, how long until the turn tries again.
const UNKNOWN_RESET: Duration = FIRST_RETRY.saturating_mul(60);
/// Past the reset, so the provider has rolled the window over.
const RESET_SLACK: Duration = FIRST_RETRY.saturating_mul(2);

/// When the failed `run` should be picked up again, if it should.
pub async fn plan(hub: &Hub, run: &Run, outcome: &Outcome) -> Option<Park> {
    let now = artisan_api::now();
    match outcome.retry? {
        Retry::Overloaded => {
            if !hub.enabled(RETRY_WHEN_OVERLOADED_KEY).await {
                return None;
            }
            // This run counts when it was itself a retry.
            let tries = hub
                .store()
                .continuation_chain(run.thread_id.clone(), Continuation::Overloaded, 16)
                .await
                .inspect_err(|error| {
                    tracing::warn!(thread = %run.thread_id, "count overload retries: {error:#}");
                })
                .unwrap_or(0);
            Some(Park {
                continuation: Continuation::Overloaded,
                due_at: now + millis(retry_delay(tries)),
            })
        }
        Retry::UsageLimit { resets_at } => {
            if !hub.enabled(CONTINUE_AFTER_USAGE_LIMIT_KEY).await {
                return None;
            }
            let resets_at = match resets_at {
                Some(resets_at) => Some(resets_at),
                None => hub
                    .engines()
                    .refresh_usage(run.engine, true)
                    .await
                    .and_then(|limits| exhausted_reset(&limits, now)),
            };
            Some(Park {
                continuation: Continuation::UsageLimit,
                due_at: usage_due(resets_at, now),
            })
        }
    }
}

/// How long the `tries`-th retry in a row waits.
fn retry_delay(tries: usize) -> Duration {
    let doublings = u32::try_from(tries.min(8)).unwrap_or(8);
    (FIRST_RETRY * 2_u32.pow(doublings)).min(LONGEST_RETRY)
}

/// When a turn the usage limit stopped tries again: past the reset, or a
/// while from now when the reset is unknown or already behind us.
fn usage_due(resets_at: Option<Timestamp>, now: Timestamp) -> Timestamp {
    match resets_at {
        Some(resets_at) if resets_at > now => resets_at + millis(RESET_SLACK),
        _ => now + millis(UNKNOWN_RESET),
    }
}

/// The latest reset among the windows that ran out: the turn can't go
/// until every one of them has reset.
fn exhausted_reset(limits: &UsageLimits, now: Timestamp) -> Option<Timestamp> {
    limits
        .windows
        .iter()
        .filter(|window| window.used_percent >= 100.0)
        .filter_map(|window| window.resets_at)
        .filter(|resets_at| *resets_at > now)
        .max()
}

fn millis(duration: Duration) -> Timestamp {
    Timestamp::try_from(duration.as_millis()).unwrap_or(Timestamp::MAX)
}

/// Waits until the earliest parked turn is due; forever when none is.
pub async fn until_next(hub: &Hub) -> impl Future<Output = ()> + use<> {
    let next = hub.store().next_parked().await.unwrap_or_else(|error| {
        tracing::error!("read the next parked turn: {error:#}");
        None
    });
    async move {
        match next {
            Some(due_at) => {
                let wait = u64::try_from(due_at - artisan_api::now()).unwrap_or(0);
                tokio::time::sleep(Duration::from_millis(wait)).await;
            }
            None => std::future::pending().await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use artisan_api::{UsageCadence, UsageWindow};

    #[test]
    fn overload_retries_back_off_to_five_minutes_and_keep_going() {
        // In Forge: 15 s, 30 s, 1 min, 2 min, 4 min, then every 5 min.
        let waits: Vec<u32> = (0..7)
            .map(|tries| {
                u32::try_from(retry_delay(tries).as_millis() / FIRST_RETRY.as_millis()).unwrap()
            })
            .collect();
        assert_eq!(waits, [1, 2, 4, 8, 16, 20, 20]);
        assert_eq!(retry_delay(10_000), LONGEST_RETRY);
    }

    #[test]
    fn a_used_up_plan_waits_for_its_last_exhausted_window() {
        let window = |used_percent: f64, resets_at: Timestamp| UsageWindow {
            label: String::new(),
            cadence: UsageCadence::Other,
            scope: None,
            used_percent,
            resets_at: Some(resets_at),
        };
        let limits = UsageLimits {
            windows: vec![
                window(100.0, 5_000),
                window(40.0, 9_000),
                window(100.0, 7_000),
            ],
            spend: None,
            read_at: 0,
        };
        assert_eq!(exhausted_reset(&limits, 1_000), Some(7_000));
        assert_eq!(usage_due(Some(7_000), 1_000), 7_000 + millis(RESET_SLACK));
        assert_eq!(usage_due(None, 1_000), 1_000 + millis(UNKNOWN_RESET));
        assert_eq!(usage_due(Some(500), 1_000), 1_000 + millis(UNKNOWN_RESET));
    }
}
