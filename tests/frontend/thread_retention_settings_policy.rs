//! Dependency-free transition coverage for the thread-retention settings
//! policy.
//!
//! The production module is linked by path so these tests do not require the
//! VP-owned frontend registration, controller, transport, streams, or UI.

#[path = "../../modules/frontend/src/thread_retention_settings_policy.rs"]
mod thread_retention_settings_policy;

use thread_retention_settings_policy::{
    THREAD_RETENTION_MAX_INACTIVITY_DAYS, THREAD_RETENTION_MIN_INACTIVITY_DAYS,
    THREAD_RETENTION_SETTINGS_LOAD_FAILURE_TITLE, THREAD_RETENTION_SETTINGS_SAVE_FAILURE_TITLE,
    ThreadRetentionPolicy, ThreadRetentionPolicyState, ThreadRetentionSettings,
    ThreadRetentionSettingsPersistenceCommand, ThreadRetentionSettingsRefreshAction,
    ThreadRetentionSettingsSaveAction, ThreadRetentionSettingsSaveOutcome,
    ThreadRetentionSettingsState, parse_inactivity_days,
};

fn policy(enabled: bool, inactivity_days: u16) -> ThreadRetentionPolicy {
    ThreadRetentionPolicy::new(enabled, inactivity_days)
}

fn ready(enabled: bool, inactivity_days: u16) -> ThreadRetentionSettingsState {
    ThreadRetentionSettingsState::from_policy(policy(enabled, inactivity_days))
}

fn save_policy(action: Option<ThreadRetentionSettingsPersistenceCommand>) -> ThreadRetentionPolicy {
    match action {
        Some(action) => {
            assert_eq!(action.requested_policy(), action.policy());
            action.policy()
        }
        None => panic!("expected an admitted save action"),
    }
}

#[test]
fn all_controller_state_tags_have_typed_projections_and_controls() {
    let loading = ThreadRetentionSettingsState::new(ThreadRetentionPolicyState::Loading);
    assert_eq!(loading.policy_state, ThreadRetentionPolicyState::Loading);
    assert_eq!(loading.state(), &ThreadRetentionPolicyState::Loading);
    assert_eq!(loading.policy_state(), &ThreadRetentionPolicyState::Loading);
    assert_eq!(loading.policy(), None);
    assert_eq!(loading.days_text(), "");
    assert!(!loading.is_saving());
    assert_eq!(
        loading.failure_title(),
        THREAD_RETENTION_SETTINGS_LOAD_FAILURE_TITLE
    );
    assert_eq!(
        loading.policy_failure_title(),
        THREAD_RETENTION_SETTINGS_LOAD_FAILURE_TITLE
    );
    assert!(!loading.toggle_enabled());
    assert!(loading.toggle_disabled());
    assert!(loading.switch_disabled());
    assert!(!loading.days_input_enabled());
    assert!(loading.days_input_disabled());
    assert!(loading.days_disabled());
    assert!(loading.retry_enabled());
    assert!(!loading.retry_disabled());
    assert!(loading.can_admit_retry());
    assert!(!loading.can_admit_toggle());
    assert!(!loading.can_admit_days_commit());

    let unverified = ThreadRetentionSettingsState::new(ThreadRetentionPolicyState::Unverified);
    assert_eq!(unverified.policy(), None);
    assert_eq!(unverified.days_text(), "");
    assert!(unverified.policy_state.is_unavailable());
    assert!(!unverified.toggle_enabled());
    assert!(unverified.toggle_disabled());
    assert!(unverified.days_input_disabled());

    let enabled = ready(true, 7);
    assert_eq!(
        enabled.policy_state,
        ThreadRetentionPolicyState::Ready {
            policy: policy(true, 7)
        }
    );
    assert_eq!(enabled.policy(), Some(policy(true, 7)));
    assert_eq!(enabled.days_text(), "7");
    assert_eq!(
        enabled.failure_title(),
        THREAD_RETENTION_SETTINGS_LOAD_FAILURE_TITLE
    );
    assert!(enabled.policy_state.is_ready());
    assert!(!enabled.policy_state.is_unavailable());
    assert!(enabled.toggle_enabled());
    assert!(!enabled.toggle_disabled());
    assert!(enabled.days_input_enabled());
    assert!(!enabled.days_input_disabled());
    assert!(enabled.can_admit_toggle());
    assert!(enabled.can_admit_days_commit());

    let disabled = ready(false, 7);
    assert!(disabled.toggle_enabled());
    assert!(!disabled.toggle_disabled());
    assert!(!disabled.days_input_enabled());
    assert!(disabled.days_input_disabled());
    // The source CommitDays guard checks Ready/saving, not policy.enabled;
    // the disabled number input is the presentation-level guard.
    assert!(disabled.can_admit_days_commit());

    assert_eq!(
        ThreadRetentionSettingsState::default().policy_state,
        ThreadRetentionPolicyState::Loading
    );
}

