//! Focused dependency-free coverage for the conversation turn footer policy.

#[path = "../../modules/frontend/src/conversation_turn_footer_policy.rs"]
mod conversation_turn_footer_policy;

use conversation_turn_footer_policy::{
    COPY_FAILURE_MESSAGE, COPY_RESPONSE_LABEL, ConversationTurnFooterPolicy, CopyOutcome,
    TURN_ACTIONS_LABEL, TurnFooterAccessibility, TurnFooterAction, TurnFooterInput,
};

const SETTLED_AT: &str = "2026-08-28T08:00:00.000Z";
const RESPONSE: &str = " exact response\nwith Unicode: café 🚀 ";

fn policy() -> ConversationTurnFooterPolicy {
    ConversationTurnFooterPolicy::new(SETTLED_AT, RESPONSE, "adapter age")
}

#[test]
fn hover_and_focus_each_admit_one_clock_sample_request() {
    let mut footer = policy();

    for input in [TurnFooterInput::Hover, TurnFooterInput::Focus] {
        let action = footer.observe(input);
        assert_eq!(action, TurnFooterAction::RequestClockSample);
        assert!(action.is_clock_sample_request());
        assert!(!action.is_no_op());
    }
}

#[test]
fn periodic_wakeups_are_not_refresh_triggers() {
    let mut footer = policy();
    let original_age = footer.relative_age().to_owned();

    for _ in 0..3 {
        assert_eq!(
            footer.observe(TurnFooterInput::PeriodicTick),
            TurnFooterAction::NoOp
        );
    }

    assert_eq!(footer.relative_age(), original_age);
    assert_eq!(footer.settled_at(), SETTLED_AT);
}

#[test]
fn adapter_formatted_age_is_consumed_without_timestamp_reimplementation() {
    let mut footer = policy();
    footer.set_relative_age("adapter-owned 2mo ago — exact");

    assert_eq!(footer.relative_age(), "adapter-owned 2mo ago — exact");
    assert_eq!(footer.settled_at(), SETTLED_AT);
    assert_eq!(footer.response_text(), RESPONSE);
}

#[test]
fn copy_action_carries_the_exact_response_payload() {
    let mut footer = policy();
    let action = footer.observe(TurnFooterInput::Copy);

    assert_eq!(action.copy_text(), Some(RESPONSE));
    assert_eq!(
        action,
        TurnFooterAction::CopyResponse {
            text: RESPONSE.to_owned()
        }
    );
    assert_eq!(footer.response_text(), RESPONSE);
}

#[test]
fn starting_copy_clears_a_prior_failure_message() {
    let mut footer = policy();
    footer.settle_copy(CopyOutcome::Failed);
    assert_eq!(footer.copy_message(), COPY_FAILURE_MESSAGE);

    assert_eq!(footer.start_copy().copy_text(), Some(RESPONSE));
    assert_eq!(footer.copy_message(), "");
}

#[test]
fn copy_success_keeps_the_message_empty() {
    let mut footer = policy();
    let _ = footer.start_copy();
    footer.settle_copy(CopyOutcome::Succeeded);

    assert_eq!(footer.copy_message(), "");
}

#[test]
fn copy_failure_exposes_the_exact_stable_message() {
    let mut footer = policy();
    let _ = footer.start_copy();
    footer.settle_copy(CopyOutcome::Failed);

    assert_eq!(footer.copy_message(), COPY_FAILURE_MESSAGE);
}

#[test]
fn settled_timestamp_and_response_remain_distinct_after_all_updates() {
    let mut footer = policy();
    let _ = footer.observe(TurnFooterInput::Hover);
    footer.set_relative_age("new adapter age");
    let _ = footer.start_copy();
    footer.settle_copy(CopyOutcome::Failed);

    assert_eq!(footer.settled_at(), SETTLED_AT);
    assert_eq!(footer.response_text(), RESPONSE);
    assert_eq!(footer.relative_age(), "new adapter age");
}

#[test]
fn accessibility_facts_preserve_labels_and_machine_readable_timestamp() {
    let footer = policy();
    let expected = TurnFooterAccessibility {
        actions_label: TURN_ACTIONS_LABEL,
        copy_label: COPY_RESPONSE_LABEL,
        settled_timestamp: SETTLED_AT,
    };

    assert_eq!(footer.accessibility(), expected);
    assert_eq!(footer.accessibility().actions_label, "Turn actions");
    assert_eq!(footer.accessibility().copy_label, "Copy response");
    assert_eq!(footer.accessibility().settled_timestamp, SETTLED_AT);
}

#[test]
fn copy_input_does_not_request_a_clock_sample() {
    let mut footer = policy();

    assert!(matches!(
        footer.observe(TurnFooterInput::Copy),
        TurnFooterAction::CopyResponse { .. }
    ));
    assert_eq!(
        footer.observe(TurnFooterInput::PeriodicTick),
        TurnFooterAction::NoOp
    );
}
