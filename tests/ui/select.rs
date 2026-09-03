use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use artisan_ui::select::{
    Select, SelectAction, SelectEntry, SelectKey, SelectScrollState, SelectSize, SelectState,
    item_debug_selector, stable_value_selector_suffix,
};
use artisan_ui::theme::{ArtisanTheme, ThemeMode};
use gpui::{Component, Context, FocusHandle, IntoElement, Render, TestAppContext, Window};

fn test_focus(cx: &mut TestAppContext) -> FocusHandle {
    cx.update(|app| app.focus_handle())
}

fn entries() -> Vec<SelectEntry<String>> {
    vec![
        SelectEntry::group("Models"),
        SelectEntry::item("fast".to_owned(), "Fast"),
        SelectEntry::disabled_item("safe".to_owned(), "Safe"),
        SelectEntry::separator(),
        SelectEntry::item("balanced".to_owned(), "Balanced"),
    ]
}

fn navigation_entries() -> Vec<SelectEntry<String>> {
    vec![
        SelectEntry::item("alpha".to_owned(), "Alpha"),
        SelectEntry::disabled_item("disabled".to_owned(), "Disabled"),
        SelectEntry::item("bravo".to_owned(), "Bravo"),
        SelectEntry::item("charlie".to_owned(), "Charlie"),
    ]
}

fn enabled_flags<V: artisan_ui::select::SelectValue>(entries: &[SelectEntry<V>]) -> Vec<bool> {
    entries.iter().map(SelectEntry::is_enabled).collect()
}

struct SelectProbe {
    focus: FocusHandle,
    theme: ArtisanTheme,
    selected: Option<String>,
    open: bool,
    state: Rc<RefCell<SelectState>>,
    changes: Rc<RefCell<Vec<String>>>,
    open_changes: Rc<RefCell<Vec<bool>>>,
}

impl Render for SelectProbe {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let changes = Rc::clone(&self.changes);
        let open_changes = Rc::clone(&self.open_changes);

        Component::new(
            Select::new(
                "select-test",
                self.focus.clone(),
                self.theme,
                self.selected.clone(),
                entries(),
            )
            .open(self.open)
            .placeholder("Choose a model")
            .debug_selector("model-select")
            .with_interaction_state(Rc::clone(&self.state))
            .on_change(move |value, _, _, _| changes.borrow_mut().push(value))
            .on_open_change(move |open, _, _| open_changes.borrow_mut().push(open)),
        )
    }
}

#[gpui::test]
fn controlled_selection_placeholder_and_current_label(cx: &mut TestAppContext) {
    let theme = ArtisanTheme::for_mode(ThemeMode::Light);
    let focus = test_focus(cx);
    let select = Select::new("select", focus, theme, None, navigation_entries())
        .placeholder("Pick a model")
        .size(SelectSize::Small)
        .debug_selector("settings-model")
        .scroll_state(SelectScrollState::Both);

    assert_eq!(select.current_label(), None);
    assert_eq!(select.display_label(), "Pick a model");
    assert_eq!(
        select.visual_style().trigger_height,
        theme.density.control_sm
    );
    assert_eq!(select.scroll_state_value(), SelectScrollState::Both);

    let semantic = select.semantic_state();
    assert_eq!(semantic.trigger_role, "combobox");
    assert_eq!(semantic.listbox_role, "listbox");
    assert_eq!(semantic.option_role, "option");
    assert_eq!(semantic.label, "Pick a model");
    assert!(semantic.placeholder);
    assert!(!semantic.expanded);
    assert_eq!(semantic.trigger_selector, "settings-model-trigger");

    let selected = Select::new(
        "select",
        test_focus(cx),
        theme,
        Some("bravo".to_owned()),
        navigation_entries(),
    )
    .debug_selector("settings-model");

    assert_eq!(selected.current_label(), Some("Bravo"));
    assert_eq!(selected.display_label(), "Bravo");
    assert!(!selected.semantic_state().placeholder);

    let flags = enabled_flags(selected.entries());
    let state = SelectState::new(true, Some(2), &flags);
    let item_states = selected.item_semantics(&state);
    assert_eq!(item_states.len(), 4);
    assert!(item_states[2].selected);
    assert_eq!(
        item_states[2].selector,
        item_debug_selector("settings-model", &"bravo".to_owned())
    );
}