#[test]
fn policy_validation_matches_the_protocol_bounds() {
    assert_eq!(THREAD_RETENTION_MIN_INACTIVITY_DAYS, 1);
    assert_eq!(THREAD_RETENTION_MAX_INACTIVITY_DAYS, 3650);

    assert!(ThreadRetentionPolicy::try_new(true, 0).is_err());
    assert!(ThreadRetentionPolicy::try_new(true, 1).is_ok());
    assert!(ThreadRetentionPolicy::try_new(false, 3650).is_ok());
    assert!(ThreadRetentionPolicy::try_new(false, 3651).is_err());
    assert!(!policy(true, 0).is_valid());
    assert!(policy(true, 1).is_valid());
    assert!(policy(true, 3650).is_valid());
    assert!(!policy(true, 3651).is_valid());
}

#[test]
fn streamed_state_replaces_only_ready_projection_fields() {
    let mut current = ready(true, 42);
    current.failure_title = "old failure".to_owned();
    current.days_text = "  99  ".to_owned();
    current.saving = true;

    current.apply_streamed_state(ThreadRetentionPolicyState::Loading);
    assert_eq!(current.policy_state, ThreadRetentionPolicyState::Loading);
    assert_eq!(current.days_text(), "  99  ");
    assert_eq!(current.failure_title(), "old failure");
    assert!(current.saving);

    current.apply_policy_state(ThreadRetentionPolicyState::Unverified);
    assert_eq!(current.policy_state, ThreadRetentionPolicyState::Unverified);
    assert_eq!(current.days_text(), "  99  ");
    assert_eq!(current.failure_title(), "old failure");
    assert!(current.saving);

    current.apply_streamed_state(ThreadRetentionPolicyState::Ready {
        policy: policy(false, 3650),
    });
    assert_eq!(
        current.policy(),
        Some(policy(false, THREAD_RETENTION_MAX_INACTIVITY_DAYS))
    );
    assert_eq!(current.days_text(), "3650");
    assert!(current.failure_title().is_empty());
    assert!(current.saving);
    assert!(!current.days_input_enabled());
}

#[test]
fn retry_enters_loading_and_refreshes_have_typed_success_and_failure_paths() {
    let mut current = ready(true, 20);
    current.failure_title = "previous title".to_owned();
    current.set_days_text("stale input");

    let action = current.retry();
    assert_eq!(action, ThreadRetentionSettingsRefreshAction::RefreshPolicy);
    assert_eq!(action.operation(), "thread.retention.query");
    assert_eq!(current.policy_state, ThreadRetentionPolicyState::Loading);
    assert_eq!(current.days_text(), "");
    // Retry does not clear the title; the legacy handler only changes state
    // and days before the refresh operation runs.
    assert_eq!(current.failure_title(), "previous title");

    current.refresh_succeeded(policy(true, 3650));
    assert_eq!(
        current.policy_state,
        ThreadRetentionPolicyState::Ready {
            policy: policy(true, 3650)
        }
    );
    assert_eq!(current.days_text(), "3650");
    assert!(current.failure_title().is_empty());

    current.set_days_text("will be retained until the next ready stream");
    current.refresh_failed();
    assert_eq!(
        current.failure_title(),
        THREAD_RETENTION_SETTINGS_LOAD_FAILURE_TITLE
    );
    assert_eq!(
        current.policy_state,
        ThreadRetentionPolicyState::Ready {
            policy: policy(true, 3650),
        }
    );
    current.apply_streamed_state(ThreadRetentionPolicyState::Unverified);
    assert_eq!(current.policy_state, ThreadRetentionPolicyState::Unverified);
    assert_eq!(
        current.days_text(),
        "will be retained until the next ready stream"
    );
    assert_eq!(
        current.failure_title(),
        THREAD_RETENTION_SETTINGS_LOAD_FAILURE_TITLE
    );

    current.refresh_succeeded(policy(false, 1));
    assert_eq!(current.days_text(), "1");
    assert!(current.failure_title().is_empty());

    current.apply_refresh_failure();
    assert_eq!(current.policy_state, ThreadRetentionPolicyState::Unverified);
    assert_eq!(
        current.failure_title(),
        THREAD_RETENTION_SETTINGS_LOAD_FAILURE_TITLE
    );
}

