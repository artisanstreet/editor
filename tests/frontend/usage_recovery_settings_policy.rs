//! Direct dependency-free coverage for usage-recovery settings state policy.
//!
//! The production module is path-linked deliberately. Shared frontend
//! registration remains VP-owned, so this test can run with plain `rustc`.

#[path = "../../modules/frontend/src/usage_recovery_settings_policy.rs"]
mod usage_recovery_settings_policy;

use usage_recovery_settings_policy::{
    USAGE_RECOVERY_SAVE_FAILURE_MESSAGE, USAGE_RECOVERY_SCOPE_FACTS,
    UsageRecoveryAuthoritativeState, UsageRecoveryDefaultScope, UsageRecoveryPersistenceCommand,
    UsageRecoverySaveOutcome, UsageRecoverySaveRejection, UsageRecoverySettingsState,
    usage_recovery_scope_facts,
};

fn state(available: bool, auto_continue: bool) -> UsageRecoverySettingsState {
    UsageRecoverySettingsState::new(UsageRecoveryAuthoritativeState::new(
        available,
        auto_continue,
    ))
}

#[test]
fn availability_and_saving_both_disable_switch_admission() {
    for (available, saving, expected_disabled) in [
        (false, false, true),
        (false, true, true),
        (true, false, false),
        (true, true, true),
    ] {
        let mut current = state(available, false);
        current.saving = saving;

        assert_eq!(current.switch_disabled(), expected_disabled);
        assert_eq!(current.can_start_save(), !expected_disabled);
    }
}

#[test]
fn admitted_save_clears_message_emits_inverse_command_and_does_not_flip_default() {
    let mut current = state(true, false);
    current.message = "old failure".to_owned();

    let command = current.start_save().expect("available idle switch admits");

    assert_eq!(
        command,
        UsageRecoveryPersistenceCommand::SetAutoContinueUsageLimits { enabled: true }
    );
    assert!(command.enabled());
    assert!(current.saving);
    assert!(current.message.is_empty());
    assert!(!current.auto_continue_usage_limits);

    let mut enabled = state(true, true);
    let command = enabled.start_save().expect("available idle switch admits");
    assert!(!command.enabled());
    assert!(enabled.auto_continue_usage_limits);
}

#[test]
fn unavailable_or_already_saving_state_emits_no_command_and_preserves_local_state() {
    let mut unavailable = state(false, true);
    unavailable.message = "keep this".to_owned();
    let before = unavailable.clone();
    assert_eq!(
        unavailable.start_save(),
        Err(UsageRecoverySaveRejection::Unavailable)
    );
    assert_eq!(unavailable, before);

    let mut saving = state(true, false);
    saving.saving = true;
    saving.message = "keep this too".to_owned();
    let before = saving.clone();
    assert_eq!(saving.start_save(), Err(UsageRecoverySaveRejection::Saving));
    assert_eq!(saving, before);
}

#[test]
fn success_always_clears_saving_without_fabricating_a_default() {
    let mut current = state(true, false);
    let command = current.start_save().expect("save starts");
    assert!(command.enabled());

    current.finish_save(UsageRecoverySaveOutcome::Succeeded);

    assert!(!current.saving);
    assert!(current.message.is_empty());
    assert!(!current.auto_continue_usage_limits);

    current.start_save().expect("a settled save can be retried");
    current.save_succeeded();
    assert!(!current.saving);
    assert!(current.message.is_empty());
    assert!(!current.auto_continue_usage_limits);
}

#[test]
fn failure_clears_saving_reports_exact_text_and_keeps_default() {
    let mut current = state(true, false);
    current.start_save().expect("save starts");
    current.finish_save(UsageRecoverySaveOutcome::Failed);

    assert!(!current.saving);
    assert_eq!(current.message, USAGE_RECOVERY_SAVE_FAILURE_MESSAGE);
    assert_eq!(
        current.message,
        "Couldn't verify the new default. Forge did not confirm the change."
    );
    assert!(!current.auto_continue_usage_limits);

    // The convenience path has the same exact failure behavior.
    current.start_save().expect("a settled save can be retried");
    current.save_failed();
    assert!(!current.saving);
    assert_eq!(current.message, USAGE_RECOVERY_SAVE_FAILURE_MESSAGE);
    assert!(!current.auto_continue_usage_limits);
}

#[test]
fn authoritative_stream_updates_replace_default_but_preserve_local_state() {
    let mut current = state(true, false);
    current.start_save().expect("save starts");
    current.message = "local status".to_owned();

    current.apply_authoritative_update(UsageRecoveryAuthoritativeState::new(false, true));

    assert!(!current.available);
    assert!(current.auto_continue_usage_limits);
    assert!(current.saving);
    assert_eq!(current.message, "local status");
    assert_eq!(
        current.authoritative(),
        UsageRecoveryAuthoritativeState::new(false, true)
    );
}

#[test]
fn scope_facts_preserve_new_turn_default_and_interruption_override() {
    let facts = usage_recovery_scope_facts();

    assert_eq!(facts, USAGE_RECOVERY_SCOPE_FACTS);
    assert_eq!(facts.default_scope, UsageRecoveryDefaultScope::NewTurn);
    assert_eq!(facts.default_scope.as_str(), "new_turn");
    assert!(facts.per_interruption_override_available);
}
