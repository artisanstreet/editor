//! Correlated provider-account usage reads over the existing transport.
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

use artisan_domain::{EngineUsageSnapshot, QuotaSurface};
use artisan_protocol::ResponsePayload;

use super::*;
use crate::native_profile_usage::{
    NativeUsageAuthentication, NativeUsageCadence, NativeUsageEntry, NativeUsageQuotaSurface,
    NativeUsageReport, NativeUsageWindow, ProfileUsageGeneration, profile_usage_display_name,
};
use crate::usage_reset_duration::parse_iso_timestamp_ms;

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
/// Returns `None` unless the snapshot carries exactly one report and it
/// belongs to `engine_id`, or its `fetched_at` cannot be parsed by the shared
/// ISO policy. Percentages come from the provider windows verbatim (already
/// clamped at the domain edge); quota surfaces and authentication states are
/// mapped without inference. A provider failure travels inside
/// `report.failure` alongside the retained windows; it never clears them.
fn snapshot_to_entry(engine_id: &str, snapshot: &EngineUsageSnapshot) -> Option<NativeUsageEntry> {
    let fetched_at_ms = parse_iso_timestamp_ms(snapshot.fetched_at())?;
    let engines = snapshot.engines();
    if engines.len() != 1 {
        return None;
    }
    let report = engines.first()?;
    if report.engine_id() != engine_id {
        return None;
    }
    Some(NativeUsageEntry {
        engine_id: report.engine_id().to_owned(),
        display_name: display_name_or_roster(report.display_name(), report.engine_id()),
        report: Some(NativeUsageReport {
            engine_id: report.engine_id().to_owned(),
            display_name: report.display_name().to_owned(),
            authentication: map_authentication(report.authentication().state()),
            account_email: report.account_email().map(str::to_owned),
            quota_surface: map_quota_surface(report.quota_surface()),
            windows: report
                .windows()
                .iter()
                .map(|window| NativeUsageWindow {
                    id: window.id().to_owned(),
                    cadence: map_cadence(window.kind()),
                    label: window.label().map(str::to_owned),
                    percent_used: window.percent_used(),
                    resets_at: window.resets_at().map(str::to_owned),
                    window_minutes: window.window_minutes(),
                })
                .collect(),
            failure: report.failure().map(str::to_owned),
            readiness: report.readiness().clone(),
        }),
        failure: None,
        fetched_at_ms: Some(fetched_at_ms),
    })
}

fn display_name_or_roster(provider_name: &str, engine_id: &str) -> String {
    if provider_name.is_empty() {
        profile_usage_display_name(engine_id).to_owned()
    } else {
        provider_name.to_owned()
    }
}

fn map_authentication(
    state: artisan_domain::EngineUsageAuthentication,
) -> NativeUsageAuthentication {
    match state {
        artisan_domain::EngineUsageAuthentication::Authenticated => {
            NativeUsageAuthentication::Authenticated
        }
        artisan_domain::EngineUsageAuthentication::Unauthenticated => {
            NativeUsageAuthentication::Unauthenticated
        }
        artisan_domain::EngineUsageAuthentication::Unknown => NativeUsageAuthentication::Unknown,
    }
}

fn map_cadence(kind: artisan_domain::EngineUsageWindowKind) -> NativeUsageCadence {
    match kind {
        artisan_domain::EngineUsageWindowKind::Session => NativeUsageCadence::Session,
        artisan_domain::EngineUsageWindowKind::Weekly => NativeUsageCadence::Weekly,
        artisan_domain::EngineUsageWindowKind::Monthly => NativeUsageCadence::Monthly,
        artisan_domain::EngineUsageWindowKind::Unknown => NativeUsageCadence::Unknown,
    }
}

fn map_quota_surface(surface: Option<QuotaSurface>) -> NativeUsageQuotaSurface {
    match surface {
        Some(QuotaSurface::Supported) => NativeUsageQuotaSurface::Supported,
        Some(QuotaSurface::Unsupported) => NativeUsageQuotaSurface::Unsupported,
        Some(QuotaSurface::Unknown) | None => NativeUsageQuotaSurface::Unknown,
    }
}