#[test]
fn retry_admission_obeys_the_rendered_save_suppression() {
    let mut current = ready(true, 8);
    current.saving = true;
    current.set_days_text("keep while blocked");
    let snapshot = current.clone();

    assert!(!current.can_admit_retry());
    assert_eq!(current.admit_retry(), None);
    assert_eq!(current, snapshot);

    // The source handler itself is a separate transition and still has the
    // documented immediate Loading/clear-days behavior if called directly.
    assert_eq!(
        current.retry(),
        ThreadRetentionSettingsRefreshAction::RefreshPolicy
    );
    assert_eq!(current.policy_state, ThreadRetentionPolicyState::Loading);
    assert_eq!(current.days_text(), "");
}

#[test]
fn both_toggle_directions_emit_the_exact_policy_and_keep_authority_streamed() {
    let mut current = ready(false, 30);
    current.failure_title = "keep until a Ready stream".to_owned();

    let enable = current.toggle();
    assert_eq!(
        enable,
        Some(ThreadRetentionSettingsSaveAction::SavePolicy {
            policy: policy(true, 30)
        })
    );
    assert_eq!(save_policy(enable), policy(true, 30));
    assert!(current.saving);
    assert_eq!(current.policy(), Some(policy(false, 30)));
    assert_eq!(current.days_text(), "30");
    assert_eq!(current.failure_title(), "keep until a Ready stream");
    assert_eq!(
        ThreadRetentionSettingsSaveAction::SavePolicy {
            policy: policy(true, 30)
        }
        .operation(),
        "thread.retention.update"
    );

    current.save_succeeded();
    assert!(!current.saving);
    current.apply_streamed_state(ThreadRetentionPolicyState::Ready {
        policy: policy(true, 30),
    });
    assert!(current.failure_title().is_empty());

    let disable = current.start_toggle(false);
    assert_eq!(
        disable,
        Some(ThreadRetentionSettingsSaveAction::SavePolicy {
            policy: policy(false, 30)
        })
    );
    assert_eq!(save_policy(disable), policy(false, 30));
    assert!(current.saving);
    assert_eq!(current.policy(), Some(policy(true, 30)));
}

#[test]
fn toggle_is_a_no_op_for_every_unavailable_tag_and_while_saving() {
    for state in [
        ThreadRetentionPolicyState::Loading,
        ThreadRetentionPolicyState::Unverified,
    ] {
        let mut current = ThreadRetentionSettingsState::new(state);
        current.set_days_text("do not touch");
        let snapshot = current.clone();
        assert_eq!(current.admit_toggle(true), None);
        assert_eq!(current.toggle(), None);
        assert_eq!(current, snapshot);
    }

    let mut current = ready(true, 12);
    current.set_days_text("12.0");
    current.saving = true;
    let snapshot = current.clone();
    assert_eq!(current.admit_toggle(false), None);
    assert_eq!(current.toggle(), None);
    assert_eq!(current, snapshot);
}

#[test]
fn save_success_and_failure_always_clear_saving_in_their_source_order() {
    let mut current = ready(true, 21);
    current.set_days_text("21.0");
    current.failure_title = "old title".to_owned();
    current.saving = true;
    current.finish_save(ThreadRetentionSettingsSaveOutcome::Succeeded);
    assert!(!current.saving);
    assert_eq!(current.days_text(), "21.0");
    assert_eq!(current.failure_title(), "old title");

    current.saving = true;
    current.save_failed();
    assert!(!current.saving);
    assert_eq!(current.days_text(), "");
    assert_eq!(
        current.failure_title(),
        THREAD_RETENTION_SETTINGS_SAVE_FAILURE_TITLE
    );
    // The controller's Unverified stream is a separate observation.
    assert_eq!(current.policy(), Some(policy(true, 21)));

    current.saving = true;
    current.finish_save(ThreadRetentionSettingsSaveOutcome::Failed);
    assert!(!current.saving);
    assert_eq!(current.days_text(), "");
    assert_eq!(
        current.failure_title(),
        THREAD_RETENTION_SETTINGS_SAVE_FAILURE_TITLE
    );

    current.apply_save_failure();
    assert_eq!(current.policy_state, ThreadRetentionPolicyState::Unverified);
    assert!(!current.saving);
    assert_eq!(current.days_text(), "");
    assert_eq!(
        current.failure_title(),
        THREAD_RETENTION_SETTINGS_SAVE_FAILURE_TITLE
    );
}

