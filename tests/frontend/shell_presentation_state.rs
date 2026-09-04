//! Exhaustive, dependency-free coverage for shell presentation state policy.
//!
//! This test deliberately includes the owned production module by path. The
//! normal crate/BUILD registration belongs to the integrating VP and is
//! intentionally outside this worker's ownership.

#[path = "../../modules/frontend/src/shell_presentation_state.rs"]
mod shell_presentation_state;

use shell_presentation_state::{
    DEFAULT_SHELL_PRESENTATION_STATE, InMemoryShellPresentationStore, LoadTransition,
    SHELL_PRESENTATION_PREFERENCES_STORAGE_KEY, SHELL_PRESENTATION_SCHEMA_VERSION, SaveOutcome,
    ShellPresentationSession, ShellPresentationState, ShellPresentationStore, StorageAction,
    StorageReadObservation, StorageWriteObservation, classify_serialized_shell_presentation,
    decode_shell_presentation_state, load, repair, save,
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

/// A store seam that counts writes, mirroring the planned-eviction test
/// double used for the composer-draft session custody tests.
struct CountingStore {
    inner: InMemoryShellPresentationStore,
    writes: usize,
}

impl CountingStore {
    fn empty() -> Self {
        Self {
            inner: InMemoryShellPresentationStore::new(),
            writes: 0,
        }
    }
}

impl ShellPresentationStore for CountingStore {
    fn read(&mut self, key: &str) -> Option<String> {
        self.inner.read(key)
    }

    fn write(&mut self, key: &str, value: String) {
        self.writes = self.writes.saturating_add(1);
        self.inner.write(key, value);
    }

    fn remove(&mut self, key: &str) {
        self.inner.remove(key);
    }
}

#[test]
fn codec_encoding_is_the_exact_canonical_document() {
    assert_eq!(
        state(false, false).to_json(),
        "{\"version\":1,\"left_collapsed\":false,\"right_collapsed\":false}"
    );
    assert_eq!(
        state(false, true).to_json(),
        "{\"version\":1,\"left_collapsed\":false,\"right_collapsed\":true}"
    );
    assert_eq!(
        state(true, false).to_json(),
        "{\"version\":1,\"left_collapsed\":true,\"right_collapsed\":false}"
    );
    assert_eq!(
        state(true, true).to_json(),
        "{\"version\":1,\"left_collapsed\":true,\"right_collapsed\":true}"
    );
}

#[test]
fn codec_round_trips_every_collapsed_pair() {
    for (left_collapsed, right_collapsed) in
        [(false, false), (false, true), (true, false), (true, true)]
    {
        let original = state(left_collapsed, right_collapsed);
        let payload = original.to_json();
        assert_eq!(
            decode_shell_presentation_state(&payload),
            Ok(original),
            "payload={payload:?}"
        );
    }
}

#[test]
fn decode_rejects_every_noncanonical_payload() {
    let valid = state(true, false).to_json();
    // The canonical codec intentionally tolerates insignificant whitespace and
    // any field order (covered by the adjacent preferences-policy suite), so
    // those shapes are asserted as valid here rather than rejected.
    for payload in [
        format!("{valid} "),
        format!(" {valid}"),
        "{\"version\":1,\"right_collapsed\":false,\"left_collapsed\":true}".to_owned(),
    ] {
        assert_eq!(
            decode_shell_presentation_state(&payload),
            Ok(state(true, false)),
            "payload={payload:?}"
        );
        assert_eq!(
            classify_serialized_shell_presentation(&payload),
            StorageReadObservation::Valid(state(true, false)),
            "payload={payload:?}"
        );
    }
    let corrupt = [
        String::new(),
        "null".to_owned(),
        "{}".to_owned(),
        "[]".to_owned(),
        "not json at all".to_owned(),
        format!("{valid}}}"),
        valid[1..].to_owned(),
        valid[..valid.len().saturating_sub(1)].to_owned(),
        "{\"version\":0,\"left_collapsed\":false,\"right_collapsed\":false}".to_owned(),
        "{\"version\":2,\"left_collapsed\":false,\"right_collapsed\":false}".to_owned(),
        "{\"version\":01,\"left_collapsed\":false,\"right_collapsed\":false}".to_owned(),
        "{\"version\":\"1\",\"left_collapsed\":false,\"right_collapsed\":false}".to_owned(),
        "{\"left_collapsed\":false,\"right_collapsed\":false}".to_owned(),
        "{\"version\":1,\"left_collapsed\":false}".to_owned(),
        "{\"version\":1,\"left_collapsed\":false,\"right_collapsed\":false,\"extra\":true}"
            .to_owned(),
        "{\"version\":1,\"left_collapsed\":True,\"right_collapsed\":false}".to_owned(),
        "{\"version\":1,\"left_collapsed\":1,\"right_collapsed\":0}".to_owned(),
        "{\"version\":1,\"left_collapsed\":\"false\",\"right_collapsed\":\"false\"}".to_owned(),
        "{\"version\":1,\"left_collapsed\":null,\"right_collapsed\":false}".to_owned(),
    ];

    for payload in &corrupt {
        assert!(
            decode_shell_presentation_state(payload).is_err(),
            "payload={payload:?}"
        );
    }
}

#[test]
fn session_startup_on_missing_key_returns_default_without_repair() {
    let mut store = CountingStore::empty();
    let mut session = ShellPresentationSession::new();

    assert_eq!(
        session.startup(&mut store),
        DEFAULT_SHELL_PRESENTATION_STATE
    );
    assert_eq!(session.state(), DEFAULT_SHELL_PRESENTATION_STATE);
    assert_eq!(store.writes, 0);
    assert!(
        !store
            .inner
            .contains(SHELL_PRESENTATION_PREFERENCES_STORAGE_KEY)
    );
}

#[test]
fn session_writes_on_change_and_reloads_on_startup() {
    let mut store = CountingStore::empty();
    let mut session = ShellPresentationSession::new();
    assert_eq!(
        session.startup(&mut store),
        DEFAULT_SHELL_PRESENTATION_STATE
    );

    assert!(session.update(&mut store, true, false));
    assert_eq!(session.state(), state(true, false));
    assert_eq!(store.writes, 1);
    assert_eq!(
        store.inner.get(SHELL_PRESENTATION_PREFERENCES_STORAGE_KEY),
        Some(&state(true, false).to_json())
    );

    assert!(!session.update(&mut store, true, false));
    assert_eq!(store.writes, 1);

    let mut restarted = ShellPresentationSession::new();
    assert_eq!(restarted.startup(&mut store), state(true, false));
    assert_eq!(restarted.state(), state(true, false));
    assert_eq!(store.writes, 1);
}

#[test]
fn session_startup_repairs_a_corrupt_payload_to_the_default() {
    let mut store = CountingStore::empty();
    store.inner.write(
        SHELL_PRESENTATION_PREFERENCES_STORAGE_KEY,
        "{\"version\":99,\"left_collapsed\":true}".to_owned(),
    );
    let mut session = ShellPresentationSession::new();

    assert_eq!(
        session.startup(&mut store),
        DEFAULT_SHELL_PRESENTATION_STATE
    );
    assert_eq!(session.state(), DEFAULT_SHELL_PRESENTATION_STATE);
    assert_eq!(
        store.inner.get(SHELL_PRESENTATION_PREFERENCES_STORAGE_KEY),
        Some(&DEFAULT_SHELL_PRESENTATION_STATE.to_json())
    );

    let mut restarted = ShellPresentationSession::new();
    assert_eq!(
        restarted.startup(&mut store),
        DEFAULT_SHELL_PRESENTATION_STATE
    );
}
