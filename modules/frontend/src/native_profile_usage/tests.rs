#![expect(
    clippy::float_cmp,
    reason = "test assertions compare the exact pixel arithmetic the UI performs; an epsilon would weaken the regression coverage"
)]
use super::*;

fn window(
    id: &str,
    cadence: NativeUsageCadence,
    label: Option<&str>,
    percent_used: f64,
) -> NativeUsageWindow {
    NativeUsageWindow {
        id: id.to_owned(),
        cadence,
        label: label.map(str::to_owned),
        percent_used,
        resets_at: None,
        window_minutes: None,
    }
}

#[test]
fn groups_follow_cadence_and_put_account_window_first() {
    let groups = group_usage_windows(&[
        window(
            "weekly-model",
            NativeUsageCadence::Weekly,
            Some("Model"),
            25.0,
        ),
        window("monthly", NativeUsageCadence::Monthly, None, 10.0),
        window("weekly-all", NativeUsageCadence::Weekly, None, 20.0),
        window(
            "weekly-model",
            NativeUsageCadence::Weekly,
            Some("duplicate"),
            99.0,
        ),
    ]);

    assert_eq!(
        groups.iter().map(|group| group.cadence).collect::<Vec<_>>(),
        vec![NativeUsageCadence::Weekly, NativeUsageCadence::Monthly]
    );
    assert_eq!(groups[0].windows[0].scope_label(), "All models");
    assert_eq!(groups[0].windows[1].scope_label(), "duplicate");
    assert_eq!(groups[0].windows[1].percent_used, 99.0);
    assert_eq!(groups[0].windows.len(), 2);
}

#[test]
fn invalid_percentages_are_not_painted_as_zero() {
    let groups = group_usage_windows(&[
        window("nan", NativeUsageCadence::Session, None, f64::NAN),
        window("too-high", NativeUsageCadence::Session, None, 101.0),
    ]);
    assert!(groups.is_empty());
}

fn report(
    engine_id: &str,
    authentication: NativeUsageAuthentication,
    windows: Vec<NativeUsageWindow>,
) -> NativeUsageReport {
    NativeUsageReport {
        engine_id: engine_id.to_owned(),
        display_name: profile_usage_display_name(engine_id).to_owned(),
        authentication,
        account_email: None,
        quota_surface: NativeUsageQuotaSurface::Supported,
        windows,
        failure: None,
        readiness: EngineReadiness::ready(),
    }
}

fn presented_entry(
    engine_id: &str,
    report: Option<NativeUsageReport>,
    failure: Option<&str>,
) -> NativeUsageEntry {
    NativeUsageEntry {
        engine_id: engine_id.to_owned(),
        display_name: profile_usage_display_name(engine_id).to_owned(),
        report,
        failure: failure.map(str::to_owned),
        fetched_at_ms: Some(1_000_000),
    }
}

fn visible_ids(entries: &[NativeUsageEntry], refreshing: &[&str]) -> Vec<String> {
    NativeProfileUsageState {
        entries: entries.to_vec(),
        refreshing_engine_ids: refreshing.iter().map(|id| (*id).to_owned()).collect(),
        pending_read_seq: Vec::new(),
    }
    .visible_usage_entries()
    .iter()
    .map(|entry| entry.engine_id.clone())
    .collect()
}

#[test]
fn dropdown_hides_providers_without_renderable_data() {
    let zero = presented_entry(
        "zero",
        Some(report(
            "zero",
            NativeUsageAuthentication::Authenticated,
            vec![window("session", NativeUsageCadence::Session, None, 0.0)],
        )),
        None,
    );
    let empty = presented_entry(
        "empty",
        Some(report(
            "empty",
            NativeUsageAuthentication::Authenticated,
            Vec::new(),
        )),
        None,
    );
    let invalid = presented_entry(
        "invalid",
        Some(report(
            "invalid",
            NativeUsageAuthentication::Authenticated,
            vec![
                window("nan", NativeUsageCadence::Session, None, f64::NAN),
                window("high", NativeUsageCadence::Session, None, 120.0),
            ],
        )),
        None,
    );
    let unauthenticated = presented_entry(
        "unauthenticated",
        Some(report(
            "unauthenticated",
            NativeUsageAuthentication::Unauthenticated,
            vec![window("session", NativeUsageCadence::Session, None, 40.0)],
        )),
        None,
    );
    let failed = presented_entry("failed", None, Some("transport"));
    let pending = NativeUsageEntry::pending("pending", "Pending");
    // A zero percentage is real data and stays visible; everything
    // without a renderable authenticated window is hidden.
    assert_eq!(
        visible_ids(
            &[zero, empty, invalid, unauthenticated, failed, pending],
            &[]
        ),
        vec!["zero"]
    );
}

