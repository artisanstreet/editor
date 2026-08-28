//! Focused, dependency-free coverage for conversation turn navigation.

#[path = "../../modules/frontend/src/conversation_turn_navigator.rs"]
mod conversation_turn_navigator;

use conversation_turn_navigator::{
    ConversationItemKind, ConversationSnapshotInput, ConversationTurnInput, ConversationTurnMarker,
    ConversationTurnOffset, ConversationWindowMarkerInput, LoadedConversationItemInput,
    active_conversation_turn, active_conversation_turn_with_threshold, conversation_turn_floor,
    conversation_turn_markers,
};

fn turn(id: &str, ordinal: u64) -> ConversationTurnInput {
    ConversationTurnInput::new(id, ordinal)
}

fn user(id: &str, turn_id: &str, ordinal: u64, text: &str) -> LoadedConversationItemInput {
    LoadedConversationItemInput::user_message(id, turn_id, ordinal, text)
}

fn assistant(id: &str, turn_id: &str, ordinal: u64, text: &str) -> LoadedConversationItemInput {
    LoadedConversationItemInput::assistant_message(id, turn_id, ordinal, text)
}

fn other(id: &str, turn_id: &str, ordinal: u64, text: &str) -> LoadedConversationItemInput {
    LoadedConversationItemInput::other(id, turn_id, ordinal, text)
}

fn remote(id: &str, label: &str, ordinal: u64, turn_ordinal: u64) -> ConversationWindowMarkerInput {
    ConversationWindowMarkerInput::new(id, label, ordinal, turn_ordinal)
}

fn snapshot(
    turns: Vec<ConversationTurnInput>,
    items: Vec<LoadedConversationItemInput>,
    window_markers: Option<Vec<ConversationWindowMarkerInput>>,
) -> ConversationSnapshotInput {
    ConversationSnapshotInput::new(turns, items, window_markers)
}

fn marker(
    id: &str,
    label: &str,
    ordinal: u64,
    turn_ordinal: Option<u64>,
) -> ConversationTurnMarker {
    ConversationTurnMarker {
        id: id.to_owned(),
        label: label.to_owned(),
        ordinal,
        turn_ordinal,
    }
}

#[test]
fn loaded_user_messages_win_overlap_and_remote_floor_is_strict() {
    let snapshot = snapshot(
        vec![turn("loaded-late", 20), turn("loaded-early", 10)],
        vec![
            user("same", "loaded-late", 40, "  loaded\nvalue  "),
            assistant("assistant", "loaded-late", 41, "not a marker"),
            other("activity", "loaded-late", 42, "also not a marker"),
        ],
        Some(vec![
            // The remote copy loses to the loaded item even with a different
            // ordinal and label.
            remote("same", "remote copy", 1, 0),
            remote("older", "older remote", 2, 9),
            // The floor is inclusive for loaded data, so remote equality is
            // excluded: only strictly older turns are reachable remotely.
            remote("at-floor", "floor remote", 3, 10),
            remote("newer", "newer remote", 4, 20),
        ]),
    );

    assert_eq!(
        conversation_turn_markers(&snapshot),
        vec![
            marker("older", "older remote", 2, Some(9)),
            marker("same", "loaded value", 40, Some(20)),
        ]
    );
}

#[test]
fn assistant_and_other_items_never_become_markers() {
    let snapshot = snapshot(
        vec![turn("one", 1), turn("two", 2)],
        vec![
            assistant("assistant", "one", 1, "assistant"),
            other("other", "two", 2, "other"),
            user("user-one", "one", 3, "first"),
            user("user-two", "two", 4, "second"),
        ],
        None,
    );

    assert_eq!(
        conversation_turn_markers(&snapshot)
            .into_iter()
            .map(|marker| marker.id)
            .collect::<Vec<_>>(),
        vec!["user-one", "user-two"]
    );
}

#[test]
fn whitespace_is_collapsed_trimmed_and_unicode_labels_are_scalar_bounded() {
    let long_unicode = "😀".repeat(121);
    let snapshot = snapshot(
        vec![turn("one", 1), turn("two", 2)],
        vec![
            user(
                "one",
                "one",
                1,
                "\u{FEFF}\t  hello\u{2003}\nworld\u{00A0}  ",
            ),
            user("two", "two", 2, &long_unicode),
        ],
        None,
    );

    let markers = conversation_turn_markers(&snapshot);
    assert_eq!(markers[0].label, "hello world");
    assert_eq!(markers[1].label, "😀".repeat(120));
    assert_eq!(markers[1].label.chars().count(), 120);
    assert!(markers[1].label.is_char_boundary(markers[1].label.len()));
}