#[test]
fn open_close_navigation_and_disabled_items_are_deterministic() {
    let entries = navigation_entries();
    let flags = enabled_flags(&entries);
    let mut state = SelectState::new(false, Some(0), &flags);

    assert_eq!(
        state.handle_key_at(SelectKey::ArrowDown, &entries, Duration::from_millis(1)),
        SelectAction::Open
    );
    assert!(state.is_open());
    assert_eq!(state.highlighted_index(), Some(0));

    assert_eq!(
        state.handle_key_at(SelectKey::ArrowDown, &entries, Duration::from_millis(2)),
        SelectAction::Highlight(2)
    );
    assert_eq!(state.highlighted_index(), Some(2));

    assert_eq!(
        state.handle_key_at(SelectKey::ArrowUp, &entries, Duration::from_millis(3)),
        SelectAction::Highlight(0)
    );

    assert_eq!(
        state.handle_key_at(SelectKey::End, &entries, Duration::ZERO),
        SelectAction::Highlight(3)
    );
    assert_eq!(
        state.handle_key_at(SelectKey::Home, &entries, Duration::ZERO),
        SelectAction::Highlight(0)
    );
    assert_eq!(
        state.handle_key_at(SelectKey::PageDown, &entries, Duration::ZERO),
        SelectAction::Highlight(3)
    );
    assert_eq!(
        state.handle_key_at(SelectKey::PageUp, &entries, Duration::ZERO),
        SelectAction::Highlight(0)
    );

    assert_eq!(
        state.handle_key_at(SelectKey::Activate, &entries, Duration::ZERO),
        SelectAction::Commit(0)
    );
    assert!(!state.is_open());
    assert!(SelectAction::Commit(0).restores_trigger_focus());

    state.reconcile(true, Some(0), &flags);
    assert_eq!(
        state.handle_key_at(SelectKey::Escape, &entries, Duration::ZERO),
        SelectAction::Close
    );
    assert!(!state.is_open());
}

#[test]
fn typeahead_matches_cycles_resets_and_filters_disabled_items() {
    let entries = navigation_entries();
    let flags = enabled_flags(&entries);
    let mut state = SelectState::new(true, None, &flags);

    assert_eq!(
        state.handle_key_at(
            SelectKey::Character('b'),
            &entries,
            Duration::from_millis(10)
        ),
        SelectAction::Highlight(2)
    );
    assert_eq!(state.typeahead_query(), "b");

    assert_eq!(
        state.handle_key_at(
            SelectKey::Character('c'),
            &entries,
            Duration::from_millis(20)
        ),
        SelectAction::Highlight(3)
    );
    assert_eq!(state.typeahead_query(), "bc");

    assert_eq!(
        state.handle_key_at(
            SelectKey::Character('a'),
            &entries,
            Duration::from_millis(2_000)
        ),
        SelectAction::Highlight(0)
    );
    assert_eq!(state.typeahead_query(), "a");

    let repeated_entries = vec![
        SelectEntry::item("a1".to_owned(), "Alpha"),
        SelectEntry::item("a2".to_owned(), "Alpine"),
        SelectEntry::disabled_item("archive".to_owned(), "Archive"),
    ];
    let repeated_flags = enabled_flags(&repeated_entries);
    let mut repeated = SelectState::new(true, None, &repeated_flags);

    assert_eq!(
        repeated.handle_key_at(
            SelectKey::Character('a'),
            &repeated_entries,
            Duration::from_millis(1)
        ),
        SelectAction::Highlight(0)
    );
    assert_eq!(
        repeated.handle_key_at(
            SelectKey::Character('a'),
            &repeated_entries,
            Duration::from_millis(2)
        ),
        SelectAction::Highlight(1)
    );
    assert_eq!(repeated.highlighted_index(), Some(1));

    let disabled_only = vec![
        SelectEntry::disabled_item("archive".to_owned(), "Archive"),
        SelectEntry::item("beta".to_owned(), "Beta"),
    ];
    let disabled_flags = enabled_flags(&disabled_only);
    let mut disabled_state = SelectState::new(true, None, &disabled_flags);

    assert_eq!(
        disabled_state.handle_key_at(
            SelectKey::Character('a'),
            &disabled_only,
            Duration::from_millis(1)
        ),
        SelectAction::None
    );
    assert_eq!(disabled_state.highlighted_index(), Some(1));
}