#[test]
fn dropdown_keeps_last_good_windows_while_refreshing_or_failed() {
    let windows = vec![window("session", NativeUsageCadence::Session, None, 62.0)];
    let refreshing = presented_entry(
        "refreshing",
        Some(report(
            "refreshing",
            NativeUsageAuthentication::Authenticated,
            windows.clone(),
        )),
        None,
    );
    let mut failed = report("failed", NativeUsageAuthentication::Authenticated, windows);
    failed.failure = Some("stale".to_owned());
    let failed = presented_entry("failed", Some(failed), Some("transport"));
    assert_eq!(
        visible_ids(&[refreshing, failed], &["refreshing"]),
        vec!["refreshing", "failed"]
    );
}

#[test]
fn reset_duration_requires_every_window_to_have_a_future_reset() {
    let mut first = window("first", NativeUsageCadence::Session, None, 25.0);
    first.resets_at = Some("2030-01-01T00:00:00Z".to_owned());
    let mut second = window("second", NativeUsageCadence::Session, Some("model"), 50.0);
    second.resets_at = Some("2030-01-01T00:30:00Z".to_owned());

    assert_eq!(
        reset_duration(&[first.clone(), second.clone()], 1_893_454_200_000),
        Some("1 hour".to_owned())
    );
    second.resets_at = None;
    assert_eq!(reset_duration(&[first, second], 1_893_454_200_000), None);
}

#[test]
fn state_keeps_newer_provider_reading_and_separate_refresh_state() {
    let mut state = NativeProfileUsageState {
        entries: vec![NativeUsageEntry {
            engine_id: "claude".to_owned(),
            display_name: "Claude".to_owned(),
            report: None,
            failure: Some("temporary".to_owned()),
            fetched_at_ms: Some(20),
        }],
        refreshing_engine_ids: vec!["claude".to_owned()],
        pending_read_seq: vec![("claude".to_owned(), 4)],
    };
    state.accept(NativeUsageEntry {
        engine_id: "claude".to_owned(),
        display_name: "Claude".to_owned(),
        report: None,
        failure: Some("stale".to_owned()),
        fetched_at_ms: Some(10),
    });
    assert_eq!(
        state.entry("claude").and_then(|entry| entry.fetched_at_ms),
        Some(20)
    );
    assert!(state.refreshing_engine_ids.is_empty());

    state.finish_refresh("claude");
    assert!(state.refreshing_engine_ids.is_empty());
}

#[test]
fn checked_labels_use_each_provider_timestamp() {
    assert_eq!(checked_label(None, 10_000), None);
    assert_eq!(
        checked_label(Some(10_000), 10_000),
        Some("last checked now".to_owned())
    );
    assert_eq!(
        checked_label(Some(0), 3_600_000),
        Some("last checked 1 hr ago".to_owned())
    );
}

#[test]
fn remaining_percent_rounds_like_the_tooltip() {
    assert_eq!(usage_remaining_percent(62.4), 38);
    assert_eq!(usage_remaining_percent(0.0), 100);
    assert_eq!(usage_remaining_percent(100.0), 0);
    assert_eq!(usage_remaining_percent(120.0), 0);
    assert_eq!(usage_remaining_percent(f64::NAN), 0);
}

#[test]
fn tip_run_up_starts_just_short_of_its_target() {
    assert_eq!(tip_run_up_from(38.0), 35.0);
    assert_eq!(tip_run_up_from(100.0), 92.0);
    assert_eq!(tip_run_up_from(0.0), 0.0);
}

