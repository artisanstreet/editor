//! Focused, dependency-free coverage for the telemetry-preference policy.
//!
//! The production module is included directly so this harness does not need
//! the VP-owned frontend `lib.rs` or any shared build registration.

#[path = "../../modules/frontend/src/telemetry_preferences.rs"]
mod telemetry_preferences;

use telemetry_preferences::{
    TELEMETRY_PREFERENCES_VERSION, TelemetryPreference, TelemetryPreferences,
    TelemetryPreferencesUpdate, TelemetryPreferencesVersion, apply_telemetry_preferences_update,
    capture_telemetry_intent_fallback, resolve_get_telemetry_preferences,
    resolve_update_telemetry_preferences,
};

#[test]
fn preference_choice_is_closed_and_uses_exact_literals() {
    assert_eq!(TelemetryPreference::ALL.len(), 3);
    assert_eq!(TelemetryPreference::Unset.as_str(), "unset");
    assert_eq!(TelemetryPreference::Enabled.as_str(), "enabled");
    assert_eq!(TelemetryPreference::Disabled.as_str(), "disabled");
    assert_eq!(TelemetryPreference::default(), TelemetryPreference::Unset);
    assert_ne!(TelemetryPreference::Unset, TelemetryPreference::Enabled);
    assert_ne!(TelemetryPreference::Enabled, TelemetryPreference::Disabled);
    assert_ne!(TelemetryPreference::Disabled, TelemetryPreference::Unset);
}

#[test]
fn initial_preferences_are_version_one_with_both_choices_unset() {
    let initial = TelemetryPreferences::initial();

    assert_eq!(initial.version, TELEMETRY_PREFERENCES_VERSION);
    assert_eq!(initial.version, 1);
    assert_eq!(initial.crash_reports, TelemetryPreference::Unset);
    assert_eq!(initial.usage_analytics, TelemetryPreference::Unset);
    assert!(initial.has_supported_version());
    assert_eq!(initial, TelemetryPreferences::default());
    assert_eq!(
        initial,
        TelemetryPreferences::new(TelemetryPreference::Unset, TelemetryPreference::Unset,)
    );
}

#[test]
fn every_one_field_update_replaces_only_its_named_choice() {
    let current =
        TelemetryPreferences::new(TelemetryPreference::Enabled, TelemetryPreference::Disabled);

    for choice in TelemetryPreference::ALL {
        let crash_update = TelemetryPreferencesUpdate::for_crash_reports(choice);
        assert_eq!(
            apply_telemetry_preferences_update(current, crash_update),
            TelemetryPreferences::new(choice, TelemetryPreference::Disabled),
            "crash_reports={choice:?}",
        );

        let usage_update = TelemetryPreferencesUpdate::for_usage_analytics(choice);
        assert_eq!(
            apply_telemetry_preferences_update(current, usage_update),
            TelemetryPreferences::new(TelemetryPreference::Enabled, choice),
            "usage_analytics={choice:?}",
        );
    }
}

#[test]
fn every_two_field_update_replaces_both_choices_independently() {
    let current = TelemetryPreferences::new(TelemetryPreference::Unset, TelemetryPreference::Unset);

    for crash_reports in TelemetryPreference::ALL {
        for usage_analytics in TelemetryPreference::ALL {
            let update = TelemetryPreferencesUpdate::for_both(crash_reports, usage_analytics);
            assert_eq!(
                current.apply_update(update),
                TelemetryPreferences::new(crash_reports, usage_analytics),
                "crash_reports={crash_reports:?}, usage_analytics={usage_analytics:?}",
            );
        }
    }
}

#[test]
fn omitted_fields_are_preserved_and_the_current_version_is_not_reset() {
    let current = TelemetryPreferences::with_version(
        TELEMETRY_PREFERENCES_VERSION,
        TelemetryPreference::Disabled,
        TelemetryPreference::Enabled,
    );

    let crash_only = current.apply_update(TelemetryPreferencesUpdate::new(
        Some(TelemetryPreference::Unset),
        None,
    ));
    assert_eq!(crash_only.crash_reports, TelemetryPreference::Unset);
    assert_eq!(crash_only.usage_analytics, TelemetryPreference::Enabled);
    assert_eq!(crash_only.version, current.version);

    let usage_only = current.apply_update(TelemetryPreferencesUpdate::new(
        None,
        Some(TelemetryPreference::Disabled),
    ));
    assert_eq!(usage_only.crash_reports, TelemetryPreference::Disabled);
    assert_eq!(usage_only.usage_analytics, TelemetryPreference::Disabled);
    assert_eq!(usage_only.version, current.version);

    let empty = TelemetryPreferencesUpdate::default();
    assert!(empty.is_empty());
    assert_eq!(current.apply_update(empty), current);
}

