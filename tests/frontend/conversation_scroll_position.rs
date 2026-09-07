//! Focused, dependency-free coverage for the transcript scroll-position policy.

#![allow(clippy::float_cmp)]

#[path = "../../modules/frontend/src/conversation_scroll_position.rs"]
mod conversation_scroll_position;

use std::collections::BTreeSet;

use conversation_scroll_position::{
    CONVERSATION_BASE_END_SPACE_PIXELS, CONVERSATION_TURN_TOP_INSET_PIXELS, ConversationScrollItem,
    ConversationScrollItemKind, ConversationSourceRef, conversation_aligned_scroll_top,
    conversation_aligned_scroll_top_with_inset, conversation_bottom_scroll_top,
    conversation_end_space_height, conversation_end_space_height_with_options,
    conversation_follow_tolerance, conversation_is_following, conversation_user_message_ids,
    conversation_user_message_with_source_reference, newest_conversation_user_message,
};

fn user(id: &str, ordinal: f64) -> ConversationScrollItem {
    ConversationScrollItem::user_message(id, ordinal, Vec::new())
}

fn other(id: &str, ordinal: f64) -> ConversationScrollItem {
    ConversationScrollItem::other(id, ordinal)
}

#[test]
fn durable_ids_keep_only_user_messages_and_deduplicate() {
    let items = vec![
        user("user-a", 1.0),
        other("assistant", 2.0),
        user("user-a", 3.0),
        other("user-looking", 4.0),
    ];

    let ids = conversation_user_message_ids(&items);

    assert_eq!(ids, BTreeSet::from([String::from("user-a")]));
}

#[test]
fn newest_message_filters_previous_and_non_user_items_then_breaks_ties() {
    let items = vec![
        user("older", 4.0),
        other("assistant-newest", 99.0),
        user("same-ordinal-a", 7.0),
        user("same-ordinal-b", 7.0),
        user("previous", 100.0),
    ];
    let previous = BTreeSet::from([String::from("previous")]);

    assert_eq!(
        newest_conversation_user_message(&items, &previous),
        Some(String::from("same-ordinal-b")),
    );
}

#[test]
fn newest_message_uses_total_order_for_malformed_ordinals() {
    let items = vec![user("finite", 1.0), user("infinity", f64::INFINITY)];
    let empty = BTreeSet::new();

    assert_eq!(
        newest_conversation_user_message(&items, &empty),
        Some(String::from("infinity"))
    );
}

#[test]
fn source_resolution_accepts_reference_or_event_id_and_ignores_non_users() {
    let items = vec![
        ConversationScrollItem::new(
            "assistant",
            1.0,
            ConversationScrollItemKind::Other,
            vec![ConversationSourceRef::with_event_id(
                "shared-ref",
                "event-a",
            )],
        ),
        ConversationScrollItem::user_message(
            "user-reference",
            2.0,
            vec![ConversationSourceRef::new("canonical-ref")],
        ),
        ConversationScrollItem::user_message(
            "user-event",
            3.0,
            vec![ConversationSourceRef::with_event_id("other-ref", "event-b")],
        ),
    ];

    assert_eq!(
        conversation_user_message_with_source_reference(&items, "shared-ref"),
        None
    );
    assert_eq!(
        conversation_user_message_with_source_reference(&items, "canonical-ref"),
        Some(String::from("user-reference"))
    );
    assert_eq!(
        conversation_user_message_with_source_reference(&items, "event-b"),
        Some(String::from("user-event"))
    );
    assert_eq!(
        conversation_user_message_with_source_reference(&items, "missing"),
        None
    );
}

