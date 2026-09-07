//! Focused parity tests for the dependency-free runtime fixture policy.
//!
//! The production module is path-linked deliberately: this packet does not
//! edit shared frontend registration, so the policy can be checked with plain
//! Rust and no Cargo or Bazel dependencies.

#[path = "../../modules/frontend/src/runtime_fixture_policy.rs"]
mod runtime_fixture_policy;

use runtime_fixture_policy::{
    FixturePolicyCommand, FixtureReceiptIntent, fixture_receipt_intent, update_model_behaviour,
    update_thread_retention_policy, update_thread_session_policy,
};

type ReceiptSelector = fn(Option<&str>) -> FixtureReceiptIntent;

#[test]
fn absent_ids_use_the_exact_command_specific_defaults() {
    let cases = [
        (
            FixturePolicyCommand::UpdateModelBehaviour,
            "fixture-model-behaviour-update",
        ),
        (
            FixturePolicyCommand::UpdateThreadSessionPolicy,
            "fixture-session-policy-update",
        ),
        (
            FixturePolicyCommand::UpdateThreadRetentionPolicy,
            "fixture-retention-update",
        ),
    ];

    for (command, expected_id) in cases {
        let intent = fixture_receipt_intent(command, None);

        assert_eq!(intent.command, command);
        assert_eq!(intent.command_id, expected_id);
    }
}

#[test]
fn explicit_empty_ids_are_preserved_instead_of_defaulted() {
    let cases = [
        FixturePolicyCommand::UpdateModelBehaviour,
        FixturePolicyCommand::UpdateThreadSessionPolicy,
        FixturePolicyCommand::UpdateThreadRetentionPolicy,
    ];

    for command in cases {
        let intent = fixture_receipt_intent(command, Some(""));

        assert_eq!(intent.command, command);
        assert_eq!(intent.command_id, "");
    }
}

#[test]
fn supplied_ids_are_copied_byte_for_byte_for_the_matching_command() {
    let cases = [
        (
            FixturePolicyCommand::UpdateModelBehaviour,
            "model id / exact?",
        ),
        (
            FixturePolicyCommand::UpdateThreadSessionPolicy,
            "session\tID\nwith spacing",
        ),
        (
            FixturePolicyCommand::UpdateThreadRetentionPolicy,
            "保留-更新-🚀",
        ),
    ];

    for (command, supplied_id) in cases {
        let intent = fixture_receipt_intent(command, Some(supplied_id));

        assert_eq!(intent.command, command);
        assert_eq!(intent.command_id.as_bytes(), supplied_id.as_bytes());
    }
}

#[test]
fn default_ids_are_separate_across_commands() {
    let intents = [
        fixture_receipt_intent(FixturePolicyCommand::UpdateModelBehaviour, None),
        fixture_receipt_intent(FixturePolicyCommand::UpdateThreadSessionPolicy, None),
        fixture_receipt_intent(FixturePolicyCommand::UpdateThreadRetentionPolicy, None),
    ];

    assert_ne!(intents[0].command_id, intents[1].command_id);
    assert_ne!(intents[0].command_id, intents[2].command_id);
    assert_ne!(intents[1].command_id, intents[2].command_id);
}

#[test]
fn operation_helpers_keep_their_matching_typed_command() {
    let cases: [(FixturePolicyCommand, ReceiptSelector); 3] = [
        (
            FixturePolicyCommand::UpdateModelBehaviour,
            update_model_behaviour,
        ),
        (
            FixturePolicyCommand::UpdateThreadSessionPolicy,
            update_thread_session_policy,
        ),
        (
            FixturePolicyCommand::UpdateThreadRetentionPolicy,
            update_thread_retention_policy,
        ),
    ];

    for (command, select) in cases {
        let intent = select(Some("supplied-id"));

        assert_eq!(intent.command, command);
        assert_eq!(intent.command_id, "supplied-id");
    }
}
