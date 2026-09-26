//! One continuous dispatch-lease heartbeat per claim execution.
//!
//! A claim lives from `claim_next_message_dispatch` through launch, provider
//! startup, binding, and the whole turn. Provider startup alone can outlast
//! the lease (a slow CLI announcing its session), and an unrenewed lease is
//! reaped by the live recovery sweep with an unknown outcome even though
//! this dispatcher still owns the work. The claim execution therefore owns
//! exactly one heartbeat from claim to settlement.
//!
//! Launch and bind fence the exact persisted lease window, so the heartbeat
//! and those two commands are serialized through the window lock: they read
//! the current window, and a renewal never moves it underneath them.

use std::time::Duration;

use artisan_database::{ClaimedMessageDispatch, Repository};
use artisan_domain::UnixMillis;
use tokio::sync::{Mutex, MutexGuard};

use crate::SystemCommandOrigin;

use super::dispatch_support::{add_duration, claim_renew_interval, wall_clock};

/// The live lease of one claimed dispatch.
pub(crate) struct ClaimLease {
    claim_lease: Duration,
    current: Mutex<ClaimedMessageDispatch>,
}

impl ClaimLease {
    pub(crate) fn new(claimed: &ClaimedMessageDispatch, claim_lease: Duration) -> Self {
        Self {
            claim_lease,
            current: Mutex::new(claimed.renewed(claimed.lease_expires_at, claimed.updated_at)),
        }
    }

    /// Holds the current lease window for one window-fenced command.
    ///
    /// Renewal waits while the guard lives, so the command sees exactly the
    /// persisted window. Never hold it across provider waits.
    pub(super) async fn hold(&self) -> MutexGuard<'_, ClaimedMessageDispatch> {
        self.current.lock().await
    }

    /// Records a window a command itself moved (launch stamps `updated_at`).
    pub(super) fn record(
        guard: &mut MutexGuard<'_, ClaimedMessageDispatch>,
        lease_expires_at: UnixMillis,
        updated_at: UnixMillis,
    ) {
        **guard = guard.renewed(lease_expires_at, updated_at);
    }

    /// Best-effort owner-fenced renewal. A failed renewal is never fatal by
    /// itself: if the lease truly lapsed, recovery owns the outcome.
    pub(crate) async fn renew(&self, repository: &Repository, origin: &SystemCommandOrigin) {
        let mut current = self.current.lock().await;
        let Some(operated_at) = wall_clock(origin) else {
            return;
        };
        let Some(lease_expires_at) = add_duration(operated_at, self.claim_lease) else {
            return;
        };
        if let Ok(renewed) = repository
            .renew_message_dispatch_lease(
                &current.message_id,
                &current.owner,
                operated_at,
                lease_expires_at,
            )
            .await
        {
            Self::record(&mut current, lease_expires_at, renewed.updated_at);
        }
    }
}

/// Drives `work` while heartbeating `lease` every third of its lifetime.
///
/// `work` may hold the SQLite writer across an await; it keeps being polled
/// while a renewal waits for that same writer (or for the window lock held
/// by a launch or bind inside `work`), so it can finish its transaction or
/// react to cancellation.
pub(crate) async fn drive_with_claim_lease<F>(
    repository: &Repository,
    origin: &SystemCommandOrigin,
    lease: &ClaimLease,
    work: F,
) -> F::Output
where
    F: std::future::Future,
{
    tokio::pin!(work);
    let renew_interval = claim_renew_interval(lease.claim_lease);
    let mut renew_at =
        tokio::time::interval_at(tokio::time::Instant::now() + renew_interval, renew_interval);
    renew_at.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            output = &mut work => break output,
            () = async {
                renew_at.tick().await;
                lease.renew(repository, origin).await;
            } => {}
        }
    }
}
