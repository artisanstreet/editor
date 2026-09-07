//! Focused, dependency-free coverage for conversation steering acknowledgement.

#[path = "../../modules/frontend/src/conversation_steering.rs"]
mod conversation_steering;

use conversation_steering::{
    ConversationLifecycle, ConversationSourceRef, ConversationSteeringItem,
    ConversationSteeringItemKind, ConversationSteeringTurn, conversation_steering_acknowledged,
};

fn source(reference: &str) -> ConversationSourceRef {
    ConversationSourceRef::new(reference)
}

fn source_with_event(reference: &str, event_id: &str) -> ConversationSourceRef {
    ConversationSourceRef::with_event_id(reference, event_id)
}

fn user(
    id: &str,
    ordinal: u64,
    run_id: Option<&str>,
    source_refs: Vec<ConversationSourceRef>,
) -> ConversationSteeringItem {
    ConversationSteeringItem::user_message(id, ordinal, run_id.map(str::to_owned), source_refs)
}

fn other(id: &str, ordinal: u64, run_id: Option<&str>) -> ConversationSteeringItem {
    match run_id {
        Some(run_id) => ConversationSteeringItem::other_for_run(id, ordinal, run_id),
        None => ConversationSteeringItem::other(id, ordinal),
    }
}

fn turn(run_id: &str, lifecycle: ConversationLifecycle) -> ConversationSteeringTurn {
    ConversationSteeringTurn::for_run(run_id, lifecycle)
}

#[test]
fn no_matching_user_message_is_not_acknowledged() {
    let items = vec![
        other("activity", 2, Some("run-a")),
        user("wrong-source", 3, Some("run-a"), vec![source("other")]),
    ];

    assert!(!conversation_steering_acknowledged(&items, &[], "target"));
}

#[test]
fn an_unbound_matching_user_message_is_acknowledged_immediately() {
    let items = vec![user("steer", 10, None, vec![source("command-1")])];
    let turns = vec![
        ConversationSteeringTurn::unbound(ConversationLifecycle::Active),
        turn("unrelated", ConversationLifecycle::Active),
    ];

    assert!(conversation_steering_acknowledged(
        &items,
        &turns,
        "command-1"
    ));
}

#[test]
fn matching_accepts_reference_or_event_id() {
    let reference_match = vec![user(
        "reference-steer",
        10,
        None,
        vec![source_with_event("command-1", "event-1")],
    )];
    let event_match = vec![user(
        "event-steer",
        10,
        None,
        vec![source_with_event("command-2", "event-2")],
    )];

    assert!(conversation_steering_acknowledged(
        &reference_match,
        &[],
        "command-1"
    ));
    assert!(conversation_steering_acknowledged(
        &event_match,
        &[],
        "event-2"
    ));
}

#[test]
fn a_non_user_item_with_matching_source_is_not_a_steer() {
    let items = vec![ConversationSteeringItem::new(
        "activity",
        10,
        Some(String::from("run-a")),
        vec![source("command-1")],
        ConversationSteeringItemKind::Other,
    )];

    assert!(!conversation_steering_acknowledged(
        &items,
        &[],
        "command-1"
    ));
}

#[test]
fn the_first_matching_user_candidate_wins() {
    let items = vec![
        user("first", 20, Some("run-a"), vec![source("command-1")]),
        user("duplicate", 10, None, vec![source("command-1")]),
    ];
    let turns = vec![turn("run-a", ConversationLifecycle::Active)];

    // The later unbound duplicate cannot replace the first bound candidate.
    assert!(!conversation_steering_acknowledged(
        &items,
        &turns,
        "command-1"
    ));
}

