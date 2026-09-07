//! Behavioral coverage for the native thread command list's selection and
//! navigation view model.
//!
//! Every expectation below traces to INVENTORY §6.3 :496–510 and the pinned
//! `bits-ui@2.18.1` command engine the palette composes:
//! `routes/components/command-menu.svelte` plus
//! `dist/bits/command/command.svelte.js`.

use artisan_domain::{ProjectId, ThreadId};
use artisan_frontend::thread_list_selection::{
    EnterComposition, ListKey, ThreadActivationIntent, ThreadListGroup, ThreadListSelection,
    ThreadRow,
};

/// Parses a fixture thread identity.
fn thread(label: &str) -> ThreadId {
    ThreadId::parse(format!("t-{label}")).expect("fixture thread id is valid")
}

/// Parses a fixture project identity.
fn project(label: &str) -> ProjectId {
    ProjectId::parse(format!("p-{label}")).expect("fixture project id is valid")
}

/// Builds one group with the same shape the palette renders.
fn group(project: Option<ProjectId>, rows: Vec<ThreadRow>) -> ThreadListGroup {
    ThreadListGroup::new(project, rows)
}

/// A representative palette list: two project sections plus an unassigned
/// section holding a disabled row first, mirroring
/// `ProjectScopedThreadGroups` order (projects in map order, unassigned last).
fn palette_groups() -> Vec<ThreadListGroup> {
    vec![
        group(
            Some(project("1")),
            vec![
                ThreadRow::enabled(thread("a")),
                ThreadRow::enabled(thread("b")),
                ThreadRow::disabled(thread("x")),
                ThreadRow::enabled(thread("c")),
            ],
        ),
        group(
            Some(project("2")),
            vec![
                ThreadRow::disabled(thread("y")),
                ThreadRow::enabled(thread("d")),
            ],
        ),
        group(None, vec![ThreadRow::enabled(thread("e"))]),
    ]
}

#[test]
fn mount_selects_the_first_enabled_row() {
    let groups = palette_groups();

    let mut selection = ThreadListSelection::new();
    selection.mount(&groups);

    assert_eq!(selection.selected_thread(&groups), Some(&thread("a")));
}

#[test]
fn mount_skips_leading_disabled_rows() {
    let groups = vec![group(
        None,
        vec![
            ThreadRow::disabled(thread("x")),
            ThreadRow::disabled(thread("y")),
            ThreadRow::enabled(thread("a")),
        ],
    )];

    let mut selection = ThreadListSelection::new();
    selection.mount(&groups);

    assert_eq!(selection.selected_thread(&groups), Some(&thread("a")));
}

#[test]
fn mount_on_empty_and_all_disabled_lists_selects_nothing() {
    let empty: Vec<ThreadListGroup> = Vec::new();
    let all_disabled = vec![
        group(Some(project("1")), vec![ThreadRow::disabled(thread("x"))]),
        group(None, vec![]),
    ];

    let mut selection = ThreadListSelection::new();
    selection.mount(&empty);
    assert_eq!(selection.selected_thread(&empty), None);

    selection.mount(&all_disabled);
    assert_eq!(selection.selected_thread(&all_disabled), None);
}

#[test]
fn arrow_down_and_up_skip_disabled_rows() {
    let groups = palette_groups();

    // From "b" the next enabled row is "c": the disabled row between them is
    // not a valid item (`getValidItems`, `itemIsDisabled`).
    let mut selection = ThreadListSelection::new();
    selection.mount(&groups);
    selection.handle_key(&groups, ListKey::ArrowDown);
    assert_eq!(selection.selected_thread(&groups), Some(&thread("b")));
    selection.handle_key(&groups, ListKey::ArrowDown);
    assert_eq!(selection.selected_thread(&groups), Some(&thread("c")));

    // Back up over the disabled row again.
    selection.handle_key(&groups, ListKey::ArrowUp);
    assert_eq!(selection.selected_thread(&groups), Some(&thread("b")));

    // Down once more lands back on "c", then crosses into project 2's only
    // valid row.
    selection.handle_key(&groups, ListKey::ArrowDown);
    assert_eq!(selection.selected_thread(&groups), Some(&thread("c")));
    selection.handle_key(&groups, ListKey::ArrowDown);
    assert_eq!(selection.selected_thread(&groups), Some(&thread("d")));
}

