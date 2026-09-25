//! The Forge's account-readiness decisions: whether an engine's account can
//! run, and which harnesses a served catalog admits because of it.
//!
//! The verdict is judged from the Forge's own probed usage reads against the
//! account-usage freshness window, never from a client clock. It replaces the
//! Editor's `engine_readiness` / `catalog_with_usage_readiness` overlay: the
//! Editor now renders the verdict each usage report carries and the runnable
//! harnesses of the catalog it is served.

#![forbid(unsafe_code)]

use artisan_catalog::NativeModelCatalog;
use artisan_domain::{
    EngineReadiness, EngineReadinessVerdict, EngineUsageAuthentication, EngineUsageReport,
};

use crate::account_usage_service::AccountUsageService;

/// Engines whose usage read proves a responding local CLI, so only their
/// authenticated fresh usage admits the engine's models to run.
///
/// Codex reads `account/rateLimits/read` and Claude runs `claude -p /usage`
/// through the installed executable. Cursor's dashboard read proves an
/// account, never a local installation, so Cursor (like the surfaceless
/// Grok Build and `OpenCode`) keeps the catalog's own runtime marking.
pub(crate) const CLI_PROBED_ENGINES: [&str; 2] = ["codex", "claude"];

/// Judges one engine's readiness.
///
/// `report` is the last usage the Forge observed for the engine and `fresh`
/// whether it is still inside the freshness window; `failure` is the latest
/// refresh failure when no report explains it. An authenticated fresh report
/// is ready even when a refresh failure is served alongside it (the Forge
/// serves last-good on transient failures); a stale one is not. Reasons are
/// complete sentences a client shows as they are.
#[must_use]
pub(crate) fn engine_readiness(
    display_name: &str,
    report: Option<&EngineUsageReport>,
    fresh: bool,
    failure: Option<&str>,
) -> EngineReadiness {
    let Some(report) = report else {
        return match failure {
            Some(failure) => verdict(
                EngineReadinessVerdict::NotReady,
                format!("{display_name} account status is unavailable: {failure}."),
            ),
            None => verdict(
                EngineReadinessVerdict::Checking,
                format!("Checking the {display_name} account status."),
            ),
        };
    };
    match report.authentication().state() {
        EngineUsageAuthentication::Authenticated if fresh => EngineReadiness::ready(),
        EngineUsageAuthentication::Authenticated if report.failure().is_none() => verdict(
            EngineReadinessVerdict::NotReady,
            format!("{display_name} account status is out of date; refresh to check it again."),
        ),
        EngineUsageAuthentication::Unauthenticated => verdict(
            EngineReadinessVerdict::NeedsSignIn,
            format!("{display_name} account sign-in is required."),
        ),
        EngineUsageAuthentication::Authenticated | EngineUsageAuthentication::Unknown => {
            match report.failure().or(failure) {
                Some(failure) => verdict(
                    EngineReadinessVerdict::NotReady,
                    format!("{display_name} account status is unavailable: {failure}."),
                ),
                None => verdict(
                    EngineReadinessVerdict::NotReady,
                    format!("{display_name} account status is unavailable right now."),
                ),
            }
        }
    }
}

fn verdict(verdict: EngineReadinessVerdict, reason: String) -> EngineReadiness {
    // Display names and failure reasons are bounded Forge-owned text well
    // inside the reason bound; an unrepresentable one degrades to a bare
    // verdict rather than a readiness the client cannot decode.
    EngineReadiness::new(verdict, Some(reason)).unwrap_or_else(|_| {
        EngineReadiness::new(verdict, None).expect("a verdict without a reason is valid")
    })
}

/// Applies the Forge's account readiness to a catalog it is about to serve.
///
/// The CLI-probed engines ([`CLI_PROBED_ENGINES`]) are runnable exactly when
/// their verdict is ready; every other runnable harness is kept as the
/// catalog marked it. Without the usage capability nothing proves a probed
/// engine's account, so neither is runnable.
#[must_use]
pub(crate) fn catalog_with_account_readiness(
    mut catalog: NativeModelCatalog,
    usage: Option<&AccountUsageService>,
) -> NativeModelCatalog {
    catalog.runnable_harness_ids.retain(|engine_id| {
        !CLI_PROBED_ENGINES.contains(&engine_id.as_str())
            || usage
                .and_then(|usage| usage.readiness(engine_id))
                .is_some_and(|readiness| readiness.is_ready())
    });
    catalog
}

#[cfg(test)]
#[path = "account_readiness/tests.rs"]
mod tests;
