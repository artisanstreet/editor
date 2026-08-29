//! Deterministic state-table coverage for host-identity refresh custody.
//!
//! The production module is included directly so this focused harness stays
//! dependency-free and does not require shared crate or build-file
//! registration.

#[path = "../../modules/frontend/src/host_identity_controller.rs"]
mod host_identity_controller;

use host_identity_controller::{
    CompletionIgnoredReason, FailureIgnoredReason, HostIdentityAction, HostIdentityGeneration,
    HostIdentityInput, HostIdentitySnapshot, HostIdentityState, HostPlatform, RefreshSuppression,
};

fn snapshot(hostname: &str, display_name: &str, username: &str) -> HostIdentitySnapshot {
    HostIdentitySnapshot {
        display_name: Some(display_name.to_owned()),
        hostname: hostname.to_owned(),
        platform: HostPlatform::Win32,
        username: Some(username.to_owned()),
    }
}

struct Case {
    name: &'static str,
    input: HostIdentityInput,
    expected_action: HostIdentityAction,
    expected_snapshot: Option<HostIdentitySnapshot>,
    expected_in_flight: Option<HostIdentityGeneration>,
    expected_current_generation: HostIdentityGeneration,
}

// This intentionally long, contiguous table keeps each transition beside the
// sequential state it produces.
#[allow(clippy::too_many_lines)]
#[test]
fn refresh_custody_follows_one_deterministic_state_table() {
    let first_generation = HostIdentityGeneration::new(1);
    let second_generation = HostIdentityGeneration::new(2);
    let identity = snapshot("DESKTOP-A", "Alice", "alice");
    let replacement = snapshot("DESKTOP-B", "Bob", "bob");

    let cases = vec![
        Case {
            name: "first refresh is admitted",
            input: HostIdentityInput::refresh(),
            expected_action: HostIdentityAction::RefreshAdmitted {
                generation: first_generation,
            },
            expected_snapshot: None,
            expected_in_flight: Some(first_generation),
            expected_current_generation: first_generation,
        },
        Case {
            name: "duplicate refresh is suppressed",
            input: HostIdentityInput::refresh(),
            expected_action: HostIdentityAction::RefreshSuppressed {
                reason: RefreshSuppression::InFlight {
                    generation: first_generation,
                },
            },
            expected_snapshot: None,
            expected_in_flight: Some(first_generation),
            expected_current_generation: first_generation,
        },
        Case {
            name: "matching failure clears the active generation",
            input: HostIdentityInput::fail(first_generation),
            expected_action: HostIdentityAction::RefreshFailed {
                generation: first_generation,
            },
            expected_snapshot: None,
            expected_in_flight: None,
            expected_current_generation: first_generation,
        },
        Case {
            name: "a later refresh receives a new generation",
            input: HostIdentityInput::refresh(),
            expected_action: HostIdentityAction::RefreshAdmitted {
                generation: second_generation,
            },
            expected_snapshot: None,
            expected_in_flight: Some(second_generation),
            expected_current_generation: second_generation,
        },
        Case {
            name: "old failure cannot clear the newer active generation",
            input: HostIdentityInput::fail(first_generation),
            expected_action: HostIdentityAction::FailureIgnored {
                generation: first_generation,
                reason: FailureIgnoredReason::NoMatchingActiveRefresh,
            },
            expected_snapshot: None,
            expected_in_flight: Some(second_generation),
            expected_current_generation: second_generation,
        },
        Case {
            name: "stale success cannot overwrite newer state",
            input: HostIdentityInput::complete(first_generation, replacement.clone()),
            expected_action: HostIdentityAction::CompletionIgnored {
                generation: first_generation,
                reason: CompletionIgnoredReason::StaleGeneration,
            },
            expected_snapshot: None,
            expected_in_flight: Some(second_generation),
            expected_current_generation: second_generation,
        },
        Case {
            name: "current success publishes and settles",
            input: HostIdentityInput::complete(second_generation, identity.clone()),
            expected_action: HostIdentityAction::IdentityPublished {
                generation: second_generation,
                snapshot: identity.clone(),
            },
            expected_snapshot: Some(identity.clone()),
            expected_in_flight: None,
            expected_current_generation: second_generation,
        },
        Case {
            name: "old failure preserves the last good identity",
            input: HostIdentityInput::fail(first_generation),
            expected_action: HostIdentityAction::FailureIgnored {
                generation: first_generation,
                reason: FailureIgnoredReason::NoMatchingActiveRefresh,
            },
            expected_snapshot: Some(identity.clone()),
            expected_in_flight: None,
            expected_current_generation: second_generation,
        },
        Case {
            name: "late success preserves the last good identity",
            input: HostIdentityInput::complete(second_generation, replacement),
            expected_action: HostIdentityAction::CompletionIgnored {
                generation: second_generation,
                reason: CompletionIgnoredReason::NoMatchingActiveRefresh,
            },
            expected_snapshot: Some(identity.clone()),
            expected_in_flight: None,
            expected_current_generation: second_generation,
        },
        Case {
            name: "loaded identity makes refresh a no-op",
            input: HostIdentityInput::refresh(),
            expected_action: HostIdentityAction::RefreshSuppressed {
                reason: RefreshSuppression::AlreadyLoaded,
            },
            expected_snapshot: Some(identity),
            expected_in_flight: None,
            expected_current_generation: second_generation,
        },
    ];

    let mut state = HostIdentityState::new();
    assert_eq!(state.snapshot(), None);
    assert_eq!(state.in_flight_generation(), None);
    assert_eq!(state.current_generation(), HostIdentityGeneration::INITIAL);

    for case in cases {
        let action = state.apply(case.input);
        assert_eq!(action, case.expected_action, "{}: action", case.name);
        assert_eq!(
            state.snapshot(),
            case.expected_snapshot.as_ref(),
            "{}: snapshot",
            case.name
        );
        assert_eq!(
            state.in_flight_generation(),
            case.expected_in_flight,
            "{}: in-flight generation",
            case.name
        );
        assert_eq!(
            state.is_refresh_in_flight(),
            case.expected_in_flight.is_some(),
            "{}: in-flight flag",
            case.name
        );
        assert_eq!(
            state.current_generation(),
            case.expected_current_generation,
            "{}: current generation",
            case.name
        );
    }
}

#[test]
fn snapshot_shape_and_platform_vocabulary_are_local_values() {
    for (platform, spelling) in [
        (HostPlatform::Win32, "win32"),
        (HostPlatform::Darwin, "darwin"),
        (HostPlatform::Linux, "linux"),
        (HostPlatform::Unknown, "unknown"),
    ] {
        let identity = HostIdentitySnapshot::new("HOST", platform);

        assert_eq!(identity.hostname, "HOST");
        assert_eq!(identity.platform, platform);
        assert_eq!(identity.platform.as_str(), spelling);
        assert_eq!(identity.display_name, None);
        assert_eq!(identity.username, None);
    }

    assert_eq!(HostIdentityGeneration::INITIAL.value(), 0);
}
