//! Deterministic state-model coverage for the native project-picker leaf.
//!
//! Every mandated §6.2 behavior is asserted here without launching a window:
//! controlled open/focus, wrapped navigation with Home/End, the 1-second
//! printable typeahead (extension, expiry, cycling, missed-prefix restart),
//! current-selection no-op, different-selection emission, New project as the
//! distinct final row, disabled refusal, and Escape/outside dismissal.

use artisan_domain::ProjectId;
use artisan_frontend::project_picker::{
    PickerRow, ProjectOption, ProjectPickerAction, ProjectPickerState, TYPEAHEAD_BUFFER_MILLIS,
};

fn fixture_id(name: &str) -> ProjectId {
    ProjectId::parse(name.to_lowercase()).expect("fixture ids are valid")
}

fn fixture_option(name: &str) -> ProjectOption {
    ProjectOption {
        id: fixture_id(name),
        name: name.to_string().into(),
    }
}

/// Alpha / Beta / Gamma catalog plus an optional current selection.
fn picker_state(current: Option<&str>) -> ProjectPickerState {
    ProjectPickerState::new(
        vec![
            fixture_option("Alpha"),
            fixture_option("Beta"),
            fixture_option("Gamma"),
        ],
        current.map(fixture_id),
    )
}

fn opened(current: Option<&str>) -> ProjectPickerState {
    let mut state = picker_state(current);
    state.press_trigger();
    assert!(state.is_open(), "fixture must open");
    state
}

#[test]
fn documented_typeahead_buffer_is_one_second() {
    assert_eq!(TYPEAHEAD_BUFFER_MILLIS, 1_000);
}

#[test]
fn closed_picker_ignores_keyboard_input() {
    let mut state = picker_state(Some("Beta"));

    state.move_next();
    state.move_last();
    state.handle_typeahead('a', 0);
    state.activate_highlighted();
    state.dismiss();

    assert!(!state.is_open());
    assert_eq!(state.highlighted_row(), None);
    assert_eq!(state.typeahead_buffer(), "");
    assert!(state.take_actions().is_empty());
}

#[test]
fn opening_highlights_the_current_project_row() {
    let mut state = picker_state(Some("Beta"));
    assert_eq!(
        state.trigger_label(),
        "Project: Beta",
        "default accessible name names the current project"
    );

    state.press_trigger();

    assert!(state.is_open());
    assert_eq!(state.highlighted_row(), Some(PickerRow::Project(1)));
}

#[test]
fn opening_without_current_project_falls_back_to_first_row() {
    let mut state = picker_state(None);
    assert_eq!(
        state.trigger_label(),
        "Project: Choose a project",
        "nothing attached reads as an invitation, not \"None\""
    );

    state.press_trigger();

    assert_eq!(state.highlighted_row(), Some(PickerRow::Project(0)));
}

#[test]
fn empty_catalog_opens_on_the_new_project_row() {
    let mut state = ProjectPickerState::new(Vec::new(), None);

    state.press_trigger();

    assert!(state.is_open());
    assert_eq!(state.highlighted_row(), Some(PickerRow::NewProject));
    assert_eq!(state.selectable_row_count(), 1);
}

#[test]
fn trigger_toggles_closed_without_emitting_an_action() {
    let mut state = opened(Some("Alpha"));

    state.press_trigger();

    assert!(!state.is_open());
    assert_eq!(state.highlighted_row(), None);
    assert!(state.take_actions().is_empty());
}

#[test]
fn trigger_label_override_replaces_the_whole_name() {
    let mut state = picker_state(Some("Alpha"));
    state.set_trigger_label(Some("Open projects".into()));

    assert_eq!(state.trigger_label(), "Open projects");

    state.set_trigger_label(None);
    assert_eq!(state.trigger_label(), "Project: Alpha");
}

#[test]
fn disabled_trigger_refuses_to_open_and_closes_an_open_menu() {
    let mut state = picker_state(Some("Alpha"));

    state.set_disabled(true);
    state.press_trigger();
    assert!(!state.is_open(), "disabled triggers must refuse to open");

    state.set_disabled(false);
    state.press_trigger();
    assert!(state.is_open());

    // Disabling mid-flight closes immediately, without emitting anything.
    state.set_disabled(true);
    assert!(!state.is_open());
    assert_eq!(state.highlighted_row(), None);
    assert!(state.take_actions().is_empty());
}

#[test]
fn arrow_navigation_wraps_past_both_ends() {
    let mut state = opened(Some("Beta"));

    state.move_next();
    assert_eq!(state.highlighted_row(), Some(PickerRow::Project(2)));

    state.move_next();
    assert_eq!(
        state.highlighted_row(),
        Some(PickerRow::NewProject),
        "movement crosses the visual separator onto the final row"
    );

    state.move_next();
    assert_eq!(
        state.highlighted_row(),
        Some(PickerRow::Project(0)),
        "moving down past the final row wraps to the first"
    );

    state.move_previous();
    assert_eq!(
        state.highlighted_row(),
        Some(PickerRow::NewProject),
        "moving up past the first row wraps to the final one"
    );
}

#[test]
fn home_and_end_jump_to_the_boundary_rows() {
    let mut state = opened(Some("Beta"));

    state.move_last();
    assert_eq!(state.highlighted_row(), Some(PickerRow::NewProject));

    state.move_first();
    assert_eq!(state.highlighted_row(), Some(PickerRow::Project(0)));
}