fn entry_with_time(engine_id: &str, fetched_at_ms: Option<i64>) -> NativeUsageEntry {
    NativeUsageEntry {
        engine_id: engine_id.to_owned(),
        display_name: profile_usage_display_name(engine_id).to_owned(),
        report: None,
        failure: Some("transport".to_owned()),
        fetched_at_ms,
    }
}

#[test]
fn opening_dispatches_missing_rows_and_freshness_avoids_repeats() {
    let mut state = NativeProfileUsageState::default();
    let now_ms = 1_000_000;
    let first = plan_profile_usage_loads(&state, now_ms, false, None);
    assert_eq!(first.len(), PROFILE_USAGE_ROSTER.len());

    for engine_id in &first {
        state.entries.push(entry_with_time(engine_id, Some(now_ms)));
    }
    assert!(plan_profile_usage_loads(&state, now_ms, false, None).is_empty());

    let stale_ms = now_ms - 181_000;
    state.entries[0].fetched_at_ms = Some(stale_ms);
    assert_eq!(
        plan_profile_usage_loads(&state, now_ms, false, None),
        vec![state.entries[0].engine_id.clone()]
    );
}

#[test]
fn forced_refresh_loads_everything_and_keeps_menu_scope() {
    let mut state = NativeProfileUsageState::default();
    let now_ms = 2_000_000;
    for (engine_id, _) in PROFILE_USAGE_ROSTER {
        state.entries.push(entry_with_time(engine_id, Some(now_ms)));
    }
    let forced = plan_profile_usage_loads(&state, now_ms, true, None);
    assert_eq!(forced.len(), PROFILE_USAGE_ROSTER.len());

    let single = plan_profile_usage_loads(&state, now_ms, true, Some("claude"));
    assert_eq!(single, vec!["claude".to_owned()]);
}

#[test]
fn inflight_reads_deduplicate_until_forced() {
    let mut state = NativeProfileUsageState::default();
    state.begin_refresh("codex");
    let now_ms = 3_000_000;
    assert!(plan_profile_usage_loads(&state, now_ms, false, Some("codex")).is_empty());
    assert_eq!(
        plan_profile_usage_loads(&state, now_ms, true, Some("codex")),
        vec!["codex".to_owned()]
    );
}

#[test]
fn response_pairing_rejects_wrong_generation_engine_or_sequence() {
    let mut state = NativeProfileUsageState::default();
    state.begin_refresh_seq("codex", 11);
    let current = ProfileUsageGeneration::first();
    let next = current.checked_next().expect("next generation");
    assert!(account_usage_response_current(
        &state, current, current, "codex", 11
    ));
    assert!(!account_usage_response_current(
        &state, current, next, "codex", 11
    ));
    assert!(!account_usage_response_current(
        &state, current, current, "claude", 11
    ));
    assert!(!account_usage_response_current(
        &state,
        current,
        current,
        "unknown-engine",
        11
    ));
    assert!(!account_usage_response_current(
        &state, current, current, "codex", 10
    ));
}

#[test]
fn old_same_engine_reply_after_force_cannot_settle_or_replace() {
    let mut state = NativeProfileUsageState::default();
    state.begin_refresh_seq("codex", 1);
    // A forced refresh while the first read is pending supersedes it.
    state.begin_refresh_seq("codex", 2);
    let current = ProfileUsageGeneration::first();
    assert!(!account_usage_response_current(
        &state, current, current, "codex", 1
    ));
    assert!(account_usage_response_current(
        &state, current, current, "codex", 2
    ));

    // The older reply is dropped: pending stays armed and no row appears.
    assert!(!state.try_accept(
        NativeUsageEntry {
            engine_id: "codex".to_owned(),
            display_name: "Codex".to_owned(),
            report: None,
            failure: Some("old".to_owned()),
            fetched_at_ms: Some(10),
        },
        1
    ));
    assert_eq!(state.pending_seq("codex"), Some(2));
    assert!(state.entry("codex").is_none());

    // A stale failure is dropped the same way.
    assert!(!state.try_accept_failure("codex", "Codex", "old".to_owned(), Some(11), 1));
    assert_eq!(state.pending_seq("codex"), Some(2));

    // The newer reply settles and replaces.
    assert!(state.try_accept(
        NativeUsageEntry {
            engine_id: "codex".to_owned(),
            display_name: "Codex".to_owned(),
            report: None,
            failure: Some("new".to_owned()),
            fetched_at_ms: Some(20),
        },
        2
    ));
    assert_eq!(
        state.entry("codex").and_then(|entry| entry.fetched_at_ms),
        Some(20)
    );
    assert!(state.pending_seq("codex").is_none());
}

