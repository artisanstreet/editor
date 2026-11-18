use crate::Hub;
use artisan_api::{Continuation, Run, Timestamp, UsageLimits};
use forge_engines::{Outcome, Retry};
use forge_store::{CONTINUE_AFTER_USAGE_LIMIT_KEY, Park, RETRY_WHEN_OVERLOADED_KEY};
use std::time::Duration;

#[cfg(not(test))]
const FIRST_RETRY: Duration = Duration::from_secs(15);
#[cfg(test)]
const FIRST_RETRY: Duration = Duration::from_millis(40);
const LONGEST_RETRY: Duration = FIRST_RETRY.saturating_mul(20);
const UNKNOWN_RESET: Duration = FIRST_RETRY.saturating_mul(60);
const RESET_SLACK: Duration = FIRST_RETRY.saturating_mul(2);

pub(crate) async fn plan(hub: &Hub, run: &Run, outcome: &Outcome) -> Option<Park> {
    let now = artisan_api::now();
    match outcome.retry()? {
        Retry::Overloaded => {
            if !hub.enabled(RETRY_WHEN_OVERLOADED_KEY).await {
                return None;
            }
            let tries = hub
                .store()
                .continuation_chain(run.thread_id.clone(), Continuation::Overloaded, 16)
                .await
                .inspect_err(|error| {
                    tracing::warn!(thread = %run.thread_id, "count overload retries: {}", diagnostics::chain(&error));
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

fn retry_delay(tries: usize) -> Duration {
    let doublings = u32::try_from(tries.min(8)).unwrap_or(8);
    (FIRST_RETRY * 2_u32.pow(doublings)).min(LONGEST_RETRY)
}

fn usage_due(resets_at: Option<Timestamp>, now: Timestamp) -> Timestamp {
    match resets_at {
        Some(resets_at) if resets_at > now => resets_at + millis(RESET_SLACK),
        _ => now + millis(UNKNOWN_RESET),
    }
}

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

const RECHECK: Duration = Duration::from_secs(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Parked {
    Due(Timestamp),
    Nothing,
    Unreadable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Wake {
    After(Duration),
    Never,
}

pub(crate) fn wake(parked: Parked, now: Timestamp, stalled: bool) -> Wake {
    match parked {
        Parked::Nothing => Wake::Never,
        Parked::Unreadable => Wake::After(RECHECK),
        Parked::Due(due_at) => {
            let wait = Duration::from_millis(u64::try_from(due_at - now).unwrap_or(0));
            Wake::After(if stalled { wait.max(RECHECK) } else { wait })
        }
    }
}

pub(crate) async fn until_next(hub: &Hub, stalled: bool) -> impl Future<Output = ()> + use<> {
    let parked = match hub.store().next_parked().await {
        Ok(Some(due_at)) => Parked::Due(due_at),
        Ok(None) => Parked::Nothing,
        Err(error) => {
            tracing::error!("read the next parked turn: {}", diagnostics::chain(&error));
            Parked::Unreadable
        }
    };
    let wake = wake(parked, artisan_api::now(), stalled);
    async move {
        match wake {
            Wake::After(wait) => tokio::time::sleep(wait).await,
            Wake::Never => std::future::pending().await,
        }
    }
}

#[cfg(test)]
#[path = "../tests/park/unit.rs"]
mod tests;