#[test]
fn typeahead_extends_prefixes_while_the_buffer_lives() {
    let mut state = opened(Some("Alpha"));

    state.handle_typeahead('g', 0);
    assert_eq!(state.highlighted_row(), Some(PickerRow::Project(2)));
    assert_eq!(state.typeahead_buffer(), "g");

    state.handle_typeahead('a', 500);
    state.handle_typeahead('m', 900);
    assert_eq!(
        state.highlighted_row(),
        Some(PickerRow::Project(2)),
        "\"gam\" keeps matching Gamma"
    );
    assert_eq!(state.typeahead_buffer(), "gam");
}

#[test]
fn typeahead_normalizes_uppercase_printable_input() {
    let mut state = opened(Some("Alpha"));

    state.handle_typeahead('G', 0);
    state.handle_typeahead('A', 100);

    assert_eq!(state.highlighted_row(), Some(PickerRow::Project(2)));
    assert_eq!(state.typeahead_buffer(), "ga");
}

#[test]
fn typeahead_buffer_expires_after_the_documented_second() {
    let mut state = opened(Some("Alpha"));

    state.handle_typeahead('b', 0);
    assert_eq!(state.highlighted_row(), Some(PickerRow::Project(1)));

    // One millisecond short of the window: still accumulating.
    state.handle_typeahead('e', 999);
    assert_eq!(state.typeahead_buffer(), "be");

    // At exactly 1s since the last keystroke the buffer has expired, so this
    // 'g' starts a fresh search instead of extending "beg".
    state.handle_typeahead('g', 1_999);
    assert_eq!(state.highlighted_row(), Some(PickerRow::Project(2)));
    assert_eq!(state.typeahead_buffer(), "g");
}

#[test]
fn repeated_single_characters_cycle_through_prefix_matches() {
    // Purpose-built catalog with two rows that genuinely start with "a":
    // ["Anchor", "Beta", "Aurora"], current Beta.
    let mut state = ProjectPickerState::new(
        vec![
            fixture_option("Anchor"),
            fixture_option("Beta"),
            fixture_option("Aurora"),
        ],
        Some(fixture_id("Beta")),
    );
    state.press_trigger();
    assert_eq!(state.highlighted_row(), Some(PickerRow::Project(1)));

    // First 'a' extends a fresh prefix and lands on the next A-row.
    state.handle_typeahead('a', 0);
    assert_eq!(state.highlighted_row(), Some(PickerRow::Project(2)));

    // Repeating the same character cycles, wrapping past the final row.
    state.handle_typeahead('a', 100);
    assert_eq!(state.highlighted_row(), Some(PickerRow::Project(0)));

    // ...and keeps cycling between the two A-prefix rows.
    state.handle_typeahead('a', 200);
    assert_eq!(
        state.highlighted_row(),
        Some(PickerRow::Project(2)),
        "cycling wraps around every selectable row"
    );
}

#[test]
fn missed_extension_restarts_the_buffer_at_the_new_character() {
    let mut state = opened(Some("Alpha"));

    state.handle_typeahead('z', 0);
    assert_eq!(state.highlighted_row(), Some(PickerRow::Project(0)));
    assert_eq!(state.typeahead_buffer(), "z");

    state.handle_typeahead('y', 100);
    assert_eq!(
        state.highlighted_row(),
        Some(PickerRow::Project(0)),
        "\"zy\" matched nothing either, so the buffer restarted at 'y'"
    );
    assert_eq!(state.typeahead_buffer(), "y");
}

#[test]
fn missed_extension_immediately_searches_the_restarted_prefix() {
    let mut state = opened(Some("Alpha"));

    state.handle_typeahead('b', 0);
    assert_eq!(state.highlighted_row(), Some(PickerRow::Project(1)));

    state.handle_typeahead('g', 100);

    assert_eq!(state.typeahead_buffer(), "g");
    assert_eq!(state.highlighted_row(), Some(PickerRow::Project(2)));
}

#[test]
fn activating_the_current_project_closes_without_action() {
    let mut state = opened(Some("Alpha"));
    assert_eq!(state.highlighted_row(), Some(PickerRow::Project(0)));

    state.activate_highlighted();

    assert!(!state.is_open(), "selection closes before anything else");
    assert!(state.take_actions().is_empty());
}

#[test]
fn choosing_a_different_project_emits_choose_after_closing() {
    let mut state = opened(Some("Alpha"));
    state.move_next();
    assert_eq!(state.highlighted_row(), Some(PickerRow::Project(1)));

    state.activate_highlighted();

    assert!(!state.is_open());
    assert_eq!(
        state.take_actions(),
        vec![ProjectPickerAction::Choose(fixture_id("Beta"))],
    );

    // The controller repoints the surface; repeating the choice is a no-op.
    state.set_current(Some(fixture_id("Beta")));
    state.press_trigger();
    assert_eq!(state.highlighted_row(), Some(PickerRow::Project(1)));
    state.activate_highlighted();
    assert!(state.take_actions().is_empty());
}

#[test]
fn new_project_is_the_distinct_final_action() {
    let mut state = opened(Some("Alpha"));
    assert_eq!(state.selectable_row_count(), 4);

    state.move_last();
    assert_eq!(state.highlighted_row(), Some(PickerRow::NewProject));

    state.activate_highlighted();

    assert!(!state.is_open());
    assert_eq!(state.take_actions(), vec![ProjectPickerAction::NewProject]);
}

#[test]
fn dismissal_closes_without_action_and_reopening_refocuses_current() {
    let mut state = opened(Some("Alpha"));

    state.move_next();
    state.dismiss();
    assert!(!state.is_open());
    assert!(state.take_actions().is_empty());

    state.press_trigger();
    assert_eq!(
        state.highlighted_row(),
        Some(PickerRow::Project(0)),
        "the current project's row is the initial focus on every open"
    );
}