#[test]
fn every_settled_lifecycle_acknowledges_a_bound_run() {
    let settled = [
        ConversationLifecycle::Completed,
        ConversationLifecycle::Failed,
        ConversationLifecycle::Interrupted,
        ConversationLifecycle::Cancelled,
    ];

    for lifecycle in settled {
        let items = vec![user("steer", 10, Some("run-a"), vec![source("command-1")])];
        let turns = vec![turn("run-a", lifecycle)];

        assert!(
            conversation_steering_acknowledged(&items, &turns, "command-1"),
            "{lifecycle:?} must settle acknowledgement"
        );
        assert!(lifecycle.is_settled());
    }
}

#[test]
fn every_unsettled_lifecycle_keeps_a_bound_run_pending_without_later_work() {
    let unsettled = [
        ConversationLifecycle::Pending,
        ConversationLifecycle::Streaming,
        ConversationLifecycle::Active,
        ConversationLifecycle::Waiting,
    ];

    for lifecycle in unsettled {
        let items = vec![user("steer", 10, Some("run-a"), vec![source("command-1")])];
        let turns = vec![turn("run-a", lifecycle)];

        assert!(
            !conversation_steering_acknowledged(&items, &turns, "command-1"),
            "{lifecycle:?} must not settle acknowledgement"
        );
        assert!(!lifecycle.is_settled());
    }
}

#[test]
fn mixed_turns_require_every_turn_for_the_run_to_be_settled() {
    let items = vec![user("steer", 10, Some("run-a"), vec![source("command-1")])];
    let mixed_turns = vec![
        turn("run-a", ConversationLifecycle::Completed),
        turn("run-a", ConversationLifecycle::Active),
        turn("run-b", ConversationLifecycle::Completed),
    ];

    assert!(!conversation_steering_acknowledged(
        &items,
        &mixed_turns,
        "command-1"
    ));

    let settled_turns = vec![
        turn("run-a", ConversationLifecycle::Failed),
        turn("run-a", ConversationLifecycle::Interrupted),
        turn("run-b", ConversationLifecycle::Active),
    ];
    assert!(conversation_steering_acknowledged(
        &items,
        &settled_turns,
        "command-1"
    ));
}

#[test]
fn zero_matching_run_turns_waits_for_later_non_user_work() {
    let steer = user("steer", 10, Some("run-a"), vec![source("command-1")]);

    assert!(!conversation_steering_acknowledged(
        std::slice::from_ref(&steer),
        &[],
        "command-1"
    ));

    let items = vec![steer, other("replacement-work", 11, Some("run-b"))];
    assert!(conversation_steering_acknowledged(&items, &[], "command-1"));
}

#[test]
fn later_non_user_work_uses_strict_durable_ordinal_comparison() {
    let cases = [
        ("earlier", other("earlier", 9, None), false),
        ("equal", other("equal", 10, None), false),
        (
            "later-user",
            user("later-user", 11, Some("run-b"), vec![]),
            false,
        ),
        ("later-other", other("later-other", 11, None), true),
    ];

    for (label, candidate, expected) in cases {
        let items = vec![
            user("steer", 10, Some("run-a"), vec![source("command-1")]),
            candidate,
        ];
        assert_eq!(
            conversation_steering_acknowledged(&items, &[], "command-1"),
            expected,
            "{label} candidate has the wrong acknowledgement result"
        );
    }
}

#[test]
fn unrelated_replacement_run_can_acknowledge_after_the_steer() {
    let items = vec![
        user("steer", 10, Some("run-a"), vec![source("command-1")]),
        other("replacement-output", 12, Some("run-b")),
    ];
    let turns = vec![turn("run-a", ConversationLifecycle::Active)];

    // The fallback intentionally does not require the later work to retain
    // the steer's run id.
    assert!(conversation_steering_acknowledged(
        &items,
        &turns,
        "command-1"
    ));
}

#[test]
fn ordinal_comparison_is_durable_even_when_input_order_is_not_sorted() {
    let items = vec![
        other("later-in-sequence", 12, None),
        user("steer", 10, Some("run-a"), vec![source("command-1")]),
        other("earlier-in-sequence", 9, None),
    ];

    assert!(conversation_steering_acknowledged(&items, &[], "command-1"));
}