#[test]
fn one_failure_preserves_siblings_and_stale_cannot_clear_refresh() {
    let mut state = NativeProfileUsageState::default();
    state.accept(NativeUsageEntry {
        engine_id: "codex".to_owned(),
        display_name: "Codex".to_owned(),
        report: Some(NativeUsageReport {
            engine_id: "codex".to_owned(),
            display_name: "Codex".to_owned(),
            authentication: NativeUsageAuthentication::Authenticated,
            account_email: None,
            quota_surface: NativeUsageQuotaSurface::Supported,
            windows: Vec::new(),
            failure: None,
            readiness: EngineReadiness::ready(),
        }),
        failure: None,
        fetched_at_ms: Some(100),
    });
    state.begin_refresh("codex");
    state.begin_refresh("claude");

    state.accept_failure("claude", "Claude", "busy".to_owned(), Some(50));
    assert!(
        state
            .entry("codex")
            .is_some_and(|entry| entry.report.is_some())
    );
    assert!(
        state
            .entry("claude")
            .is_some_and(super::NativeUsageEntry::has_response)
    );

    state.accept(NativeUsageEntry {
        engine_id: "codex".to_owned(),
        display_name: "Codex".to_owned(),
        report: None,
        failure: Some("stale".to_owned()),
        fetched_at_ms: Some(10),
    });
    assert_eq!(
        state.entry("codex").and_then(|entry| entry.fetched_at_ms),
        Some(100)
    );
}

#[test]
fn refresh_failure_keeps_last_good_meters_and_stays_visible() {
    let mut state = NativeProfileUsageState::default();
    state.accept(NativeUsageEntry {
        engine_id: "cursor".to_owned(),
        display_name: "Cursor".to_owned(),
        report: Some(NativeUsageReport {
            engine_id: "cursor".to_owned(),
            display_name: "Cursor".to_owned(),
            authentication: NativeUsageAuthentication::Authenticated,
            account_email: None,
            quota_surface: NativeUsageQuotaSurface::Supported,
            windows: vec![window("five_hour", NativeUsageCadence::Session, None, 42.0)],
            failure: None,
            readiness: EngineReadiness::ready(),
        }),
        failure: None,
        fetched_at_ms: Some(77),
    });
    state.begin_refresh_seq("cursor", 9);
    state.accept_failure("cursor", "Cursor", "peer".to_owned(), Some(78));
    let entry = state.entry("cursor").expect("cursor row");
    // Meters stay: the last-good report and its original observation time
    // are preserved, while the refresh failure is stored alongside.
    assert_eq!(
        entry.report.as_ref().map(|report| report.windows.len()),
        Some(1)
    );
    assert_eq!(entry.failure.as_deref(), Some("peer"));
    assert_eq!(entry.fetched_at_ms, Some(77));
    assert!(!state.refreshing_engine_ids.contains(&"cursor".to_owned()));
}

#[test]
fn provider_failure_with_windows_keeps_both_visible() {
    let report = NativeUsageReport {
        engine_id: "codex".to_owned(),
        display_name: "Codex".to_owned(),
        authentication: NativeUsageAuthentication::Authenticated,
        account_email: None,
        quota_surface: NativeUsageQuotaSurface::Supported,
        windows: vec![window("seven_day", NativeUsageCadence::Weekly, None, 10.0)],
        failure: Some("partial read".to_owned()),
        readiness: EngineReadiness::ready(),
    };
    assert_eq!(report.renderable_windows().len(), 1);
    assert_eq!(report.failure.as_deref(), Some("partial read"));
}

