//! Correlated provider-account usage reads over the existing transport, made
//! only for the user's explicit refresh (the Forge pushes every change).
//!
//! The parent `native_transport_service` remains the owner of the session,
//! frame factory, reconnect policy, and public transport enums. Root mounts
//! this file as a child module so it can use the existing private
//! `ServiceRuntime`, `ExpectedResponse`, `publish`, `account_usage_request`,
//! and failure seams without widening those internals.
//!
//! The adapter never invokes provider CLIs and never fabricates quota state.
//! Each read narrows to one engine so its snapshot `fetched_at` represents
//! that provider. The original per-provider observation time is preserved;
//! the client receipt clock is never substituted.

#![forbid(unsafe_code)]
#![allow(clippy::module_name_repetitions)]

use artisan_domain::EngineUsageSnapshot;
use artisan_protocol::ResponsePayload;

use super::*;
use crate::native_profile_usage::{NativeUsageEntry, ProfileUsageGeneration, usage_entry};

/// Handles one account-usage read on the existing authenticated runtime.
///
/// The read reuses the existing bounded `runtime.request` path, so the
/// existing request deadline, admission budget, and cancellation behavior
/// apply unchanged; no separate transport owner is invented. The serial
/// command loop still awaits each narrowed query, so per-engine
/// deduplication upstream bounds the worst-case delay for composer/control
/// commands.
pub(super) async fn read_account_usage(
    runtime: &mut ServiceRuntime,
    frames: &mut FrameFactory,
    events: &SyncSender<NativeTransportEvent>,
    engine_id: String,
    generation: ProfileUsageGeneration,
    request_seq: u64,
    force: bool,
) -> Result<(), ServiceFailure> {
    let request = match account_usage_request(&engine_id, force) {
        Ok(request) => request,
        Err(failure) => {
            return publish(
                events,
                NativeTransportEvent::AccountUsageFailed {
                    engine_id,
                    generation,
                    request_seq,
                    failure,
                },
            );
        }
    };
    let payload = match runtime
        .request(
            frames,
            request,
            ExpectedResponse::AccountUsage {
                engine_id: engine_id.clone(),
            },
        )
        .await
    {
        Ok(payload) => payload,
        Err(error) => {
            let failure: ServiceFailure = error.into();
            return publish(
                events,
                NativeTransportEvent::AccountUsageFailed {
                    engine_id,
                    generation,
                    request_seq,
                    failure,
                },
            );
        }
    };
    let ResponsePayload::AccountUsage(snapshot) = payload else {
        return publish(
            events,
            NativeTransportEvent::AccountUsageFailed {
                engine_id,
                generation,
                request_seq,
                failure: ServiceFailure::invalid(ServiceFailureStage::Request),
            },
        );
    };
    match snapshot_to_entry(&engine_id, &snapshot) {
        Some(entry) => publish(
            events,
            NativeTransportEvent::AccountUsage {
                engine_id,
                generation,
                request_seq,
                entry,
            },
        ),
        None => publish(
            events,
            NativeTransportEvent::AccountUsageFailed {
                engine_id,
                generation,
                request_seq,
                failure: ServiceFailure::new(
                    ServiceFailureStage::Request,
                    ServiceFailureCategory::Integrity,
                ),
            },
        ),
    }
}

/// Converts one narrowed snapshot into a provider-owned frontend row.
///
/// Returns `None` unless the snapshot carries exactly one report, it belongs
/// to `engine_id`, and its `fetched_at` parses by the shared ISO policy.
fn snapshot_to_entry(engine_id: &str, snapshot: &EngineUsageSnapshot) -> Option<NativeUsageEntry> {
    usage_entry(snapshot).filter(|entry| entry.engine_id == engine_id)
}
