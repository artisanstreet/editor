//! Focused, dependency-free tests for the backend telemetry control policy.

#![forbid(unsafe_code)]
#![allow(dead_code)]

#[path = "../../modules/backend/src/telemetry_preferences_control_policy.rs"]
mod telemetry_preferences_control_policy;

use telemetry_preferences_control_policy::{
    TELEMETRY_PREFERENCES_VERSION, TelemetryPreference, TelemetryPreferences,
    TelemetryPreferencesControlError, TelemetryPreferencesControlOperation,
    TelemetryPreferencesControlPolicy, TelemetryPreferencesControlPort, TelemetryPreferencesUpdate,
    read, read_from, resolve_read_result, resolve_update_result, update, update_from,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PortFailure {
    Unavailable,
}

#[derive(Clone, Copy, Debug)]
struct FixedPort {
    read: Result<TelemetryPreferences, PortFailure>,
    update: Result<TelemetryPreferences, PortFailure>,
}

impl TelemetryPreferencesControlPort for FixedPort {
    type Error = PortFailure;

    fn read(&self) -> Result<TelemetryPreferences, Self::Error> {
        self.read
    }

    fn update(
        &self,
        _patch: TelemetryPreferencesUpdate,
    ) -> Result<TelemetryPreferences, Self::Error> {
        self.update
    }
}

fn expected_preferences(
    crash_reports: Option<TelemetryPreference>,
    usage_analytics: Option<TelemetryPreference>,
) -> TelemetryPreferences {
    TelemetryPreferences::new(
        crash_reports.unwrap_or(TelemetryPreference::Unset),
        usage_analytics.unwrap_or(TelemetryPreference::Unset),
    )
}

#[test]
fn preference_vocabulary_has_all_and_only_the_exact_protocol_spellings() {
    let expected = [
        (TelemetryPreference::Unset, "unset"),
        (TelemetryPreference::Enabled, "enabled"),
        (TelemetryPreference::Disabled, "disabled"),
    ];

    assert_eq!(TelemetryPreference::ALL.len(), expected.len());
    for ((choice, spelling), listed) in expected.into_iter().zip(TelemetryPreference::ALL) {
        assert_eq!(listed, choice);
        assert_eq!(choice.as_str(), spelling);
    }
    assert_eq!(TelemetryPreference::default(), TelemetryPreference::Unset);
}

#[test]
fn default_reads_are_independent_unset_values_with_fixed_version_one() {
    let expected =
        TelemetryPreferences::new(TelemetryPreference::Unset, TelemetryPreference::Unset);

    assert_eq!(
        TelemetryPreferencesControlPolicy::new(),
        TelemetryPreferencesControlPolicy
    );
    assert_eq!(TelemetryPreferencesControlPolicy::read(), expected);
    assert_eq!(read(), expected);
    assert_eq!(expected, TelemetryPreferences::default());
    assert_eq!(expected.version, TELEMETRY_PREFERENCES_VERSION);
    assert_eq!(expected.version, 1);
}

#[test]
fn every_partial_and_full_patch_combination_maps_each_omission_to_unset() {
    let patch_values = [
        None,
        Some(TelemetryPreference::Unset),
        Some(TelemetryPreference::Enabled),
        Some(TelemetryPreference::Disabled),
    ];

    for &crash_reports in &patch_values {
        for &usage_analytics in &patch_values {
            let patch = TelemetryPreferencesUpdate::new(crash_reports, usage_analytics);
            let expected = expected_preferences(crash_reports, usage_analytics);

            assert_eq!(patch.into_preferences(), expected);
            assert_eq!(TelemetryPreferencesControlPolicy::update(patch), expected);
            assert_eq!(update(patch), expected);
            assert_eq!(expected.version, TELEMETRY_PREFERENCES_VERSION);
        }
    }
}

#[test]
fn patch_constructors_cover_each_independent_field_and_empty_patch() {
    assert_eq!(
        TelemetryPreferencesUpdate::for_crash_reports(TelemetryPreference::Enabled),
        TelemetryPreferencesUpdate::new(Some(TelemetryPreference::Enabled), None),
    );
    assert_eq!(
        TelemetryPreferencesUpdate::for_usage_analytics(TelemetryPreference::Disabled),
        TelemetryPreferencesUpdate::new(None, Some(TelemetryPreference::Disabled)),
    );
    assert_eq!(
        TelemetryPreferencesUpdate::for_both(
            TelemetryPreference::Disabled,
            TelemetryPreference::Enabled,
        ),
        TelemetryPreferencesUpdate::new(
            Some(TelemetryPreference::Disabled),
            Some(TelemetryPreference::Enabled),
        ),
    );

    let empty = TelemetryPreferencesUpdate::default();
    assert!(empty.is_empty());
    assert_eq!(update(empty), TelemetryPreferences::initial());
}

#[test]
fn repeated_updates_are_stateless_and_do_not_inherit_another_call() {
    let first = TelemetryPreferencesUpdate::for_both(
        TelemetryPreference::Enabled,
        TelemetryPreference::Disabled,
    );
    let second = TelemetryPreferencesUpdate::for_crash_reports(TelemetryPreference::Disabled);
    let third = TelemetryPreferencesUpdate::for_usage_analytics(TelemetryPreference::Enabled);

    assert_eq!(
        update(first),
        TelemetryPreferences::new(TelemetryPreference::Enabled, TelemetryPreference::Disabled,),
    );
    assert_eq!(
        update(second),
        TelemetryPreferences::new(TelemetryPreference::Disabled, TelemetryPreference::Unset),
    );
    assert_eq!(
        update(third),
        TelemetryPreferences::new(TelemetryPreference::Unset, TelemetryPreference::Enabled),
    );
    assert_eq!(
        TelemetryPreferencesControlPolicy::update(first),
        TelemetryPreferences::new(TelemetryPreference::Enabled, TelemetryPreference::Disabled,),
    );
}

#[test]
fn crash_and_usage_choices_are_independent_for_each_single_field_update() {
    for choice in TelemetryPreference::ALL {
        assert_eq!(
            update(TelemetryPreferencesUpdate::for_crash_reports(choice)),
            TelemetryPreferences::new(choice, TelemetryPreference::Unset),
            "crash_reports={choice:?}",
        );
        assert_eq!(
            update(TelemetryPreferencesUpdate::for_usage_analytics(choice)),
            TelemetryPreferences::new(TelemetryPreference::Unset, choice),
            "usage_analytics={choice:?}",
        );
    }
}

#[test]
fn successful_injected_port_values_are_forwarded_without_reinterpretation() {
    let supplied =
        TelemetryPreferences::new(TelemetryPreference::Disabled, TelemetryPreference::Enabled);
    let port = FixedPort {
        read: Ok(supplied),
        update: Ok(supplied),
    };

    assert_eq!(read_from(&port), Ok(supplied));
    assert_eq!(
        update_from(&port, TelemetryPreferencesUpdate::default()),
        Ok(supplied)
    );
    assert_eq!(
        TelemetryPreferencesControlPolicy::read_from(&port),
        Ok(supplied)
    );
    assert_eq!(
        TelemetryPreferencesControlPolicy::update_from(
            &port,
            TelemetryPreferencesUpdate::for_both(
                TelemetryPreference::Unset,
                TelemetryPreference::Unset,
            ),
        ),
        Ok(supplied),
    );
}

#[test]
fn injected_read_and_update_failures_map_to_exact_operations() {
    let port = FixedPort {
        read: Err(PortFailure::Unavailable),
        update: Err(PortFailure::Unavailable),
    };
    let read_error = read_from(&port).unwrap_err();
    let update_error = update_from(&port, TelemetryPreferencesUpdate::default()).unwrap_err();

    assert_eq!(read_error, TelemetryPreferencesControlError::read());
    assert_eq!(
        read_error.operation,
        TelemetryPreferencesControlOperation::Read
    );
    assert_eq!(read_error.operation.as_str(), "read");
    assert_eq!(update_error, TelemetryPreferencesControlError::update());
    assert_eq!(
        update_error.operation,
        TelemetryPreferencesControlOperation::Update
    );
    assert_eq!(update_error.operation.as_str(), "update");
    assert_eq!(
        read_error.to_string(),
        "telemetry preferences read operation failed"
    );
    assert_eq!(
        update_error.to_string(),
        "telemetry preferences update operation failed"
    );
}

#[test]
fn standalone_sync_result_mappers_preserve_success_and_discard_only_port_errors() {
    let supplied =
        TelemetryPreferences::new(TelemetryPreference::Unset, TelemetryPreference::Disabled);

    assert_eq!(
        resolve_read_result::<PortFailure>(Ok(supplied)),
        Ok(supplied)
    );
    assert_eq!(
        resolve_update_result::<PortFailure>(Ok(supplied)),
        Ok(supplied)
    );
    assert_eq!(
        resolve_read_result::<PortFailure>(Err(PortFailure::Unavailable)),
        Err(TelemetryPreferencesControlError::read())
    );
    assert_eq!(
        resolve_update_result::<PortFailure>(Err(PortFailure::Unavailable)),
        Err(TelemetryPreferencesControlError::update())
    );
}