#[test]
fn closed_typeahead_commits_and_controlled_updates_reconcile_highlight() {
    let entries = navigation_entries();
    let flags = enabled_flags(&entries);
    let mut state = SelectState::new(false, None, &flags);

    assert_eq!(
        state.handle_key_at(
            SelectKey::Character('c'),
            &entries,
            Duration::from_millis(5)
        ),
        SelectAction::Commit(3)
    );
    assert!(!state.is_open());
    assert_eq!(state.highlighted_index(), Some(3));

    state.reconcile(false, Some(3), &flags);
    assert_eq!(state.selected_index(), Some(3));
    assert_eq!(state.highlighted_index(), Some(3));
    assert_eq!(state.typeahead_query(), "");

    state.reconcile(true, Some(2), &flags);
    assert!(state.is_open());
    assert_eq!(state.highlighted_index(), Some(2));

    assert_eq!(
        state.handle_key_at(SelectKey::ArrowDown, &entries, Duration::from_millis(10)),
        SelectAction::Highlight(3)
    );

    state.reconcile(true, Some(0), &flags);
    assert_eq!(state.highlighted_index(), Some(0));

    state.reconcile(true, Some(1), &flags);
    assert_eq!(state.highlighted_index(), Some(0));
}

#[test]
fn selectors_groups_separators_and_scroll_edges_are_public() {
    assert_eq!(
        stable_value_selector_suffix(&"alpha".to_owned()),
        stable_value_selector_suffix(&"alpha".to_owned())
    );
    assert_ne!(
        stable_value_selector_suffix(&"alpha".to_owned()),
        stable_value_selector_suffix(&"bravo".to_owned())
    );

    assert_eq!(
        SelectScrollState::from_edges(false, false),
        SelectScrollState::None
    );
    assert_eq!(
        SelectScrollState::from_edges(true, false),
        SelectScrollState::Start
    );
    assert_eq!(
        SelectScrollState::from_edges(false, true),
        SelectScrollState::End
    );
    assert_eq!(
        SelectScrollState::from_edges(true, true),
        SelectScrollState::Both
    );
    assert!(SelectScrollState::Both.can_scroll_up());
    assert!(SelectScrollState::Both.can_scroll_down());

    let entries = entries();
    assert!(matches!(entries[0], SelectEntry::Group(_)));
    assert!(matches!(entries[3], SelectEntry::Separator));
    assert!(!entries[2].is_enabled());
}

#[gpui::test]
fn gpui_trigger_open_close_commit_and_focus_restore(cx: &mut TestAppContext) {
    let theme = ArtisanTheme::for_mode(ThemeMode::Light);
    let state = Rc::new(RefCell::new(SelectState::default()));
    let changes = Rc::new(RefCell::new(Vec::<String>::new()));
    let open_changes = Rc::new(RefCell::new(Vec::<bool>::new()));
    let focus = test_focus(cx);

    let probe = SelectProbe {
        focus: focus.clone(),
        theme,
        selected: None,
        open: false,
        state: Rc::clone(&state),
        changes: Rc::clone(&changes),
        open_changes: Rc::clone(&open_changes),
    };

    let (probe, cx) = cx.add_window_view(|_, _| probe);
    cx.run_until_parked();

    let focus_for_window = focus.clone();
    cx.update(|window, _| window.focus(&focus_for_window));
    cx.simulate_keystrokes("enter");

    assert_eq!(&*open_changes.borrow(), &[true]);
    assert!(state.borrow().is_open());

    cx.update(|_, app| {
        probe.update(app, |probe, cx| {
            probe.open = true;
            cx.notify();
        });
    });
    cx.run_until_parked();

    cx.simulate_keystrokes("down");
    cx.simulate_keystrokes("end");
    cx.simulate_keystrokes("enter");

    assert_eq!(&*open_changes.borrow(), &[true, false]);
    assert_eq!(&*changes.borrow(), &["balanced".to_owned()]);
    cx.update(|window, _| assert!(focus.is_focused(window)));

    cx.update(|_, app| {
        probe.update(app, |probe, cx| {
            probe.open = true;
            cx.notify();
        });
    });
    cx.run_until_parked();

    cx.simulate_keystrokes("escape");

    assert_eq!(&*open_changes.borrow(), &[true, false, false]);
    cx.update(|window, _| assert!(focus.is_focused(window)));
}
