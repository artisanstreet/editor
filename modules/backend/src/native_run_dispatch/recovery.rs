//! Durable live recovery pages and owner shutdown settling.
//!
//! The dispatcher sweeps expired leases between claims with the live
//! lease-expiry disposition (never labelled startup reconciliation, which
//! only the Forge startup pass records), and keeps shutting the single owner
//! down until custody settles. Every function here reads dispatcher policy or
//! the injected notifier and returns to the claim loop.

use artisan_database::Repository;
use artisan_domain::{PatchId, UnixMillis};
use artisan_transport::CancelHandle;

use crate::{
    SystemCommandOrigin,
    conversation_commit_notifier::ConversationCommitNotifier,
    engine_owner::{EngineOwner, EngineOwnerShutdown},
    startup_reconciliation_sweep::{
        PatchSourceError, StartupReconciliationPatchSource, StartupReconciliationPatches,
        StartupReconciliationSweepInput,
    },
};

use super::{
    NativeRunDispatcherConfig,
    dispatch_support::{wait_for_next_claim, wall_clock},
};

struct LiveRecoveryPatchSource {
    notifier: ConversationCommitNotifier,
}

impl StartupReconciliationPatchSource for LiveRecoveryPatchSource {
    fn patch_ids_for(
        &mut self,
        candidate: &artisan_database::StartupReconciliationCandidate,
    ) -> Result<StartupReconciliationPatches, PatchSourceError> {
        let turn_patch_id =
            PatchId::parse(candidate.run_id.as_str()).map_err(|_| PatchSourceError)?;
        let item_patch_id = candidate
            .assistant_item_id
            .as_ref()
            .map(|item_id| PatchId::parse(item_id.as_str()).map_err(|_| PatchSourceError))
            .transpose()?;
        Ok(StartupReconciliationPatches::new(
            turn_patch_id,
            item_patch_id,
        ))
    }

    fn on_durable_disposition(
        &mut self,
        candidate: &artisan_database::StartupReconciliationCandidate,
    ) {
        let _ = self.notifier.publish(&candidate.thread_id);
    }
}

async fn perform_live_recovery_page(
    repository: &Repository,
    config: &NativeRunDispatcherConfig,
    operated_at: UnixMillis,
) -> Result<
    crate::startup_reconciliation_sweep::StartupReconciliationSweepReport,
    Box<crate::startup_reconciliation_sweep::StartupReconciliationSweepError>,
> {
    let input =
        StartupReconciliationSweepInput::live_lease_expiry(operated_at, 64).map_err(Box::new)?;
    let mut source = LiveRecoveryPatchSource {
        notifier: config.conversation_commit_notifier(),
    };
    crate::startup_reconciliation_sweep::sweep_startup_reconciliation(
        repository,
        input,
        &mut source,
    )
    .await
    .map_err(Box::new)
}

pub(super) async fn run_recovery_pages(
    repository: &Repository,
    config: &NativeRunDispatcherConfig,
    origin: &SystemCommandOrigin,
    stop: &CancelHandle,
    process_cancel: &CancelHandle,
) -> bool {
    loop {
        if stop.is_cancelled() || process_cancel.is_cancelled() {
            return false;
        }
        let Some(operated_at) = wall_clock(origin) else {
            if !wait_for_next_claim(stop, process_cancel, config.poll_interval).await {
                return false;
            }
            return false;
        };
        if let Ok(report) = perform_live_recovery_page(repository, config, operated_at).await {
            if report.discovered == 64 {
                if !wait_for_next_claim(stop, process_cancel, config.poll_interval).await {
                    return false;
                }
                continue;
            }
            return true;
        }
        if !wait_for_next_claim(stop, process_cancel, config.poll_interval).await {
            return false;
        }
        return false;
    }
}

pub(super) async fn run_final_recovery_page(
    repository: &Repository,
    config: &NativeRunDispatcherConfig,
    origin: &SystemCommandOrigin,
) {
    let Some(operated_at) = wall_clock(origin) else {
        return;
    };
    let _ = perform_live_recovery_page(repository, config, operated_at).await;
}

/// Shuts the owner down exactly once and returns its bounded verdict.
///
/// `EngineOwner::shutdown` already bounds an already-quarantined tail, so
/// looping on `Quarantined` would spin forever on retained custody and
/// re-introduce the very dispatcher shutdown hang this path exists to avoid.
pub(super) async fn shutdown_owner_bounded(owner: &mut EngineOwner) -> EngineOwnerShutdown {
    owner.shutdown().await
}
