//! Direct dependency-free coverage for the setup-label transition policy.

#![forbid(unsafe_code)]

#[path = "../../modules/frontend/src/setup_label_transition_policy.rs"]
mod setup_label_transition_policy;

use setup_label_transition_policy::{
    DEFAULT_TEXT_SWAP_DURATION_MS, DisplayedValue, SetupLabelDisplayedValue,
    SetupLabelPendingTimer, SetupLabelTimerToken, SetupLabelTransition, SetupLabelTransitionAction,
    SetupLabelTransitionClass, SetupLabelTransitionController, SetupLabelTransitionState,
    SetupLabelValue, TEXT_SWAP_ENTER_START_CLASS, TEXT_SWAP_EXIT_CLASS,
    effective_text_swap_duration_ms,
};

fn value(label: &str, email: Option<&str>) -> DisplayedValue {
    SetupLabelDisplayedValue::new(label, email.map(str::to_owned))
}

fn label_only(label: &str) -> DisplayedValue {
    SetupLabelDisplayedValue::label_only(label)
}

fn actions(transition: SetupLabelTransition) -> Vec<SetupLabelTransitionAction> {
    transition.into_actions()
}

fn attach(controller: &mut SetupLabelTransitionController) {
    assert!(controller.attach().is_empty());
}

fn current_token(controller: &SetupLabelTransitionController) -> SetupLabelTimerToken {
    controller
        .pending_timer()
        .expect("a changed attached value has a pending timer")
        .token
}

fn duration_is(actual: f64, expected: f64) -> bool {
    actual.to_bits() == expected.to_bits()
}

fn assert_duration(actual: f64, expected: f64) {
    assert_eq!(actual.to_bits(), expected.to_bits());
}

#[test]
fn first_observation_sets_rendered_state_without_class_or_timer_actions() {
    let mut controller = SetupLabelTransitionController::new();
    attach(&mut controller);
    let first = value("Preparing", Some("person@example.test"));

    assert_eq!(
        actions(controller.observe(first.clone(), Some(42.0))),
        vec![SetupLabelTransitionAction::SetRenderedValue {
            value: first.clone(),
        }]
    );
    assert_eq!(controller.rendered_value(), Some(&first));
    assert_eq!(
        controller.rendered_text().as_deref(),
        Some("Preparing person@example.test")
    );
    assert!(controller.pending_timer().is_none());
    assert_eq!(
        controller.state(),
        &SetupLabelTransitionState::Displayed { value: first }
    );
    assert_eq!(controller.next_timer_token(), 0);
}

#[test]
fn unchanged_label_and_email_pair_is_a_no_op() {
    let mut controller = SetupLabelTransitionController::new();
    let first = value("Preparing", Some("person@example.test"));
    assert_eq!(actions(controller.observe(first.clone(), None)).len(), 1);

    assert!(controller.observe(first, Some(999.0)).is_empty());
    assert_eq!(controller.next_timer_token(), 0);
    assert!(controller.pending_timer().is_none());
}

#[test]
fn changed_value_without_an_attached_element_replaces_immediately() {
    let mut controller = SetupLabelTransitionController::new();
    let first = label_only("One");
    let second = value("Two", Some("two@example.test"));
    let _ = controller.observe(first, Some(7.0));

    assert_eq!(
        actions(controller.observe(second.clone(), Some(7.0))),
        vec![SetupLabelTransitionAction::SetRenderedValue {
            value: second.clone(),
        }]
    );
    assert_eq!(controller.rendered_value(), Some(&second));
    assert!(controller.pending_timer().is_none());
    assert_eq!(controller.next_timer_token(), 0);
}

