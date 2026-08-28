//! Exhaustive focused tests for the durable notification decision policy.

#[path = "../../modules/frontend/src/notification_events.rs"]
mod notification_events;

use notification_events::{
    EventEnvelope, EventPayload, InteractionState, RunLifecycleState, SystemNotification,
    SystemNotificationCategory, SystemNotificationContext, SystemNotificationDecision,
    is_notifiable_event, summarize, system_notification_decision_for,
};

const THREAD_ID: &str = "thread-42";
const ROUTE_PATH: &str = "/workspace/project/thread-42";

fn context<'a>(
    active_thread_id: Option<&'a str>,
    focused: bool,
    route_path: &'a str,
    thread_title: Option<&'a str>,
) -> SystemNotificationContext<'a> {
    SystemNotificationContext::new(active_thread_id, focused, route_path, thread_title)
}

fn run_event<'a>(
    thread_id: &'a str,
    run_id: Option<&'a str>,
    state: RunLifecycleState,
) -> EventEnvelope<'a> {
    EventEnvelope::new(thread_id, EventPayload::RunLifecycle { state }).with_run_id(run_id)
}

fn approval_event<'a>(
    thread_id: &'a str,
    approval_id: &'a str,
    state: InteractionState,
    description: &'a str,
) -> EventEnvelope<'a> {
    EventEnvelope::new(
        thread_id,
        EventPayload::ApprovalInteraction {
            approval_id,
            description,
            state,
        },
    )
}

fn question_event<'a>(
    thread_id: &'a str,
    question_id: &'a str,
    state: InteractionState,
    text: &'a str,
) -> EventEnvelope<'a> {
    EventEnvelope::new(
        thread_id,
        EventPayload::QuestionInteraction {
            question_id,
            text,
            state,
        },
    )
}

fn expected_notification(
    body: &str,
    category: SystemNotificationCategory,
    id: &str,
    title: &str,
) -> SystemNotification {
    SystemNotification {
        body: body.to_owned(),
        category,
        id: id.to_owned(),
        route_path: ROUTE_PATH.to_owned(),
        title: title.to_owned(),
    }
}

#[test]
fn every_lifecycle_state_is_notifiable_but_only_terminal_outcomes_show() {
    let cases = [
        (
            RunLifecycleState::Queued,
            SystemNotificationDecision::Ignore,
        ),
        (
            RunLifecycleState::Running,
            SystemNotificationDecision::Ignore,
        ),
        (
            RunLifecycleState::Waiting,
            SystemNotificationDecision::Ignore,
        ),
        (
            RunLifecycleState::Interrupted,
            SystemNotificationDecision::Ignore,
        ),
        (
            RunLifecycleState::Completed,
            SystemNotificationDecision::Show {
                notification: expected_notification(
                    "Finished.",
                    SystemNotificationCategory::RunCompleted,
                    "run:run-7",
                    "Thread title",
                ),
            },
        ),
        (
            RunLifecycleState::Cancelled,
            SystemNotificationDecision::Ignore,
        ),
        (
            RunLifecycleState::Failed,
            SystemNotificationDecision::Show {
                notification: expected_notification(
                    "The run failed.",
                    SystemNotificationCategory::RunFailed,
                    "run:run-7",
                    "Thread title",
                ),
            },
        ),
        (
            RunLifecycleState::Closed,
            SystemNotificationDecision::Ignore,
        ),
    ];
    let context = context(None, false, ROUTE_PATH, Some("Thread title"));

    for (state, expected) in cases {
        let envelope = run_event(THREAD_ID, Some("run-7"), state);
        assert!(is_notifiable_event(&envelope));
        assert_eq!(
            system_notification_decision_for(&envelope, &context),
            expected,
            "unexpected decision for {state:?}"
        );
    }
}

#[test]
fn requested_interactions_show_exact_category_and_identity() {
    let context = context(None, false, ROUTE_PATH, Some("Thread title"));
    let approval = approval_event(
        THREAD_ID,
        "approval-9",
        InteractionState::Requested,
        "  git\n push\torigin\u{2003}main  ",
    );
    let question = question_event(
        THREAD_ID,
        "question-3",
        InteractionState::Requested,
        "Which\nbranch?",
    );

    assert!(is_notifiable_event(&approval));
    assert!(is_notifiable_event(&question));
    assert_eq!(
        system_notification_decision_for(&approval, &context),
        SystemNotificationDecision::Show {
            notification: expected_notification(
                "Needs approval — git push origin main",
                SystemNotificationCategory::Approval,
                "approval:approval-9",
                "Thread title",
            ),
        }
    );
    assert_eq!(
        system_notification_decision_for(&question, &context),
        SystemNotificationDecision::Show {
            notification: expected_notification(
                "Has a question — Which branch?",
                SystemNotificationCategory::Question,
                "question:question-3",
                "Thread title",
            ),
        }
    );
}

#[test]
fn category_values_keep_the_exact_typescript_strings() {
    let cases = [
        (SystemNotificationCategory::Approval, "approval"),
        (SystemNotificationCategory::Question, "question"),
        (SystemNotificationCategory::RunCompleted, "run_completed"),
        (SystemNotificationCategory::RunFailed, "run_failed"),
    ];

    for (category, expected) in cases {
        assert_eq!(category.as_str(), expected);
    }
}