#[test]
fn movement_stops_at_both_edges_without_wrapping() {
    let groups = palette_groups();
    let mut selection = ThreadListSelection::new();

    // ArrowUp from nothing selected has no target (`findIndex` yields -1).
    selection.handle_key(&groups, ListKey::ArrowUp);
    assert_eq!(selection.selected_thread(&groups), None);

    // ArrowDown from nothing selects the first row: -1 + 1 = 0.
    selection.handle_key(&groups, ListKey::ArrowDown);
    assert_eq!(selection.selected_thread(&groups), Some(&thread("a")));

    // ArrowUp at the top stays; no `loop` option is set by the palette.
    selection.handle_key(&groups, ListKey::ArrowUp);
    assert_eq!(selection.selected_thread(&groups), Some(&thread("a")));

    selection.handle_key(&groups, ListKey::End);
    assert_eq!(selection.selected_thread(&groups), Some(&thread("e")));

    // ArrowDown at the bottom stays instead of wrapping to the first row.
    selection.handle_key(&groups, ListKey::ArrowDown);
    assert_eq!(selection.selected_thread(&groups), Some(&thread("e")));
}

#[test]
fn home_and_end_select_the_first_and_last_valid_rows() {
    let groups = palette_groups();
    let mut selection = ThreadListSelection::new();
    selection.mount(&groups);

    selection.handle_key(&groups, ListKey::End);
    assert_eq!(selection.selected_thread(&groups), Some(&thread("e")));

    selection.handle_key(&groups, ListKey::Home);
    assert_eq!(selection.selected_thread(&groups), Some(&thread("a")));
}

#[test]
fn meta_arrows_jump_between_boundaries_from_anywhere() {
    let groups = palette_groups();
    let mut selection = ThreadListSelection::new();
    selection.mount(&groups);

    selection.handle_key(&groups, ListKey::ArrowDown);
    selection.handle_key(&groups, ListKey::MetaArrowDown);
    assert_eq!(selection.selected_thread(&groups), Some(&thread("e")));

    selection.handle_key(&groups, ListKey::MetaArrowUp);
    assert_eq!(selection.selected_thread(&groups), Some(&thread("a")));
}

#[test]
fn boundary_keys_do_nothing_when_no_valid_rows_exist() {
    let empty: Vec<ThreadListGroup> = Vec::new();
    let all_disabled = vec![group(
        Some(project("1")),
        vec![ThreadRow::disabled(thread("x"))],
    )];

    for groups in [&empty, &all_disabled] {
        let mut selection = ThreadListSelection::new();
        selection.mount(groups);

        selection.handle_key(groups, ListKey::Home);
        assert_eq!(selection.selected_thread(groups), None);
        selection.handle_key(groups, ListKey::End);
        assert_eq!(selection.selected_thread(groups), None);
        selection.handle_key(groups, ListKey::MetaArrowDown);
        assert_eq!(selection.selected_thread(groups), None);
        selection.handle_key(groups, ListKey::MetaArrowUp);
        assert_eq!(selection.selected_thread(groups), None);
        selection.handle_key(groups, ListKey::Enter(EnterComposition::Clear));
        assert_eq!(selection.selected_thread(groups), None);
    }
}

#[test]
fn alt_arrow_down_lands_on_the_first_row_of_the_next_group_with_valid_rows() {
    let groups = vec![
        group(
            Some(project("1")),
            vec![
                ThreadRow::enabled(thread("a")),
                ThreadRow::enabled(thread("b")),
            ],
        ),
        // A group holding only disabled rows is walked past: it renders but
        // contributes no valid items to `updateSelectedByGroup`.
        group(Some(project("2")), vec![ThreadRow::disabled(thread("y"))]),
        group(None, vec![ThreadRow::enabled(thread("c"))]),
    ];
    let mut selection = ThreadListSelection::new();
    selection.mount(&groups);

    // Mid-group Alt+ArrowDown jumps straight into the next usable group and
    // takes its FIRST valid row, not the row adjacent to the selection.
    selection.handle_key(&groups, ListKey::ArrowDown);
    selection.handle_key(&groups, ListKey::AltArrowDown);
    assert_eq!(selection.selected_thread(&groups), Some(&thread("c")));
}

#[test]
fn alt_arrow_up_returns_to_the_first_row_of_the_previous_group() {
    let groups = palette_groups();
    let mut selection = ThreadListSelection::new();
    selection.mount(&groups);

    selection.handle_key(&groups, ListKey::End);
    selection.handle_key(&groups, ListKey::AltArrowUp);
    assert_eq!(selection.selected_thread(&groups), Some(&thread("d")));

    selection.handle_key(&groups, ListKey::AltArrowUp);
    assert_eq!(selection.selected_thread(&groups), Some(&thread("a")));
}

