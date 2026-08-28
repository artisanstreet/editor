//! Exhaustive, dependency-free coverage for marketplace fixture outcomes.
//!
//! The production module is path-linked deliberately. This keeps the focused
//! harness independent of shared module registration, transport, and runtime
//! dependencies.

#[path = "../../modules/frontend/src/marketplace_fixture_policy.rs"]
mod marketplace_fixture_policy;

use marketplace_fixture_policy::{
    CAPABILITY_FIXTURES_UNAVAILABLE_MESSAGE, FIXTURE_OAUTH_AUTHORIZATION_URL,
    FIXTURE_OAUTH_CONTINUATION_REFERENCE, MarketplaceFixtureCommand, MarketplaceFixtureOutcome,
    NPX_SKILLS_FIXTURES_UNAVAILABLE_MESSAGE, ROUTINE_FIXTURES_UNAVAILABLE_MESSAGE,
    marketplace_fixture_outcome,
};

#[derive(Clone, Copy, Debug)]
enum ExpectedOutcome {
    Receipt(&'static str),
    Unsupported(&'static str),
    OAuthBegin,
}

const ALL_OPERATIONS: [(MarketplaceFixtureCommand, ExpectedOutcome); 37] = {
    use MarketplaceFixtureCommand::*;

    [
        (
            PreviewRoutineInstall,
            ExpectedOutcome::Unsupported(ROUTINE_FIXTURES_UNAVAILABLE_MESSAGE),
        ),
        (
            RequestRoutineInstall,
            ExpectedOutcome::Receipt("fixture-routine-install"),
        ),
        (
            DecideRoutineInstall,
            ExpectedOutcome::Receipt("fixture-routine-decision"),
        ),
        (
            EnableRoutine,
            ExpectedOutcome::Receipt("fixture-routine-enable"),
        ),
        (
            DisableRoutine,
            ExpectedOutcome::Receipt("fixture-routine-disable"),
        ),
        (
            RemoveRoutine,
            ExpectedOutcome::Receipt("fixture-routine-remove"),
        ),
        (
            RollbackRoutine,
            ExpectedOutcome::Receipt("fixture-routine-rollback"),
        ),
        (
            SyncRoutine,
            ExpectedOutcome::Receipt("fixture-routine-sync"),
        ),
        (
            ResolveRoutineDrift,
            ExpectedOutcome::Receipt("fixture-routine-drift"),
        ),
        (
            RequestRoutineDriftOverwrite,
            ExpectedOutcome::Receipt("fixture-routine-drift-overwrite-request"),
        ),
        (
            DecideRoutineDriftOverwrite,
            ExpectedOutcome::Receipt("fixture-routine-drift-overwrite-decision"),
        ),
        (
            InvokeRoutine,
            ExpectedOutcome::Unsupported(ROUTINE_FIXTURES_UNAVAILABLE_MESSAGE),
        ),
        (
            DiscoverNpxSkills,
            ExpectedOutcome::Unsupported(NPX_SKILLS_FIXTURES_UNAVAILABLE_MESSAGE),
        ),
        (
            ImportNpxSkills,
            ExpectedOutcome::Receipt("fixture-npx-skills-import"),
        ),
        (
            PreviewCapabilityConnect,
            ExpectedOutcome::Unsupported(CAPABILITY_FIXTURES_UNAVAILABLE_MESSAGE),
        ),
        (
            RequestCapabilityConnect,
            ExpectedOutcome::Receipt("fixture-capability-connect"),
        ),
        (
            DecideCapabilityConnect,
            ExpectedOutcome::Receipt("fixture-capability-decision"),
        ),
        (
            StartCapability,
            ExpectedOutcome::Receipt("fixture-capability-start"),
        ),
        (
            ReconnectCapability,
            ExpectedOutcome::Receipt("fixture-capability-reconnect"),
        ),
        (
            CheckCapabilityHealth,
            ExpectedOutcome::Receipt("fixture-capability-health"),
        ),
        (
            DisconnectCapability,
            ExpectedOutcome::Receipt("fixture-capability-disconnect"),
        ),
        (
            RestartCapability,
            ExpectedOutcome::Receipt("fixture-capability-restart"),
        ),
        (
            UninstallCapability,
            ExpectedOutcome::Receipt("fixture-capability-uninstall"),
        ),
        (
            EnableCapability,
            ExpectedOutcome::Receipt("fixture-capability-enable"),
        ),
        (
            DisableCapability,
            ExpectedOutcome::Receipt("fixture-capability-disable"),
        ),
        (
            RemoveCapability,
            ExpectedOutcome::Receipt("fixture-capability-remove"),
        ),
        (
            SyncCapability,
            ExpectedOutcome::Receipt("fixture-capability-sync"),
        ),
        (
            ResolveCapabilityDrift,
            ExpectedOutcome::Receipt("fixture-capability-drift"),
        ),
        (
            RequestCapabilityDriftOverwrite,
            ExpectedOutcome::Receipt("fixture-capability-drift-overwrite-request"),
        ),
        (
            DecideCapabilityDriftOverwrite,
            ExpectedOutcome::Receipt("fixture-capability-drift-overwrite-decision"),
        ),
        (
            RequestCapabilityInvocation,
            ExpectedOutcome::Unsupported(CAPABILITY_FIXTURES_UNAVAILABLE_MESSAGE),
        ),
        (
            DecideCapabilityInvocation,
            ExpectedOutcome::Unsupported(CAPABILITY_FIXTURES_UNAVAILABLE_MESSAGE),
        ),
        (
            InvokeCapability,
            ExpectedOutcome::Unsupported(CAPABILITY_FIXTURES_UNAVAILABLE_MESSAGE),
        ),
        (BeginCapabilityOAuth, ExpectedOutcome::OAuthBegin),
        (
            CompleteCapabilityOAuth,
            ExpectedOutcome::Receipt("fixture-capability-oauth-complete"),
        ),
        (
            RefreshCapabilityOAuth,
            ExpectedOutcome::Receipt("fixture-capability-oauth-refresh"),
        ),
        (
            RevokeCapabilityOAuth,
            ExpectedOutcome::Receipt("fixture-capability-oauth-revoke"),
        ),
    ]
};

#[test]
fn every_legacy_operation_has_its_exact_default_outcome() {
    for (command, expected) in ALL_OPERATIONS {
        let outcome = marketplace_fixture_outcome(command, None);
        assert_eq!(outcome.operation(), command);

        match (expected, outcome) {
            (
                ExpectedOutcome::Receipt(expected_id),
                MarketplaceFixtureOutcome::Receipt {
                    operation,
                    command_id,
                },
            ) => {
                assert_eq!(operation, command);
                assert_eq!(command_id, expected_id);
                assert_eq!(command.fallback_command_id(), Some(expected_id));
                assert!(command.produces_receipt());
                assert!(!command.is_unsupported());
            }
            (
                ExpectedOutcome::Unsupported(expected_message),
                MarketplaceFixtureOutcome::Unsupported { operation, message },
            ) => {
                assert_eq!(operation, command);
                assert_eq!(message, expected_message);
                assert_eq!(command.unsupported_message(), Some(expected_message));
                assert!(!command.produces_receipt());
                assert!(command.is_unsupported());
            }
            (
                ExpectedOutcome::OAuthBegin,
                MarketplaceFixtureOutcome::OAuthBegin {
                    operation,
                    authorization_url,
                    continuation_reference,
                },
            ) => {
                assert_eq!(operation, command);
                assert_eq!(authorization_url, FIXTURE_OAUTH_AUTHORIZATION_URL);
                assert_eq!(continuation_reference, FIXTURE_OAUTH_CONTINUATION_REFERENCE);
                assert_eq!(command.fallback_command_id(), None);
                assert!(!command.produces_receipt());
                assert!(!command.is_unsupported());
            }
            (
                ExpectedOutcome::Receipt(_)
                | ExpectedOutcome::Unsupported(_)
                | ExpectedOutcome::OAuthBegin,
                other,
            ) => {
                panic!("unexpected outcome for {command:?}: {other:?}");
            }
        }
    }
}

#[test]
fn every_receipt_operation_prefers_a_supplied_id_byte_for_byte() {
    for (command, expected) in ALL_OPERATIONS {
        let ExpectedOutcome::Receipt(fallback) = expected else {
            continue;
        };
        let supplied_id = "provided\tID\nwith unicode 🚀";

        let outcome = command.outcome(Some(supplied_id));
        assert_eq!(
            outcome,
            MarketplaceFixtureOutcome::Receipt {
                operation: command,
                command_id: supplied_id.to_owned(),
            }
        );
        assert_eq!(outcome.command_id(), Some(supplied_id));
        assert_eq!(command.fallback_command_id(), Some(fallback));
    }
}

#[test]
fn an_explicit_empty_id_is_not_treated_as_absent() {
    for (command, expected) in ALL_OPERATIONS {
        if let ExpectedOutcome::Receipt(_) = expected {
            assert_eq!(
                command.outcome(Some("")),
                MarketplaceFixtureOutcome::Receipt {
                    operation: command,
                    command_id: String::new(),
                }
            );
        }
    }
}

#[test]
fn unsupported_and_oauth_operations_do_not_turn_supplied_ids_into_receipts() {
    for (command, expected) in ALL_OPERATIONS {
        if let ExpectedOutcome::Receipt(_) = expected {
            continue;
        }

        let outcome = command.outcome(Some("must-not-become-a-receipt"));
        assert_eq!(outcome.operation(), command);
        assert_eq!(outcome.command_id(), None);

        match (expected, outcome) {
            (
                ExpectedOutcome::Unsupported(expected_message),
                MarketplaceFixtureOutcome::Unsupported { message, .. },
            ) => assert_eq!(message, expected_message),
            (
                ExpectedOutcome::OAuthBegin,
                MarketplaceFixtureOutcome::OAuthBegin {
                    authorization_url,
                    continuation_reference,
                    ..
                },
            ) => {
                assert_eq!(authorization_url, FIXTURE_OAUTH_AUTHORIZATION_URL);
                assert_eq!(continuation_reference, FIXTURE_OAUTH_CONTINUATION_REFERENCE);
            }
            (other, outcome) => panic!("unexpected pair {other:?} / {outcome:?}"),
        }
    }
}

#[test]
fn every_receipt_fallback_is_unique_and_operation_identity_survives_duplicate_ids() {
    let mut fallback_ids = Vec::new();
    for (command, expected) in ALL_OPERATIONS {
        let ExpectedOutcome::Receipt(expected_id) = expected else {
            continue;
        };
        assert!(
            !fallback_ids.contains(&expected_id),
            "duplicate fallback ID {expected_id:?} for {command:?}"
        );
        fallback_ids.push(expected_id);
    }

    assert_eq!(fallback_ids.len(), 29);

    let first = MarketplaceFixtureCommand::RequestRoutineInstall.outcome(Some("same-id"));
    let second = MarketplaceFixtureCommand::RequestCapabilityConnect.outcome(Some("same-id"));
    assert_eq!(first.command_id(), Some("same-id"));
    assert_eq!(second.command_id(), Some("same-id"));
    assert_ne!(first.operation(), second.operation());
}
