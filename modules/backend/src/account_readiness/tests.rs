use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use artisan_domain::{EngineUsageAuth, QuotaSurface, ReadAccountUsage};
use artisan_native_engine::account_usage::ProviderUsage;

use super::*;
use crate::account_usage_service::{AccountUsageReader, ReaderFailure};

#[derive(Clone, Debug)]
enum Outcome {
    Authenticated,
    Unauthenticated,
    Failure,
}

#[derive(Debug)]
struct Scripted {
    engine_id: &'static str,
    display_name: &'static str,
    outcome: Outcome,
}

impl AccountUsageReader for Scripted {
    fn engine_id(&self) -> &'static str {
        self.engine_id
    }

    fn display_name(&self) -> &'static str {
        self.display_name
    }

    fn read(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<ProviderUsage, ReaderFailure>> + Send + '_>> {
        let outcome = self.outcome.clone();
        Box::pin(async move {
            match outcome {
                Outcome::Authenticated => Ok(ProviderUsage::authenticated(Vec::new())),
                Outcome::Unauthenticated => Ok(ProviderUsage::unauthenticated("signed out")),
                Outcome::Failure => Err(ReaderFailure::unavailable("provider went down")),
            }
        })
    }
}

fn service(
    freshness: Duration,
    readers: &[(&'static str, &'static str, Outcome)],
) -> AccountUsageService {
    AccountUsageService::with_readers(
        readers
            .iter()
            .map(|(engine_id, display_name, outcome)| {
                Arc::new(Scripted {
                    engine_id,
                    display_name,
                    outcome: outcome.clone(),
                }) as Arc<dyn AccountUsageReader>
            })
            .collect(),
        freshness,
        Duration::from_secs(5),
    )
}

fn report(state: EngineUsageAuthentication, failure: Option<&str>) -> EngineUsageReport {
    EngineUsageReport::new(
        None,
        EngineUsageAuth::new(state, None).expect("auth"),
        "Codex",
        "codex",
        failure.map(str::to_owned),
        Some(QuotaSurface::Supported),
        Vec::new(),
    )
    .expect("report")
}

#[test]
fn only_a_fresh_authenticated_report_is_ready() {
    let authenticated = report(EngineUsageAuthentication::Authenticated, None);
    assert_eq!(
        engine_readiness("Codex", Some(&authenticated), true, None),
        EngineReadiness::ready()
    );
    let stale = engine_readiness("Codex", Some(&authenticated), false, None);
    assert_eq!(stale.verdict(), EngineReadinessVerdict::NotReady);
    assert_eq!(
        stale.reason(),
        Some("Codex account status is out of date; refresh to check it again.")
    );
    // A fresh last-good report stays ready while a refresh failure is
    // served alongside it; a stale one reports the failure.
    let last_good = report(
        EngineUsageAuthentication::Authenticated,
        Some("read timed out"),
    );
    assert!(engine_readiness("Codex", Some(&last_good), true, None).is_ready());
    assert_eq!(
        engine_readiness("Codex", Some(&last_good), false, None).reason(),
        Some("Codex account status is unavailable: read timed out.")
    );
}

#[test]
fn sign_in_failures_and_unobserved_engines_carry_their_reasons() {
    let signed_out = report(EngineUsageAuthentication::Unauthenticated, None);
    let verdict = engine_readiness("Codex", Some(&signed_out), true, None);
    assert_eq!(verdict.verdict(), EngineReadinessVerdict::NeedsSignIn);
    assert_eq!(verdict.reason(), Some("Codex account sign-in is required."));

    let unknown = report(EngineUsageAuthentication::Unknown, None);
    assert_eq!(
        engine_readiness("Codex", Some(&unknown), true, None).reason(),
        Some("Codex account status is unavailable right now.")
    );

    let failed = engine_readiness("Claude", None, false, Some("provider went down"));
    assert_eq!(failed.verdict(), EngineReadinessVerdict::NotReady);
    assert_eq!(
        failed.reason(),
        Some("Claude account status is unavailable: provider went down.")
    );

    let unseen = engine_readiness("Claude", None, false, None);
    assert_eq!(unseen.verdict(), EngineReadinessVerdict::Checking);
    assert_eq!(unseen.reason(), Some("Checking the Claude account status."));
}