#[test]
fn label_only_and_email_only_changes_are_detected_as_pair_changes() {
    let mut controller = SetupLabelTransitionController::new();
    attach(&mut controller);
    let _ = controller.observe(value("Ready", Some("a@example.test")), None);

    let label_change = value("Working", Some("a@example.test"));
    let label_actions = actions(controller.observe(label_change.clone(), Some(12.0)));
    assert_eq!(label_actions.len(), 2);
    assert!(matches!(
        label_actions[0],
        SetupLabelTransitionAction::AddClass {
            class: SetupLabelTransitionClass::Exit
        }
    ));
    assert!(matches!(
        label_actions[1],
        SetupLabelTransitionAction::ScheduleTimer { delay_ms, .. } if duration_is(delay_ms, 12.0)
    ));
    let label_token = current_token(&controller);
    let _ = controller.fire_timer(label_token);

    let email_change = value("Working", Some("b@example.test"));
    let email_actions = actions(controller.observe(email_change, Some(13.0)));
    assert!(matches!(
        email_actions[0],
        SetupLabelTransitionAction::AddClass {
            class: SetupLabelTransitionClass::Exit
        }
    ));
    assert!(matches!(
        email_actions[1],
        SetupLabelTransitionAction::ScheduleTimer { delay_ms, .. } if duration_is(delay_ms, 13.0)
    ));
    assert_ne!(current_token(&controller), label_token);
}

#[test]
fn present_empty_email_keeps_the_ascii_separator_space() {
    let empty_email = SetupLabelDisplayedValue::new("Label", Some(String::new()));
    assert_eq!(empty_email.rendered_text(), "Label ");
    assert_eq!(
        SetupLabelDisplayedValue::with_email("Label", "").email(),
        Some("")
    );

    let no_email = label_only("Label");
    assert_eq!(no_email.rendered_text(), "Label");
}

#[test]
fn css_duration_matches_finite_nonzero_and_javascript_fallback_cases() {
    assert_duration(
        effective_text_swap_duration_ms(None),
        DEFAULT_TEXT_SWAP_DURATION_MS,
    );
    assert_duration(
        effective_text_swap_duration_ms(Some(0.0)),
        DEFAULT_TEXT_SWAP_DURATION_MS,
    );
    assert_duration(
        effective_text_swap_duration_ms(Some(-0.0)),
        DEFAULT_TEXT_SWAP_DURATION_MS,
    );
    assert_duration(
        effective_text_swap_duration_ms(Some(f64::NAN)),
        DEFAULT_TEXT_SWAP_DURATION_MS,
    );
    assert_duration(
        effective_text_swap_duration_ms(Some(f64::INFINITY)),
        DEFAULT_TEXT_SWAP_DURATION_MS,
    );
    assert_duration(effective_text_swap_duration_ms(Some(275.5)), 275.5);
    assert_duration(effective_text_swap_duration_ms(Some(-12.25)), -12.25);
}

#[test]
fn attached_change_schedules_the_effective_duration_and_preserves_negative_delay() {
    let mut controller = SetupLabelTransitionController::new();
    attach(&mut controller);
    let _ = controller.observe(label_only("One"), None);

    let fallback = actions(controller.observe(label_only("Two"), None));
    assert!(matches!(
        fallback[1],
        SetupLabelTransitionAction::ScheduleTimer { delay_ms, token }
            if duration_is(delay_ms, DEFAULT_TEXT_SWAP_DURATION_MS) && token.get() == 1
    ));
    let first_token = current_token(&controller);
    let _ = controller.fire_timer(first_token);

    let negative = actions(controller.observe(label_only("Three"), Some(-4.5)));
    assert!(matches!(
        negative[1],
        SetupLabelTransitionAction::ScheduleTimer { delay_ms, .. } if duration_is(delay_ms, -4.5)
    ));
    let negative_token = current_token(&controller);
    assert_eq!(
        actions(controller.cancel_pending_timer()),
        vec![SetupLabelTransitionAction::CancelTimer {
            token: negative_token,
        }]
    );
    assert!(controller.on_timer(negative_token).is_empty());
}

