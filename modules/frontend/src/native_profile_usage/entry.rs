//! One engine's usage report as the profile menu keeps it.
//!
//! The Forge serves (on an explicit read) and pushes (whenever it changes)
//! each engine's usage as a narrowed snapshot carrying exactly that engine's
//! report and observation time. The conversion maps provider-owned facts
//! without inference: percentages come from the provider windows verbatim
//! (already clamped at the domain edge), and a refresh failure travels in
//! `report.failure` beside the retained windows.

use artisan_domain::{EngineUsageSnapshot, QuotaSurface};

use super::{
    NativeUsageAuthentication, NativeUsageCadence, NativeUsageEntry, NativeUsageQuotaSurface,
    NativeUsageReport, NativeUsageWindow, profile_usage_display_name,
};
use crate::usage_reset_duration::parse_iso_timestamp_ms;

/// Converts one narrowed snapshot into its engine's row.
///
/// Returns `None` unless the snapshot carries exactly one report and its
/// `fetched_at` parses by the shared ISO policy.
#[must_use]
pub fn usage_entry(snapshot: &EngineUsageSnapshot) -> Option<NativeUsageEntry> {
    let fetched_at_ms = parse_iso_timestamp_ms(snapshot.fetched_at())?;
    let [report] = snapshot.engines() else {
        return None;
    };
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
