//! Focused, dependency-free coverage for shell presentation persistence.
//!
//! The production policy lives in `shell_presentation_state` (reconciled here
//! instead of duplicating the state machine under a second module name), so
//! this harness includes that module directly and does not need registration
//! in the VP-owned frontend library or build files beyond the test target.

#![forbid(unsafe_code)]

#[path = "../../modules/frontend/src/shell_presentation_state.rs"]
mod shell_presentation_state;

use shell_presentation_state::{
    DEFAULT_SHELL_PRESENTATION_STATE, LoadTransition, SHELL_PRESENTATION_PREFERENCES_STORAGE_KEY,
    SHELL_PRESENTATION_SCHEMA_VERSION, SaveOutcome, ShellPresentationDecodeError,
    ShellPresentationState, StorageAction, StorageReadObservation, StorageWriteObservation,
    classify_serialized_shell_presentation, decode_shell_presentation_state, load, repair, save,
};

fn state(left_collapsed: bool, right_collapsed: bool) -> ShellPresentationState {
    ShellPresentationState::new(left_collapsed, right_collapsed)
}

#[test]
fn schema_key_and_default_state_are_exact() {
    assert_eq!(SHELL_PRESENTATION_SCHEMA_VERSION, 1);
    assert_eq!(
        SHELL_PRESENTATION_PREFERENCES_STORAGE_KEY,
        "artisan.shell-presentation"
    );
    assert_eq!(
        DEFAULT_SHELL_PRESENTATION_STATE,
        ShellPresentationState {
            version: 1,
            left_collapsed: false,
            right_collapsed: false,
        }
    );
    assert_eq!(
        ShellPresentationState::default(),
        DEFAULT_SHELL_PRESENTATION_STATE
    );
}

#[test]
fn every_version_one_state_has_deterministic_valid_round_trip() {
    for left_collapsed in [false, true] {
        for right_collapsed in [false, true] {
            let expected = state(left_collapsed, right_collapsed);
            let encoded = expected.to_json();
            let expected_encoding = format!(
                "{{\"version\":1,\"left_collapsed\":{left_collapsed},\"right_collapsed\":{right_collapsed}}}"
            );

            assert_eq!(encoded, expected_encoding);
            assert_eq!(expected.serialize(), encoded);
            assert_eq!(ShellPresentationState::from_json(&encoded), Ok(expected));
            assert_eq!(ShellPresentationState::deserialize(&encoded), Ok(expected));
            assert_eq!(decode_shell_presentation_state(&encoded), Ok(expected));
            assert_eq!(
                classify_serialized_shell_presentation(&encoded),
                StorageReadObservation::Valid(expected)
            );
        }
    }

    // JSON object order and insignificant whitespace do not change the value.
    let reordered = r#" { "right_collapsed" : true, "version" : 1, "left_collapsed" : false } "#;
    assert_eq!(
        decode_shell_presentation_state(reordered),
        Ok(state(false, true))
    );
}

#[test]
fn missing_load_returns_default_without_repair() {
    let transition = load(StorageReadObservation::Missing);

    assert_eq!(
        transition,
        LoadTransition {
            state: DEFAULT_SHELL_PRESENTATION_STATE,
            action: StorageAction::None,
        }
    );
    assert_eq!(transition.state, DEFAULT_SHELL_PRESENTATION_STATE);
    assert_eq!(transition.action, StorageAction::None);
}