#[test]
fn current_timer_firing_has_exact_render_tick_class_layout_order() {
    let mut controller = SetupLabelTransitionController::new();
    attach(&mut controller);
    let first = label_only("Old");
    let next = value("New", Some("new@example.test"));
    let _ = controller.observe(first, None);
    let scheduled = actions(controller.observe(next.clone(), Some(1.0)));
    let SetupLabelTransitionAction::ScheduleTimer { token, .. } = scheduled[1] else {
        panic!("the second action must schedule the timer");
    };

    assert_eq!(
        actions(controller.fire_timer(token)),
        vec![
            SetupLabelTransitionAction::SetRenderedValue {
                value: next.clone(),
            },
            SetupLabelTransitionAction::RequestTick,
            SetupLabelTransitionAction::RemoveClass {
                class: SetupLabelTransitionClass::Exit,
            },
            SetupLabelTransitionAction::AddClass {
                class: SetupLabelTransitionClass::EnterStart,
            },
            SetupLabelTransitionAction::RequestLayoutRead,
            SetupLabelTransitionAction::RemoveClass {
                class: SetupLabelTransitionClass::EnterStart,
            },
        ]
    );
    assert_eq!(controller.rendered_value(), Some(&next));
    assert!(controller.pending_timer().is_none());
}

#[test]
fn supersession_cancels_before_the_new_decision_and_allocates_a_new_token() {
    let mut controller = SetupLabelTransitionController::new();
    attach(&mut controller);
    let _ = controller.observe(label_only("Old"), None);
    let first = actions(controller.observe(label_only("First"), Some(10.0)));
    let first_token = current_token(&controller);

    let second = value("Second", Some("second@example.test"));
    let superseded = actions(controller.observe(second.clone(), Some(20.0)));
    assert!(matches!(
        superseded[0],
        SetupLabelTransitionAction::CancelTimer { token } if token == first_token
    ));
    assert!(matches!(
        superseded[1],
        SetupLabelTransitionAction::AddClass {
            class: SetupLabelTransitionClass::Exit
        }
    ));
    assert!(matches!(
        superseded[2],
        SetupLabelTransitionAction::ScheduleTimer { token, delay_ms }
            if token.get() > first_token.get() && duration_is(delay_ms, 20.0)
    ));
    assert_eq!(controller.rendered_text().as_deref(), Some("Old"));
    assert_eq!(
        controller.pending_timer().map(|timer| &timer.value),
        Some(&second)
    );
    assert_eq!(first.len(), 2);
}

#[test]
fn stale_timer_fires_are_empty_and_cannot_mutate_state() {
    let mut controller = SetupLabelTransitionController::new();
    attach(&mut controller);
    let _ = controller.observe(label_only("Old"), None);
    let _ = controller.observe(label_only("First"), Some(10.0));
    let stale = current_token(&controller);
    let _ = controller.observe(label_only("Second"), Some(20.0));
    let current = current_token(&controller);
    let snapshot = controller.clone();

    assert!(controller.fire_timer(stale).is_empty());
    assert_eq!(controller, snapshot);
    assert_eq!(controller.fire_timer(current).len(), 6);
    assert_eq!(controller.rendered_text().as_deref(), Some("Second"));
}

#[test]
fn observing_the_old_value_while_pending_cancels_then_becomes_a_no_op() {
    let mut controller = SetupLabelTransitionController::new();
    attach(&mut controller);
    let old = label_only("Old");
    let _ = controller.observe(old.clone(), None);
    let _ = controller.observe(label_only("New"), Some(10.0));
    let stale = current_token(&controller);

    assert_eq!(
        actions(controller.observe(old.clone(), Some(99.0))),
        vec![SetupLabelTransitionAction::CancelTimer { token: stale }]
    );
    assert_eq!(
        controller.state(),
        &SetupLabelTransitionState::Displayed { value: old }
    );
}