#[tokio::test]
async fn served_reports_carry_the_verdict_and_the_service_remembers_it() {
    let usage = service(
        Duration::from_secs(180),
        &[
            ("codex", "Codex", Outcome::Authenticated),
            ("claude", "Claude", Outcome::Unauthenticated),
            ("cursor", "Cursor", Outcome::Failure),
        ],
    );
    assert_eq!(
        usage
            .readiness("codex")
            .map(|readiness| readiness.verdict()),
        Some(EngineReadinessVerdict::Checking),
        "an engine the Forge has not read yet is still being checked"
    );
    assert_eq!(usage.readiness("unrostered"), None);

    let snapshot = usage
        .read(&ReadAccountUsage::new(None, false).expect("query"))
        .await;
    let verdicts = snapshot
        .engines()
        .iter()
        .map(|report| (report.engine_id().to_owned(), report.readiness().verdict()))
        .collect::<Vec<_>>();
    assert_eq!(
        verdicts,
        vec![
            ("codex".to_owned(), EngineReadinessVerdict::Ready),
            ("claude".to_owned(), EngineReadinessVerdict::NeedsSignIn),
            ("cursor".to_owned(), EngineReadinessVerdict::NotReady),
        ]
    );
    assert!(
        usage
            .readiness("codex")
            .is_some_and(|readiness| readiness.is_ready())
    );
    assert_eq!(
        usage
            .readiness("cursor")
            .and_then(|readiness| readiness.reason().map(str::to_owned)),
        Some("Cursor account status is unavailable: provider went down.".to_owned())
    );
}

#[tokio::test]
async fn a_remembered_verdict_expires_with_the_freshness_window() {
    let usage = service(
        Duration::from_millis(1),
        &[("codex", "Codex", Outcome::Authenticated)],
    );
    let snapshot = usage
        .read(&ReadAccountUsage::new(Some("codex".to_owned()), false).expect("query"))
        .await;
    assert!(snapshot.engines()[0].readiness().is_ready());
    tokio::time::sleep(Duration::from_millis(5)).await;
    assert_eq!(
        usage
            .readiness("codex")
            .map(|readiness| readiness.verdict()),
        Some(EngineReadinessVerdict::NotReady)
    );
}

fn catalog() -> NativeModelCatalog {
    let mut catalog = NativeModelCatalog::harnesses_only().expect("harnesses");
    catalog.runnable_harness_ids = ["opencode2", "codex", "claude", "grok", "cursor"]
        .into_iter()
        .map(str::to_owned)
        .collect();
    catalog
}

#[tokio::test]
async fn the_served_catalog_admits_probed_engines_only_when_ready() {
    let usage = service(
        Duration::from_secs(180),
        &[
            ("codex", "Codex", Outcome::Authenticated),
            ("claude", "Claude", Outcome::Unauthenticated),
            ("cursor", "Cursor", Outcome::Failure),
        ],
    );
    // Before any read neither probed engine has proven its account.
    assert_eq!(
        catalog_with_account_readiness(catalog(), Some(&usage)).runnable_harness_ids,
        vec!["opencode2", "grok", "cursor"]
    );
    let _ = usage
        .read(&ReadAccountUsage::new(None, false).expect("query"))
        .await;
    // Codex is ready; signed-out Claude is not; Cursor's dashboard verdict
    // never gates its runtime marking.
    assert_eq!(
        catalog_with_account_readiness(catalog(), Some(&usage)).runnable_harness_ids,
        vec!["opencode2", "codex", "grok", "cursor"]
    );
    assert_eq!(
        catalog_with_account_readiness(catalog(), None).runnable_harness_ids,
        vec!["opencode2", "grok", "cursor"]
    );
}