#[test]
fn malformed_and_unknown_version_values_request_default_repair() {
    let malformed_values = [
        ("not json", ShellPresentationDecodeError::InvalidJson),
        (
            r#"{"version":1,"left_collapsed":"false","right_collapsed":false}"#,
            ShellPresentationDecodeError::InvalidLeftCollapsed,
        ),
        (
            r#"{"version":1,"left_collapsed":false,"right_collapsed":0}"#,
            ShellPresentationDecodeError::InvalidRightCollapsed,
        ),
        (
            r#"{"version":1,"left_collapsed":false}"#,
            ShellPresentationDecodeError::MissingRightCollapsed,
        ),
    ];

    for (value, expected_error) in malformed_values {
        assert_eq!(
            decode_shell_presentation_state(value),
            Err(expected_error),
            "{value}"
        );
        assert_eq!(
            classify_serialized_shell_presentation(value),
            StorageReadObservation::Malformed,
            "{value}"
        );
        assert_eq!(
            load(StorageReadObservation::from_serialized(value)).action,
            StorageAction::WriteDefault {
                key: SHELL_PRESENTATION_PREFERENCES_STORAGE_KEY,
                state: DEFAULT_SHELL_PRESENTATION_STATE,
            },
            "{value}"
        );
    }

    for version in [0, 2, u64::from(u8::MAX) + 1] {
        let value =
            format!("{{\"version\":{version},\"left_collapsed\":false,\"right_collapsed\":false}}");
        assert_eq!(
            decode_shell_presentation_state(&value),
            Err(ShellPresentationDecodeError::UnsupportedVersion(version))
        );
        assert_eq!(
            StorageReadObservation::from_serialized(&value),
            StorageReadObservation::Malformed
        );
    }

    let manually_unsupported =
        StorageReadObservation::Valid(ShellPresentationState::with_version(2, true, true));
    assert_eq!(
        load(manually_unsupported).action,
        StorageAction::WriteDefault {
            key: SHELL_PRESENTATION_PREFERENCES_STORAGE_KEY,
            state: DEFAULT_SHELL_PRESENTATION_STATE,
        }
    );

    assert_eq!(
        load(StorageReadObservation::ReadFailure).action,
        StorageAction::WriteDefault {
            key: SHELL_PRESENTATION_PREFERENCES_STORAGE_KEY,
            state: DEFAULT_SHELL_PRESENTATION_STATE,
        }
    );
}

#[test]
fn failed_repair_requests_removal_only_for_malformed_reads() {
    let malformed = load(StorageReadObservation::Malformed);
    let expected_repair = StorageAction::WriteDefault {
        key: SHELL_PRESENTATION_PREFERENCES_STORAGE_KEY,
        state: DEFAULT_SHELL_PRESENTATION_STATE,
    };

    assert_eq!(malformed.action, expected_repair);
    assert_eq!(
        repair(malformed, StorageWriteObservation::Succeeded).action,
        StorageAction::None
    );
    assert_eq!(
        repair(malformed, StorageWriteObservation::Failed).action,
        StorageAction::Remove {
            key: SHELL_PRESENTATION_PREFERENCES_STORAGE_KEY,
        }
    );

    for observation in [
        StorageReadObservation::Malformed,
        StorageReadObservation::ReadFailure,
    ] {
        let transition = load(observation);
        assert_eq!(transition.action, expected_repair);
        assert_eq!(
            repair(transition, StorageWriteObservation::Succeeded).action,
            StorageAction::None
        );
        assert_eq!(
            repair(transition, StorageWriteObservation::Failed).action,
            StorageAction::Remove {
                key: SHELL_PRESENTATION_PREFERENCES_STORAGE_KEY,
            }
        );
    }

    for observation in [
        StorageReadObservation::Missing,
        StorageReadObservation::Valid(state(true, false)),
    ] {
        let transition = load(observation);
        assert_eq!(
            repair(transition, StorageWriteObservation::Failed).action,
            StorageAction::None
        );
    }
}

#[test]
fn save_always_emits_the_supplied_state_and_absorbs_failure() {
    for expected_state in [
        state(false, false),
        state(false, true),
        state(true, false),
        state(true, true),
    ] {
        for observation in [
            (StorageWriteObservation::Succeeded, SaveOutcome::Succeeded),
            (
                StorageWriteObservation::Failed,
                SaveOutcome::FailureAbsorbed,
            ),
        ] {
            let transition = save(expected_state, observation.0);
            assert_eq!(
                transition.action,
                StorageAction::Save {
                    key: SHELL_PRESENTATION_PREFERENCES_STORAGE_KEY,
                    state: expected_state,
                }
            );
            assert_eq!(transition.outcome, observation.1);
        }
    }
}