#[test]
fn empty_and_single_reachable_marker_lists_are_suppressed() {
    let empty = snapshot(vec![], vec![], None);
    assert!(conversation_turn_markers(&empty).is_empty());

    let single = snapshot(
        vec![turn("one", 1)],
        vec![user("one", "one", 1, "only")],
        None,
    );
    assert!(conversation_turn_markers(&single).is_empty());

    let blank_with_one_real = snapshot(
        vec![turn("one", 1), turn("two", 2)],
        vec![
            user("blank", "one", 1, "\n\t"),
            user("real", "two", 2, "real"),
        ],
        None,
    );
    assert!(conversation_turn_markers(&blank_with_one_real).is_empty());
}

#[test]
fn markers_sort_by_ordinal_then_id_and_preserve_turn_ordinals() {
    let snapshot = snapshot(
        vec![turn("known", 7)],
        vec![
            // A missing owning turn is represented as None rather than
            // falling back to the item's ordinal.
            user("missing", "unknown", 5, "missing turn"),
            user("z", "known", 5, "loaded z"),
        ],
        Some(vec![
            remote("a", "remote a", 5, 2),
            remote("m", "remote m", 2, 1),
        ]),
    );

    assert_eq!(conversation_turn_floor(&snapshot), Some(7));
    assert_eq!(
        conversation_turn_markers(&snapshot),
        vec![
            marker("m", "remote m", 2, Some(1)),
            marker("a", "remote a", 5, Some(2)),
            marker("missing", "missing turn", 5, None),
            marker("z", "loaded z", 5, Some(7)),
        ]
    );
}

#[test]
fn no_loaded_floor_allows_remote_markers_and_missing_window_is_empty() {
    let no_window = snapshot(vec![], vec![], None);
    assert!(conversation_turn_markers(&no_window).is_empty());

    let no_floor = snapshot(
        vec![],
        vec![],
        Some(vec![
            remote("old", "old", 1, 100),
            remote("new", "new", 2, 101),
        ]),
    );
    assert_eq!(
        conversation_turn_markers(&no_floor),
        vec![
            marker("old", "old", 1, Some(100)),
            marker("new", "new", 2, Some(101)),
        ]
    );
}

#[test]
fn active_turn_uses_first_at_top_and_last_passed_offset() {
    let offsets = [
        ConversationTurnOffset::new("first", 140.0),
        ConversationTurnOffset::new("second", 100.0),
        ConversationTurnOffset::new("third", 96.0),
        ConversationTurnOffset::new("fourth", 80.0),
        ConversationTurnOffset::new("fifth", 120.0),
    ];

    // The default threshold is 96, and the input order is already transcript
    // order. "fourth" is the last passed marker even though the next offset
    // has moved below it in viewport coordinates.
    assert_eq!(active_conversation_turn(&offsets), Some("fourth"));

    let at_top = [
        ConversationTurnOffset::new("first", 97.0),
        ConversationTurnOffset::new("second", 140.0),
    ];
    assert_eq!(active_conversation_turn(&at_top), Some("first"));
}

#[test]
fn active_turn_includes_exact_threshold_and_supports_a_passed_threshold() {
    let offsets = [
        ConversationTurnOffset::new("first", 220.0),
        ConversationTurnOffset::new("second", 150.0),
        ConversationTurnOffset::new("third", 100.0),
        ConversationTurnOffset::new("fourth", 99.0),
    ];

    let exact_threshold = [
        ConversationTurnOffset::new("first", 220.0),
        ConversationTurnOffset::new("second", 150.0),
        ConversationTurnOffset::new("later", 151.0),
    ];
    assert_eq!(
        active_conversation_turn_with_threshold(&exact_threshold, 150.0),
        Some("second")
    );
    assert_eq!(
        active_conversation_turn_with_threshold(&offsets, 99.0),
        Some("fourth")
    );
    assert_eq!(
        active_conversation_turn_with_threshold(&offsets, 98.0),
        Some("first")
    );
    assert_eq!(active_conversation_turn(&[]), None);
}

#[test]
fn item_kind_remains_typed_without_protocol_dependencies() {
    let item = LoadedConversationItemInput::new(
        "id",
        "turn",
        4,
        ConversationItemKind::AssistantMessage,
        "reply",
    );
    assert_eq!(item.kind, ConversationItemKind::AssistantMessage);
}
