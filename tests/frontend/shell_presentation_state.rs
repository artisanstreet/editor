//! Exhaustive, dependency-free coverage for shell presentation state policy.
//!
//! This test deliberately includes the owned production module by path. The
//! normal crate/BUILD registration belongs to the integrating VP and is
//! intentionally outside this worker's ownership.

#[path = "../../modules/frontend/src/shell_presentation_state.rs"]
mod shell_presentation_state;

use shell_presentation_state::{
    DEFAULT_SHELL_PRESENTATION_STATE, LoadTransition, SHELL_PRESENTATION_PREFERENCES_STORAGE_KEY,
    SHELL_PRESENTATION_SCHEMA_VERSION, SaveOutcome, ShellPresentationState, StorageAction,
    StorageReadObservation, StorageWriteObservation, load, repair, save,
};

fn state(left_collapsed: bool, right_collapsed: bool) -> ShellPresentationState {
    ShellPresentationState::new(left_collapsed, right_collapsed)
}

#[test]
fn version_one_schema_and_defaults_are_exact() {
    assert_eq!(SHELL_PRESENTATION_SCHEMA_VERSION, 1);
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
    assert_eq!(state(true, false).version, 1);
    assert!(state(true, false).left_collapsed);
    assert!(!state(true, false).right_collapsed);
}

#[test]
fn storage_key_is_the_exact_legacy_key() {
    assert_eq!(
        SHELL_PRESENTATION_PREFERENCES_STORAGE_KEY,
        "artisan.shell-presentation"
    );
}

#[test]
fn load_exhaustively_maps_missing_valid_malformed_and_read_failure() {
    let stored = state(true, true);
    let repair_action = StorageAction::WriteDefault {
        key: SHELL_PRESENTATION_PREFERENCES_STORAGE_KEY,
        state: DEFAULT_SHELL_PRESENTATION_STATE,
    };
    let cases = [
        (
            StorageReadObservation::Missing,
            LoadTransition {
                state: DEFAULT_SHELL_PRESENTATION_STATE,
                action: StorageAction::None,
            },
        ),
        (
            StorageReadObservation::Valid(stored),
            LoadTransition {
                state: stored,
                action: StorageAction::None,
            },
        ),
        (
            StorageReadObservation::Malformed,
            LoadTransition {
                state: DEFAULT_SHELL_PRESENTATION_STATE,
                action: repair_action,
            },
        ),
        (
            StorageReadObservation::ReadFailure,
            LoadTransition {
                state: DEFAULT_SHELL_PRESENTATION_STATE,
                action: repair_action,
            },
        ),
    ];

    for (observation, expected) in cases {
        assert_eq!(load(observation), expected, "observation={observation:?}");
    }
}

#[test]
fn repair_exhaustively_requests_removal_only_after_failed_repair_write() {
    let read_cases = [
        (
            StorageReadObservation::Missing,
            StorageAction::None,
            StorageAction::None,
            StorageAction::None,
        ),
        (
            StorageReadObservation::Valid(state(false, true)),
            StorageAction::None,
            StorageAction::None,
            StorageAction::None,
        ),
        (
            StorageReadObservation::Malformed,
            StorageAction::WriteDefault {
                key: SHELL_PRESENTATION_PREFERENCES_STORAGE_KEY,
                state: DEFAULT_SHELL_PRESENTATION_STATE,
            },
            StorageAction::None,
            StorageAction::Remove {
                key: SHELL_PRESENTATION_PREFERENCES_STORAGE_KEY,
            },
        ),
        (
            StorageReadObservation::ReadFailure,
            StorageAction::WriteDefault {
                key: SHELL_PRESENTATION_PREFERENCES_STORAGE_KEY,
                state: DEFAULT_SHELL_PRESENTATION_STATE,
            },
            StorageAction::None,
            StorageAction::Remove {
                key: SHELL_PRESENTATION_PREFERENCES_STORAGE_KEY,
            },
        ),
    ];

    for (observation, expected_action, expected_success, expected_failure) in read_cases {
        let transition = load(observation);
        assert_eq!(
            transition.action, expected_action,
            "observation={observation:?}"
        );
        assert_eq!(
            repair(transition, StorageWriteObservation::Succeeded).action,
            expected_success,
            "successful repair for {observation:?}"
        );
        assert_eq!(
            repair(transition, StorageWriteObservation::Failed).action,
            expected_failure,
            "failed repair for {observation:?}"
        );
    }
}

#[test]
fn save_exhaustively_absorbs_both_write_outcomes_for_every_state() {
    let states = [
        state(false, false),
        state(false, true),
        state(true, false),
        state(true, true),
    ];
    let observations = [
        (StorageWriteObservation::Succeeded, SaveOutcome::Succeeded),
        (
            StorageWriteObservation::Failed,
            SaveOutcome::FailureAbsorbed,
        ),
    ];

    for state in states {
        for (observation, expected_outcome) in observations {
            let transition = save(state, observation);
            assert_eq!(
                transition.action,
                StorageAction::Save {
                    key: SHELL_PRESENTATION_PREFERENCES_STORAGE_KEY,
                    state,
                },
                "save action for state={state:?}, observation={observation:?}"
            );
            assert_eq!(
                transition.outcome, expected_outcome,
                "save outcome for state={state:?}, observation={observation:?}"
            );
            if observation == StorageWriteObservation::Failed {
                assert_ne!(
                    transition.action,
                    StorageAction::Remove {
                        key: SHELL_PRESENTATION_PREFERENCES_STORAGE_KEY,
                    }
                );
            }
        }
    }
}