#[test]
fn repeated_updates_are_idempotent_for_each_choice() {
    let current =
        TelemetryPreferences::new(TelemetryPreference::Enabled, TelemetryPreference::Disabled);

    for update in [
        TelemetryPreferencesUpdate::for_crash_reports(TelemetryPreference::Disabled),
        TelemetryPreferencesUpdate::for_usage_analytics(TelemetryPreference::Enabled),
        TelemetryPreferencesUpdate::for_both(
            TelemetryPreference::Unset,
            TelemetryPreference::Enabled,
        ),
    ] {
        let once = current.apply_update(update);
        assert_eq!(once.apply_update(update), once);
        assert_eq!(
            apply_telemetry_preferences_update(once, update),
            apply_telemetry_preferences_update(current, update).apply_update(update),
        );
    }
}

#[test]
fn absent_get_falls_back_to_initial_and_present_get_replaces_exactly() {
    let initial = TelemetryPreferences::initial();
    assert_eq!(resolve_get_telemetry_preferences(None), initial);

    let remote =
        TelemetryPreferences::new(TelemetryPreference::Disabled, TelemetryPreference::Enabled);
    assert_eq!(resolve_get_telemetry_preferences(Some(remote)), remote);
}

#[test]
fn absent_update_applies_patch_while_present_update_replaces_exactly() {
    let current =
        TelemetryPreferences::new(TelemetryPreference::Enabled, TelemetryPreference::Disabled);
    let update = TelemetryPreferencesUpdate::for_crash_reports(TelemetryPreference::Unset);

    assert_eq!(
        resolve_update_telemetry_preferences(current, update, None),
        TelemetryPreferences::new(TelemetryPreference::Unset, TelemetryPreference::Disabled),
    );

    let remote = TelemetryPreferences::new(TelemetryPreference::Unset, TelemetryPreference::Unset);
    assert_eq!(
        resolve_update_telemetry_preferences(current, update, Some(remote)),
        remote,
    );
}

#[test]
fn remote_replacement_wins_even_after_local_updates_and_repeated_resolution() {
    let initial = TelemetryPreferences::initial();
    let local = resolve_update_telemetry_preferences(
        initial,
        TelemetryPreferencesUpdate::for_both(
            TelemetryPreference::Enabled,
            TelemetryPreference::Disabled,
        ),
        None,
    );
    let remote =
        TelemetryPreferences::new(TelemetryPreference::Disabled, TelemetryPreference::Enabled);

    let replaced = resolve_update_telemetry_preferences(
        local,
        TelemetryPreferencesUpdate::default(),
        Some(remote),
    );
    assert_eq!(replaced, remote);
    assert_eq!(
        resolve_update_telemetry_preferences(
            replaced,
            TelemetryPreferencesUpdate::for_crash_reports(TelemetryPreference::Enabled),
            Some(remote),
        ),
        remote,
    );
    assert_eq!(resolve_get_telemetry_preferences(Some(remote)), remote);
}

#[test]
fn future_versions_are_classified_without_coercion_and_remain_exact_when_present() {
    let future = TelemetryPreferences::with_version(
        2,
        TelemetryPreference::Enabled,
        TelemetryPreference::Disabled,
    );

    assert_eq!(
        future.version_status(),
        TelemetryPreferencesVersion::Unsupported(2)
    );
    assert!(!future.has_supported_version());
    assert_eq!(future.version_status().raw(), 2);
    assert_eq!(
        resolve_get_telemetry_preferences(Some(future)),
        future,
        "a present decoded result is never silently rewritten by this policy",
    );
    let updated = future.apply_update(TelemetryPreferencesUpdate::for_crash_reports(
        TelemetryPreference::Unset,
    ));
    assert_eq!(updated.version, 2);
    assert_eq!(
        updated.version_status(),
        TelemetryPreferencesVersion::Unsupported(2)
    );
}

#[test]
fn absent_capture_is_a_successful_no_op() {
    capture_telemetry_intent_fallback();
    capture_telemetry_intent_fallback();
}
