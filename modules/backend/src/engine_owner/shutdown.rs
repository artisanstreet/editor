//! Bounded owner-shutdown observation for [`EngineOwner`].
//!
//! A health quarantine that predates the shutdown call emits no further watch
//! change, and a quarantine tail that retains child custody parks on
//! `eventual_wait_once` indefinitely. Shutdown therefore reads the current
//! health before arming any waiter and gives an already-quarantined join a
//! bounded settle grace: an observed completion is preferred, but a parked
//! custody tail resolves to the typed incomplete `Quarantined` report instead
//! of hanging forever.

#![forbid(unsafe_code)]

use std::time::Duration;

use super::operation::HealthState as OwnerHealth;
use super::{EngineOwner, EngineOwnerShutdown};

/// Bounded grace for a quarantine tail that may still settle.
///
/// Large enough for a just-killed child to be reaped by the owner task, small
/// enough that an already-quarantined shutdown stays prompt. The grace is
/// intentionally a constant: the facade carries no per-instance budgets.
const QUARANTINE_SETTLE_GRACE: Duration = Duration::from_millis(100);

/// Observes one shutdown of `owner`, bounded on every path.
pub(super) async fn observe(owner: &mut EngineOwner) -> EngineOwnerShutdown {
    let read_rx = owner.health.clone();
    let mut wait_rx = owner.health.clone();
    match owner.observed_join {
        Some(joined_cleanly) => return verdict(joined_cleanly),
        None if *read_rx.borrow() == OwnerHealth::Quarantined => {
            return settle_quarantined(owner).await;
        }
        None => {}
    }
    let mut changed = Box::pin(wait_rx.changed());
    loop {
        if let Some(joined_cleanly) = owner.observed_join {
            return verdict(joined_cleanly);
        }
        tokio::select! {
            biased;

            joined = &mut owner.join => {
                let joined_cleanly = joined.is_ok();
                owner.observed_join = Some(joined_cleanly);
                return verdict(joined_cleanly);
            }
            _ = &mut changed => {
                drop(changed);
                if *read_rx.borrow() == OwnerHealth::Quarantined {
                    return settle_quarantined(owner).await;
                }
                changed = Box::pin(wait_rx.changed());
            }
        }
    }
}

/// Waits the settle grace for a quarantined join, then reports honestly.
async fn settle_quarantined(owner: &mut EngineOwner) -> EngineOwnerShutdown {
    match tokio::time::timeout(QUARANTINE_SETTLE_GRACE, &mut owner.join).await {
        Ok(joined) => {
            let joined_cleanly = joined.is_ok();
            owner.observed_join = Some(joined_cleanly);
            verdict(joined_cleanly)
        }
        Err(_) => EngineOwnerShutdown::Quarantined,
    }
}

const fn verdict(joined_cleanly: bool) -> EngineOwnerShutdown {
    if joined_cleanly {
        EngineOwnerShutdown::Joined
    } else {
        EngineOwnerShutdown::TaskLost
    }
}