#[test]
fn completed_run_uses_run_id_and_falls_back_to_thread_id() {
    let context = context(None, false, ROUTE_PATH, Some("Thread title"));
    let with_run_id = run_event(
        THREAD_ID,
        Some("run-explicit"),
        RunLifecycleState::Completed,
    );
    let without_run_id = run_event(THREAD_ID, None, RunLifecycleState::Completed);

    let SystemNotificationDecision::Show {
        notification: explicit,
    } = system_notification_decision_for(&with_run_id, &context)
    else {
        panic!("completed run with identity should show");
    };
    let SystemNotificationDecision::Show {
        notification: fallback,
    } = system_notification_decision_for(&without_run_id, &context)
    else {
        panic!("completed run without identity should show");
    };

    assert_eq!(explicit.id, "run:run-explicit");
    assert_eq!(fallback.id, "run:thread-42");
}

#[test]
fn focused_active_thread_suppresses_only_shows() {
    let event = run_event(THREAD_ID, Some("run-7"), RunLifecycleState::Completed);
    let cases = [
        (Some(THREAD_ID), true, false),
        (Some("other-thread"), true, true),
        (None, true, true),
        (Some(THREAD_ID), false, true),
        (Some("other-thread"), false, true),
        (None, false, true),
    ];

    for (active_thread_id, focused, should_show) in cases {
        let context = context(active_thread_id, focused, ROUTE_PATH, Some("Thread title"));
        assert_eq!(
            matches!(
                system_notification_decision_for(&event, &context),
                SystemNotificationDecision::Show { .. }
            ),
            should_show,
            "focus={focused}, active_thread_id={active_thread_id:?}"
        );
    }
}

#[test]
fn title_fallback_and_route_path_are_preserved_exactly() {
    let route = "opaque://route/with spaces?raw=%2F";
    let no_title = SystemNotificationContext::new(None, false, route, None);
    let empty_title = SystemNotificationContext::new(None, false, route, Some(""));
    let event = run_event("thread-route", Some("run-route"), RunLifecycleState::Failed);

    let SystemNotificationDecision::Show {
        notification: fallback,
    } = system_notification_decision_for(&event, &no_title)
    else {
        panic!("failed run should show");
    };
    assert_eq!(fallback.title, "Artisan");
    assert_eq!(fallback.route_path, route);

    let SystemNotificationDecision::Show {
        notification: explicit,
    } = system_notification_decision_for(&event, &empty_title)
    else {
        panic!("failed run should show with an empty title");
    };
    assert_eq!(explicit.title, "");
    assert_eq!(explicit.route_path, route);
}

#[test]
fn summary_collapses_unicode_whitespace_and_trims_edges() {
    let text =
        "\u{FEFF}\t\nfirst\u{00A0}\u{2003}\u{2028}\u{2029}\u{202F}\u{205F}\u{3000}second\r\n ";

    assert_eq!(summarize(text), "first second");
    // U+0085 is not part of ECMAScript /\\s, so the source policy retains it.
    assert_eq!(summarize("left\u{0085}right"), "left\u{0085}right");
}

#[test]
fn summary_keeps_exactly_120_scalars_and_truncates_the_121st() {
    let exactly_120 = "a".repeat(120);
    let too_long = "a".repeat(121);

    assert_eq!(summarize(&exactly_120), exactly_120);
    assert_eq!(summarize(&too_long), format!("{}…", "a".repeat(119)));
    assert_eq!(summarize(&too_long).chars().count(), 120);
}

#[test]
fn summary_trims_a_collapsed_separator_at_the_truncation_boundary() {
    let text = format!("{} tail", "a".repeat(118));

    assert_eq!(summarize(&text), format!("{}…", "a".repeat(118)));
    assert_eq!(summarize(&text).chars().count(), 119);
}

#[test]
fn summary_counts_unicode_scalars_without_splitting_utf8() {
    let exactly_120 = "😀".repeat(120);
    let too_long = "😀".repeat(121);
    let summary = summarize(&too_long);

    assert_eq!(summarize(&exactly_120), exactly_120);
    assert_eq!(summary, format!("{}…", "😀".repeat(119)));
    assert_eq!(summary.chars().count(), 120);
    assert!(std::str::from_utf8(summary.as_bytes()).is_ok());
}

#[test]
fn resolved_interactions_revoke_even_when_the_active_thread_is_focused() {
    let context = context(Some(THREAD_ID), true, ROUTE_PATH, Some("Thread title"));
    let approval = approval_event(
        THREAD_ID,
        "approval-9",
        InteractionState::Resolved,
        "already answered",
    );
    let question = question_event(
        THREAD_ID,
        "question-3",
        InteractionState::Resolved,
        "already answered",
    );

    assert!(is_notifiable_event(&approval));
    assert!(is_notifiable_event(&question));
    assert_eq!(
        system_notification_decision_for(&approval, &context),
        SystemNotificationDecision::Revoke {
            id: "approval:approval-9".to_owned(),
        }
    );
    assert_eq!(
        system_notification_decision_for(&question, &context),
        SystemNotificationDecision::Revoke {
            id: "question:question-3".to_owned(),
        }
    );
}

#[test]
fn irrelevant_events_are_never_notifiable_or_actionable() {
    let envelope =
        EventEnvelope::new(THREAD_ID, EventPayload::Irrelevant).with_run_id(Some("run-ignored"));

    assert!(!is_notifiable_event(&envelope));
    assert_eq!(
        system_notification_decision_for(
            &envelope,
            &context(Some(THREAD_ID), false, ROUTE_PATH, Some("Thread title")),
        ),
        SystemNotificationDecision::Ignore
    );
}