#[test]
fn narrowed_pairing_accepts_exactly_one_matching_report() {
    assert_eq!(narrowed_report_position(&["codex"], "codex"), Some(0));
    assert_eq!(narrowed_report_position(&[], "codex"), None);
    assert_eq!(narrowed_report_position(&["claude"], "codex"), None);
    assert_eq!(
        narrowed_report_position(&["codex", "claude"], "codex"),
        None
    );
    assert_eq!(
        narrowed_report_position(&["unknown-engine"], "unknown-engine"),
        Some(0)
    );
}

#[test]
fn disconnect_clears_incompatible_cache_and_pending() {
    let mut state = NativeProfileUsageState::default();
    state.entries.push(entry_with_time("codex", Some(10)));
    state.begin_refresh_seq("claude", 3);
    state.clear_for_connection();
    assert!(state.entries.is_empty());
    assert!(state.refreshing_engine_ids.is_empty());
    assert_eq!(state.pending_seq("claude"), None);
}

fn readiness_entry(
    engine_id: &str,
    readiness: EngineReadiness,
    failure: Option<&str>,
) -> NativeUsageEntry {
    let mut entry = NativeUsageEntry::pending(engine_id, engine_id);
    entry.report = Some(NativeUsageReport {
        engine_id: engine_id.to_owned(),
        display_name: profile_usage_display_name(engine_id).to_owned(),
        authentication: NativeUsageAuthentication::Authenticated,
        account_email: None,
        quota_surface: NativeUsageQuotaSurface::Supported,
        windows: Vec::new(),
        failure: failure.map(str::to_owned),
        readiness,
    });
    // An old observation: the Editor never re-judges the Forge's verdict
    // against its own clock.
    entry.fetched_at_ms = Some(1);
    entry
}

#[test]
fn readiness_renders_the_forge_verdict_as_delivered() {
    let mut state = NativeProfileUsageState::default();
    // No report yet: nothing claims the engine can run.
    assert_eq!(
        engine_readiness(&state, "codex"),
        EngineReadinessVerdict::NotReady
    );
    assert_eq!(engine_readiness_reason(&state, "codex"), None);
    // A read in flight without a report is being checked.
    state.begin_refresh_seq("codex", 1);
    assert_eq!(
        engine_readiness(&state, "codex"),
        EngineReadinessVerdict::Checking
    );

    state.accept(readiness_entry("codex", EngineReadiness::ready(), None));
    assert_eq!(
        engine_readiness(&state, "codex"),
        EngineReadinessVerdict::Ready
    );

    let sign_in = EngineReadiness::new(
        EngineReadinessVerdict::NeedsSignIn,
        Some("Claude account sign-in is required.".to_owned()),
    )
    .expect("verdict");
    state.accept(readiness_entry("claude", sign_in, None));
    assert_eq!(
        engine_readiness(&state, "claude"),
        EngineReadinessVerdict::NeedsSignIn
    );
    assert_eq!(
        engine_readiness_reason(&state, "claude").as_deref(),
        Some("Claude account sign-in is required.")
    );

    // A later report's verdict replaces the earlier one, including while
    // its engine is being refreshed again.
    state.begin_refresh_seq("codex", 2);
    let not_ready = EngineReadiness::new(
        EngineReadinessVerdict::NotReady,
        Some("Codex account status is unavailable: read timed out.".to_owned()),
    )
    .expect("verdict");
    state.accept(readiness_entry("codex", not_ready, Some("read timed out")));
    assert_eq!(
        engine_readiness(&state, "codex"),
        EngineReadinessVerdict::NotReady
    );
}

#[test]
fn refresh_failures_stay_visible_beside_the_forge_verdict() {
    let mut state = NativeProfileUsageState::default();
    // The Forge serves last-good meters with the refresh failure and keeps
    // judging the account ready; the failure stays actionable.
    state.accept(readiness_entry(
        "codex",
        EngineReadiness::ready(),
        Some("provider usage read timed out"),
    ));
    assert_eq!(
        engine_readiness(&state, "codex"),
        EngineReadinessVerdict::Ready
    );
    assert_eq!(
        engine_refresh_failure(&state, "codex").as_deref(),
        Some("provider usage read timed out")
    );
    assert_eq!(engine_refresh_failure(&state, "claude"), None);
}
