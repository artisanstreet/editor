//! External domain coverage for the provider-account usage vocabulary.
//!
//! Proves the query plugs into the shared `Query` enum, the auth and surface
//! states map through their wire strings, and mixed success/failure snapshots
//! compose within their bounds. Pure value coverage; no engine or provider.

use std::str::FromStr;

use artisan_domain::{
    ENGINE_USAGE_ENGINES_MAX, ENGINE_USAGE_WINDOWS_MAX_PER_ENGINE, EngineUsageAuth,
    EngineUsageAuthentication, EngineUsageError, EngineUsageReport, EngineUsageSnapshot,
    EngineUsageWindow, EngineUsageWindowKind, Query, QuotaSurface, ReadAccountUsage,
};

fn report(engine_id: &str, display_name: &str) -> EngineUsageReport {
    EngineUsageReport::new(
        None,
        EngineUsageAuth::new(EngineUsageAuthentication::Authenticated, None)
            .expect("auth is valid"),
        display_name.to_owned(),
        engine_id.to_owned(),
        None,
        Some(QuotaSurface::Supported),
        vec![
            EngineUsageWindow::new(
                "five_hour".to_owned(),
                EngineUsageWindowKind::Session,
                None,
                12.5,
                None,
                Some(300),
            )
            .expect("window is valid"),
        ],
    )
    .expect("report is valid")
}

#[test]
fn usage_query_plugs_into_the_shared_query_enum() {
    let narrowed = ReadAccountUsage::new(Some("cursor".to_owned()), true).expect("query is valid");
    match Query::ReadAccountUsage(narrowed.clone()) {
        Query::ReadAccountUsage(query) => {
            assert_eq!(query.engine_id(), Some("cursor"));
            assert!(query.force());
        }
        _ => panic!("expected readAccountUsage query"),
    }
    let _ = narrowed;
    let broad = ReadAccountUsage::new(None, false).expect("query is valid");
    assert_eq!(broad.engine_id(), None);
    assert!(!broad.force());
}

#[test]
fn auth_and_surface_states_map_through_their_wire_strings() {
    assert_eq!(
        EngineUsageAuthentication::from_str("authenticated"),
        Ok(EngineUsageAuthentication::Authenticated)
    );
    assert_eq!(
        EngineUsageAuthentication::from_str("signed-in"),
        Err(EngineUsageError::UnknownAuthentication {
            value: "signed-in".to_owned()
        })
    );
    assert_eq!(
        QuotaSurface::from_str("unsupported"),
        Ok(QuotaSurface::Unsupported)
    );
    assert_eq!(
        QuotaSurface::from_str("partial"),
        Err(EngineUsageError::UnknownQuotaSurface {
            value: "partial".to_owned()
        })
    );
    assert_eq!(
        EngineUsageWindowKind::from_str("weekly"),
        Ok(EngineUsageWindowKind::Weekly)
    );
    assert_eq!(EngineUsageWindowKind::Weekly.as_str(), "weekly");
    assert_eq!(QuotaSurface::Supported.as_str(), "supported");
    assert_eq!(
        EngineUsageAuthentication::Unauthenticated.as_str(),
        "unauthenticated"
    );
}

#[test]
fn mixed_success_and_failure_reports_compose_one_snapshot() {
    let failed = EngineUsageReport::new(
        None,
        EngineUsageAuth::new(
            EngineUsageAuthentication::Unauthenticated,
            Some("Cursor sign-in is no longer valid.".to_owned()),
        )
        .expect("auth is valid"),
        "Cursor".to_owned(),
        "cursor".to_owned(),
        Some("Cursor sign-in is no longer valid.".to_owned()),
        Some(QuotaSurface::Supported),
        Vec::new(),
    )
    .expect("failure report is valid");
    assert!(failed.windows().is_empty());
    assert_eq!(
        failed.authentication().reason(),
        Some("Cursor sign-in is no longer valid.")
    );
    let snapshot = EngineUsageSnapshot::new(
        vec![report("codex", "Codex"), failed],
        "2026-09-09T12:00:00Z".to_owned(),
    )
    .expect("snapshot is valid");
    assert_eq!(snapshot.engines().len(), 2);
    assert_eq!(snapshot.engines()[0].account_email(), None);
    assert_eq!(snapshot.fetched_at(), "2026-09-09T12:00:00Z");
}

#[test]
fn collection_bounds_match_the_typescript_contract() {
    assert_eq!(ENGINE_USAGE_WINDOWS_MAX_PER_ENGINE, 64);
    assert_eq!(ENGINE_USAGE_ENGINES_MAX, 16);
}

#[test]
fn failure_marking_keeps_windows_and_validates() {
    let marked = report("codex", "Codex")
        .with_failure("refresh failed".to_owned())
        .expect("marking should validate");
    assert_eq!(marked.failure(), Some("refresh failed"));
    assert_eq!(marked.windows().len(), 1);
    assert_eq!(marked.engine_id(), "codex");
    assert!(
        report("codex", "Codex")
            .with_failure(String::new())
            .is_err()
    );
}