#[test]
fn detach_and_unmount_cancel_pending_timer_and_late_callbacks_stay_inert() {
    for unmount in [false, true] {
        let mut controller = SetupLabelTransitionController::new();
        attach(&mut controller);
        let _ = controller.observe(label_only("Old"), None);
        let _ = controller.observe(label_only("New"), Some(10.0));
        let token = current_token(&controller);

        let cancelled = if unmount {
            actions(controller.unmount())
        } else {
            actions(controller.detach())
        };
        assert_eq!(
            cancelled,
            vec![SetupLabelTransitionAction::CancelTimer { token }]
        );
        assert!(!controller.element_attached());
        assert!(controller.pending_timer().is_none());
        assert!(controller.fire_timer(token).is_empty());
        assert_eq!(controller.rendered_text().as_deref(), Some("Old"));
    }
}

#[test]
fn reattachment_allows_a_later_changed_observation_to_animate() {
    let mut controller = SetupLabelTransitionController::new();
    let _ = controller.observe(label_only("Old"), None);
    let _ = controller.detach();
    let _ = controller.observe(label_only("Detached"), None);

    attach(&mut controller);
    let next = label_only("Attached");
    let transition = actions(controller.observe(next.clone(), Some(33.0)));
    assert_eq!(transition.len(), 2);
    assert!(matches!(
        transition[0],
        SetupLabelTransitionAction::AddClass {
            class: SetupLabelTransitionClass::Exit
        }
    ));
    let token = current_token(&controller);
    let _ = controller.fire_timer(token);
    assert_eq!(controller.rendered_value(), Some(&next));
}

#[test]
fn unicode_and_whitespace_are_owned_and_rendered_without_normalization() {
    let original = value("  名前\n— café 🚀  ", Some("  邮箱\t@example.test  "));
    assert_eq!(
        original.rendered_text(),
        "  名前\n— café 🚀     邮箱\t@example.test  "
    );

    let mut controller = SetupLabelTransitionController::new();
    let _ = controller.observe(original.clone(), None);
    let changed = value("\u{2003}新しい\u{00a0}ラベル", Some("почта@example.test"));
    let _ = controller.observe(changed.clone(), None);
    assert_eq!(controller.rendered_value(), Some(&changed));
    assert_eq!(
        controller.rendered_text().as_deref(),
        Some("\u{2003}新しい\u{00a0}ラベル почта@example.test")
    );
}

#[test]
fn state_and_action_helpers_expose_typed_values_without_runtime_facilities() {
    assert_eq!(
        SetupLabelTransitionClass::Exit.as_str(),
        TEXT_SWAP_EXIT_CLASS
    );
    assert_eq!(
        SetupLabelTransitionClass::EnterStart.as_str(),
        TEXT_SWAP_ENTER_START_CLASS
    );
    let alias_value: SetupLabelValue = label_only("alias");
    assert_eq!(alias_value.label(), "alias");
    let pending = SetupLabelPendingTimer {
        token: SetupLabelTimerToken::new(7),
        value: label_only("pending"),
    };
    let waiting = SetupLabelTransitionState::WaitingForTimer {
        rendered: label_only("rendered"),
        pending: pending.clone(),
    };
    assert!(waiting.is_initialized());
    assert!(waiting.is_waiting_for_timer());
    assert_eq!(waiting.pending_timer(), Some(&pending));
    assert_eq!(
        waiting
            .rendered_value()
            .map(SetupLabelDisplayedValue::label),
        Some("rendered")
    );
    assert!(!SetupLabelTransitionState::Uninitialized.is_initialized());
    let transition = SetupLabelTransition::new(vec![SetupLabelTransitionAction::RequestTick]);
    assert_eq!(
        transition.actions(),
        &[SetupLabelTransitionAction::RequestTick]
    );
    assert_eq!(transition.len(), 1);
    assert!(!transition.is_empty());
}

#[test]
fn default_controller_matches_new_and_detach_is_idempotent() {
    let mut default = SetupLabelTransitionController::default();
    assert_eq!(default, SetupLabelTransitionController::new());
    assert!(default.set_element_attached(true).is_empty());
    assert!(default.element_attached());
    assert!(default.set_element_attached(false).is_empty());
    assert!(default.detach_element().is_empty());
    assert!(default.detach_element().is_empty());
    assert!(!default.element_attached());
}