#[test]
fn bottom_scroll_top_is_clamped_and_invalid_dimensions_are_safe() {
    assert_eq!(conversation_bottom_scroll_top(900.0, 300.0), 600.0);
    assert_eq!(conversation_bottom_scroll_top(100.0, 300.0), 0.0);
    assert_eq!(conversation_bottom_scroll_top(-100.0, 300.0), 0.0);
    assert_eq!(conversation_bottom_scroll_top(900.0, -300.0), 900.0);

    for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert_eq!(conversation_bottom_scroll_top(bad, 300.0), 0.0);
        assert_eq!(conversation_bottom_scroll_top(900.0, bad), 900.0);
    }
}

#[test]
fn follow_tolerance_has_a_floor_and_scales_with_viewport() {
    assert_eq!(conversation_follow_tolerance(0.0), 64.0);
    assert_eq!(conversation_follow_tolerance(1_000.0), 64.0);
    assert_eq!(conversation_follow_tolerance(2_000.0), 120.0);
    assert_eq!(conversation_follow_tolerance(-1_000.0), 64.0);
    for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert_eq!(conversation_follow_tolerance(bad), 64.0);
    }
}

#[test]
fn following_comparison_is_strict_at_the_exact_threshold() {
    // 1_000 - 400 - 536 = 64, exactly the floor tolerance.
    assert!(!conversation_is_following(536.0, 1_000.0, 400.0));
    assert!(conversation_is_following(536.001, 1_000.0, 400.0));
    assert!(conversation_is_following(600.0, 1_000.0, 400.0));
    assert!(!conversation_is_following(-20.0, 1_000.0, 400.0));

    for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert!(!conversation_is_following(bad, 1_000.0, 400.0));
        assert!(conversation_is_following(0.0, bad, 400.0));
        assert!(!conversation_is_following(0.0, 1_000.0, bad));
    }
}

#[test]
fn aligned_scroll_uses_default_inset_and_clamps_at_zero() {
    assert_eq!(CONVERSATION_TURN_TOP_INSET_PIXELS, 16.0);
    assert_eq!(conversation_aligned_scroll_top(100.0, 20.0, 200.0), 264.0);
    assert_eq!(conversation_aligned_scroll_top(100.0, 320.0, 200.0), 0.0);
    assert_eq!(
        conversation_aligned_scroll_top_with_inset(100.0, 20.0, 200.0, 8.0),
        272.0
    );
}

#[test]
fn aligned_scroll_sanitizes_invalid_geometry_and_overflow() {
    assert_eq!(conversation_aligned_scroll_top(-1.0, 20.0, 200.0), 164.0);
    assert_eq!(
        conversation_aligned_scroll_top(100.0, f64::NAN, 200.0),
        284.0
    );
    assert_eq!(
        conversation_aligned_scroll_top(f64::MAX, 0.0, f64::MAX),
        0.0
    );
    assert!(conversation_aligned_scroll_top(f64::MAX, 0.0, f64::MAX).is_finite());
}

#[test]
fn end_space_uses_default_base_and_inset_and_clamps_to_base() {
    assert_eq!(CONVERSATION_BASE_END_SPACE_PIXELS, 192.0);
    assert_eq!(conversation_end_space_height(600.0, 500.0, 100.0), 984.0);
    assert_eq!(conversation_end_space_height(100.0, 20.0, 500.0), 192.0);
    assert_eq!(
        conversation_end_space_height_with_options(600.0, 500.0, 100.0, 64.0, 8.0),
        992.0
    );
}

#[test]
fn end_space_sanitizes_invalid_geometry_and_overflow() {
    assert_eq!(conversation_end_space_height(-600.0, 500.0, 100.0), 384.0);
    assert_eq!(conversation_end_space_height(600.0, f64::NAN, 100.0), 484.0);
    assert_eq!(
        conversation_end_space_height(f64::MAX, 0.0, 0.0),
        f64::MAX - CONVERSATION_TURN_TOP_INSET_PIXELS
    );
    assert!(conversation_end_space_height(f64::MAX, f64::MAX, 0.0).is_finite());
    assert!(conversation_end_space_height(f64::MAX, f64::MAX, 0.0) >= 0.0);
}