#[test]
fn alt_movement_past_the_final_group_falls_back_to_a_single_row_move() {
    let groups = palette_groups();
    let mut selection = ThreadListSelection::new();
    selection.mount(&groups);

    // From "a", the previous group does not exist: the engine falls back to
    // `updateSelectedByItem(-1)`, which at the top edge keeps the selection.
    selection.handle_key(&groups, ListKey::AltArrowUp);
    assert_eq!(selection.selected_thread(&groups), Some(&thread("a")));

    // From mid-final-group there is no next group either: the fallback still
    // moves one valid row down ("d" -> "e").
    selection.handle_key(&groups, ListKey::MetaArrowDown);
    selection.handle_key(&groups, ListKey::AltArrowUp);
    assert_eq!(selection.selected_thread(&groups), Some(&thread("d")));
    selection.handle_key(&groups, ListKey::AltArrowDown);
    assert_eq!(selection.selected_thread(&groups), Some(&thread("e")));

    // At the very last row the fallback has nowhere to go: edges stop.
    selection.handle_key(&groups, ListKey::AltArrowDown);
    assert_eq!(selection.selected_thread(&groups), Some(&thread("e")));
}

#[test]
fn enter_activates_the_selected_thread() {
    let groups = palette_groups();
    let mut selection = ThreadListSelection::new();
    selection.mount(&groups);

    let intent = selection.handle_key(&groups, ListKey::Enter(EnterComposition::Clear));
    assert_eq!(
        intent,
        Some(ThreadActivationIntent::OpenThread {
            thread_id: thread("a")
        })
    );

    selection.handle_key(&groups, ListKey::End);
    let intent = selection.handle_key(&groups, ListKey::Enter(EnterComposition::Clear));
    assert_eq!(
        intent,
        Some(ThreadActivationIntent::OpenThread {
            thread_id: thread("e")
        })
    );
}

#[test]
fn enter_without_any_selection_activates_nothing() {
    let groups = palette_groups();
    let mut selection = ThreadListSelection::new();

    let intent = selection.handle_key(&groups, ListKey::Enter(EnterComposition::Clear));

    assert_eq!(intent, None);
    assert_eq!(selection.selected_thread(&groups), None);
}

#[test]
fn enter_is_suppressed_during_ime_composition() {
    let groups = palette_groups();
    let suppressed = [EnterComposition::Composing, EnterComposition::KeyCode229];

    for composition in suppressed {
        let mut selection = ThreadListSelection::new();
        selection.mount(&groups);

        let intent = selection.handle_key(&groups, ListKey::Enter(composition));

        assert_eq!(intent, None, "{composition:?} must suppress activation");
        assert_eq!(
            selection.selected_thread(&groups),
            Some(&thread("a")),
            "{composition:?} must leave selection untouched"
        );
    }

    // Clear composition activates normally afterwards.
    let mut selection = ThreadListSelection::new();
    selection.mount(&groups);
    let intent = selection.handle_key(&groups, ListKey::Enter(EnterComposition::Clear));
    assert!(intent.is_some());
}

#[test]
fn losing_the_selected_row_repairs_selection_to_the_first_valid_row() {
    let groups = palette_groups();
    let mut selection = ThreadListSelection::new();
    selection.mount(&groups);
    selection.handle_key(&groups, ListKey::End);
    assert_eq!(selection.selected_thread(&groups), Some(&thread("e")));

    // The unassigned section leaves the live list, taking the selection with
    // it. The engine repairs exactly this on its removal path: the teardown
    // of the selected item selects the first valid row (`registerItem`
    // cleanup → `#selectFirstItem`), and the model exposes the same rule.
    let narrowed = &groups[..2];
    assert_eq!(selection.selected_thread(narrowed), None);

    selection.resync(narrowed);
    assert_eq!(selection.selected_thread(narrowed), Some(&thread("a")));

    // An intact selection is left alone: the engine's cleanup acts only when
    // the removed item was the selected one.
    selection.handle_key(narrowed, ListKey::ArrowDown);
    assert_eq!(selection.selected_thread(narrowed), Some(&thread("b")));
    selection.resync(narrowed);
    assert_eq!(selection.selected_thread(narrowed), Some(&thread("b")));
}