#[test]
fn save_stream_ready_during_a_flight_replaces_days_but_does_not_settle_saving() {
    let mut current = ready(true, 10);
    let action = current.admit_toggle(false);
    assert_eq!(save_policy(action), policy(false, 10));
    assert!(current.saving);

    current.failure_title = "stream must clear this on Ready".to_owned();
    current.set_days_text("pending input");
    current.apply_streamed_state(ThreadRetentionPolicyState::Ready {
        policy: policy(false, 99),
    });
    assert_eq!(current.policy(), Some(policy(false, 99)));
    assert_eq!(current.days_text(), "99");
    assert!(current.failure_title().is_empty());
    assert!(current.saving);
    assert!(current.toggle_disabled());
    assert!(current.days_input_disabled());
    assert_eq!(current.toggle(), None);

    current.save_succeeded();
    assert!(!current.saving);
    assert!(current.toggle_enabled());
    assert!(!current.days_input_enabled());
}

#[test]
fn day_input_boundaries_and_javascript_number_forms_are_exact() {
    for (input, expected) in [
        ("0", None),
        ("1", Some(1)),
        ("3650", Some(3650)),
        ("3651", None),
        ("", None),
        ("   ", None),
        ("\t\n", None),
        ("1.5", None),
        ("3650.5", None),
        ("1e0", Some(1)),
        ("3.65e3", Some(3650)),
        ("3.651e3", None),
        ("1e-1", None),
        ("not a number", None),
        ("NaN", None),
        ("Infinity", None),
        ("0x10", Some(16)),
        ("0b10", Some(2)),
        ("0o10", Some(8)),
    ] {
        assert_eq!(parse_inactivity_days(input), expected, "input {input:?}");
    }
}

#[test]
fn invalid_day_input_reverts_and_valid_unchanged_input_emits_no_save() {
    for input in ["0", "3651", "", "  ", "1.5", "3.651e3", "not a number"] {
        let mut current = ready(true, 42);
        current.set_days_text(input);
        assert_eq!(current.commit_days(), None, "input {input:?}");
        assert_eq!(current.days_text(), "42", "input {input:?}");
        assert!(!current.saving, "input {input:?}");
    }

    for input in ["42", "42.0", "4.2e1", "0x2a"] {
        let mut current = ready(true, 42);
        current.set_days_text(input);
        assert_eq!(current.commit_days(), None, "input {input:?}");
        assert_eq!(current.days_text(), input, "input {input:?}");
        assert!(!current.saving, "input {input:?}");
    }
}

#[test]
fn changed_day_values_emit_saves_at_both_inclusive_boundaries() {
    for (current_days, input, expected_days) in [(42, "1", 1), (42, "3650", 3650)] {
        let mut current: ThreadRetentionSettings = ready(true, current_days);
        current.set_days_text(input);
        let action = current.commit_days();
        assert_eq!(save_policy(action), policy(true, expected_days));
        assert!(current.saving);
        assert_eq!(current.days_text(), input);
    }

    let mut disabled = ready(false, 42);
    disabled.set_days_text("1");
    assert_eq!(
        save_policy(disabled.commit_days()),
        policy(false, 1),
        "the source commit guard does not inspect policy.enabled"
    );
}

#[test]
fn day_commit_is_a_no_op_when_not_ready_or_already_saving() {
    for state in [
        ThreadRetentionPolicyState::Loading,
        ThreadRetentionPolicyState::Unverified,
    ] {
        let mut current = ThreadRetentionSettingsState::new(state);
        current.set_days_text("99");
        let snapshot = current.clone();
        assert_eq!(current.commit_days(), None);
        assert_eq!(current, snapshot);
    }

    let mut current = ready(true, 7);
    current.set_days_text("8");
    current.saving = true;
    let snapshot = current.clone();
    assert_eq!(current.commit_days(), None);
    assert_eq!(current, snapshot);
}

#[test]
fn only_exact_enter_runs_the_keyboard_commit() {
    for key in ["", "enter", "ENTER", "NumpadEnter", "Escape", "Tab", " "] {
        let mut current = ready(true, 7);
        current.set_days_text("8");
        let snapshot = current.clone();
        assert_eq!(current.commit_days_on_key(key), None, "key {key:?}");
        assert_eq!(current, snapshot, "key {key:?}");
    }

    let mut current = ready(true, 7);
    current.set_days_text("8");
    assert_eq!(
        current.commit_days_on_key("Enter"),
        Some(ThreadRetentionSettingsSaveAction::SavePolicy {
            policy: policy(true, 8)
        })
    );
    assert!(current.saving);

    current.save_succeeded();
    current.apply_streamed_state(ThreadRetentionPolicyState::Ready {
        policy: policy(true, 8),
    });
    current.set_days_text("not a number");
    assert_eq!(current.commit_days_on_enter("Enter"), None);
    assert_eq!(current.days_text(), "8");
}
