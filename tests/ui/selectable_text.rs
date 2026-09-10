//! Behavioral coverage for native selectable transcript text.
//!
//! Pure selection, merge, keystroke, and drag-lifecycle tests need no
//! window. The GPUI tests mount one [`SelectableText`] probe and drive the
//! focused keyboard paths (`control-a` select-all, `control-c` copy through
//! the platform clipboard) exactly like the shared input-surface tests do.

use artisan_ui::selectable_text::{
    SelectableText, SelectableTextState, clamp_to_char_boundary, is_copy_keystroke,
    is_select_all_keystroke, merge_selection_highlight, normalize_selection,
    selection_style_for_theme,
};
use artisan_ui::theme::{ArtisanTheme, ThemeMode};
use gpui::{
    Context, FocusHandle, HighlightStyle, IntoElement, Modifiers, Render, TestAppContext, Window,
    div, px,
};

const BODY: &str = "hello world";
const EMOJI_BODY: &str = "a💡b";

fn base_style() -> HighlightStyle {
    HighlightStyle {
        fade_out: Some(0.5),
        ..Default::default()
    }
}

fn modifiers(control: bool, platform: bool, shift: bool, alt: bool) -> Modifiers {
    Modifiers {
        control,
        platform,
        shift,
        alt,
        ..Default::default()
    }
}

#[test]
fn clamp_floors_mid_character_indices_and_saturates() {
    assert_eq!(clamp_to_char_boundary(BODY, 0), 0);
    assert_eq!(clamp_to_char_boundary(BODY, 5), 5);
    assert_eq!(clamp_to_char_boundary(BODY, 10_000), BODY.len());
    // 💡 occupies bytes 1..5; every interior index folds to the boundary.
    assert_eq!(clamp_to_char_boundary(EMOJI_BODY, 1), 1);
    assert_eq!(clamp_to_char_boundary(EMOJI_BODY, 2), 1);
    assert_eq!(clamp_to_char_boundary(EMOJI_BODY, 4), 1);
    assert_eq!(clamp_to_char_boundary(EMOJI_BODY, 5), 5);
}

#[test]
fn normalize_orders_endpoints_and_rejects_collapsed() {
    assert_eq!(normalize_selection(2, 8, BODY), Some(2..8));
    assert_eq!(normalize_selection(8, 2, BODY), Some(2..8));
    assert_eq!(normalize_selection(4, 4, BODY), None);
    assert_eq!(normalize_selection(0, BODY.len(), BODY), Some(0..BODY.len()));
    // Clamping applies before the collapse check: both ends fold to 1.
    assert_eq!(normalize_selection(2, 4, EMOJI_BODY), None);
    assert_eq!(normalize_selection(0, 5, EMOJI_BODY), Some(0..5));
}

#[test]
fn drag_lifecycle_selects_range_and_click_clears() {
    let state = SelectableTextState::new();
    state.validate_for_text(BODY);

    state.begin_drag(2);
    assert!(state.is_dragging());
    assert!(!state.has_selection());
    assert_eq!(state.down_index(), Some(2));

    assert!(state.update_drag(8));
    assert!(!state.update_drag(8));
    assert!(state.end_drag(BODY));
    assert!(!state.is_dragging());
    assert_eq!(state.selection_range(), Some(2..8));
    assert_eq!(state.selected_text(BODY), "llo wo");
    assert!(state.suppresses_click());

    // A press without movement collapses the retained selection.
    state.begin_drag(5);
    assert!(!state.end_drag(BODY));
    assert!(!state.has_selection());
    assert!(!state.suppresses_click());
    assert_eq!(state.selected_text(BODY), "");
    assert_eq!(state.copy_text(BODY), None);
}

#[test]
fn reverse_drag_normalizes_and_unicode_slices_safely() {
    let state = SelectableTextState::new();
    state.validate_for_text(EMOJI_BODY);

    state.begin_drag(5);
    state.update_drag(0);
    assert!(state.end_drag(EMOJI_BODY));
    assert_eq!(state.selection_range(), Some(0..5));
    assert_eq!(state.selected_text(EMOJI_BODY), "a💡");
    assert_eq!(state.copy_text(EMOJI_BODY), Some("a💡".to_owned()));
}

#[test]
fn text_change_drops_stale_ranges() {
    let state = SelectableTextState::new();
    state.validate_for_text(BODY);
    state.select_all();
    assert_eq!(state.selection_range(), Some(0..BODY.len()));

    state.validate_for_text("revised body text");
    assert!(!state.has_selection());
    assert_eq!(state.selection_range(), None);

    state.validate_for_text("revised body text");
    state.select_all();
    assert_eq!(state.selection_range(), Some(0.."revised body text".len()));
}

#[test]
fn select_all_is_empty_safe_and_clear_resets_press_latch() {
    let state = SelectableTextState::new();
    state.validate_for_text("");
    state.select_all();
    assert!(!state.has_selection());

    state.validate_for_text(BODY);
    state.begin_drag(3);
    state.clear_selection();
    assert!(!state.has_selection());
    assert!(!state.is_dragging());
    assert_eq!(state.down_index(), None);

    state.clear_press();
    assert_eq!(state.down_index(), None);
}

#[test]
fn merge_passes_plain_ranges_through_and_selection_wins_overlaps() {
    let theme = ArtisanTheme::for_mode(ThemeMode::Light);
    let selection_style = selection_style_for_theme(theme);
    assert_eq!(
        selection_style.background_color,
        Some(theme.interaction.selection_background.to_paint())
    );
    assert_eq!(
        selection_style.color,
        Some(theme.interaction.selection_foreground.to_paint())
    );

    let base = vec![(0..5, base_style()), (6..11, base_style())];
    let passthrough = merge_selection_highlight(BODY, base.clone(), None, &selection_style);
    assert_eq!(passthrough, base);

    let merged = merge_selection_highlight(BODY, base, Some(4..7), &selection_style);
    assert_eq!(
        merged
            .iter()
            .map(|(range, _)| range.clone())
            .collect::<Vec<_>>(),
        vec![0..4, 4..7, 7..11]
    );
    assert_eq!(merged[1].1, selection_style);
    assert_eq!(merged[0].1, base_style());
    assert_eq!(merged[2].1, base_style());

    // Degenerate and out-of-range caller ranges never reach StyledText.
    let messy = vec![(3..3, base_style()), (9..10_000, base_style())];
    let cleaned = merge_selection_highlight(BODY, messy, None, &selection_style);
    assert_eq!(cleaned.len(), 1);
    assert_eq!(cleaned[0].0, 9..BODY.len());
}

#[test]
fn keystroke_predicates_match_copy_and_select_all_only() {
    assert!(is_copy_keystroke("c", &modifiers(true, false, false, false)));
    assert!(is_copy_keystroke("C", &modifiers(false, true, false, false)));
    assert!(is_select_all_keystroke("a", &modifiers(true, false, false, false)));
    assert!(is_select_all_keystroke("A", &modifiers(false, true, false, false)));

    assert!(!is_copy_keystroke("c", &modifiers(false, false, false, false)));
    assert!(!is_copy_keystroke("c", &modifiers(true, false, true, false)));
    assert!(!is_copy_keystroke("c", &modifiers(true, false, false, true)));
    assert!(!is_copy_keystroke("x", &modifiers(true, false, false, false)));
    assert!(!is_select_all_keystroke("a", &modifiers(false, false, false, false)));
    assert!(!is_select_all_keystroke("a", &modifiers(true, false, true, false)));
    assert!(!is_select_all_keystroke("c", &modifiers(true, false, false, false)));
}

struct SelectionProbe {
    focus: FocusHandle,
    state: SelectableTextState,
    text: String,
}

impl SelectionProbe {
    fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus: cx.focus_handle(),
            state: SelectableTextState::new(),
            text: BODY.to_owned(),
        }
    }
}

impl Render for SelectionProbe {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().w(px(400.0)).child(
            SelectableText::new(
                "probe-selectable",
                self.text.clone(),
                &self.state,
                ArtisanTheme::for_mode(ThemeMode::Light),
                Vec::new(),
            )
            .focus(self.focus.clone()),
        )
    }
}

#[gpui::test]
fn probe_paints_without_selection_on_fresh_state(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, cx| SelectionProbe::new(cx));
    cx.run_until_parked();

    cx.update(|_, app| {
        let probe = view.read(app);
        assert!(!probe.state.has_selection());
        assert_eq!(probe.state.selection_range(), None);
    });
}

#[gpui::test]
fn focused_select_all_shortcut_selects_full_text(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, cx| SelectionProbe::new(cx));
    cx.update(|window, app| {
        let focus = view.read(app).focus.clone();
        window.focus(&focus, app);
    });
    cx.run_until_parked();

    cx.simulate_keystrokes("ctrl-a");
    cx.run_until_parked();

    cx.update(|_, app| {
        assert_eq!(view.read(app).state.selection_range(), Some(0..BODY.len()));
    });
}

#[gpui::test]
fn focused_copy_shortcut_writes_selection_to_clipboard(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, cx| SelectionProbe::new(cx));
    cx.update(|window, app| {
        let focus = view.read(app).focus.clone();
        window.focus(&focus, app);
    });
    cx.run_until_parked();

    cx.simulate_keystrokes("ctrl-a");
    cx.run_until_parked();
    cx.simulate_keystrokes("ctrl-c");
    cx.run_until_parked();

    let copied = cx.update(|_, app| {
        app.read_from_clipboard()
            .as_ref()
            .and_then(gpui::ClipboardItem::text)
    });
    assert_eq!(copied, Some(BODY.to_owned()));
}

#[gpui::test]
fn unfocused_copy_shortcut_leaves_clipboard_alone(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, cx| SelectionProbe::new(cx));
    cx.run_until_parked();

    cx.update(|_, app| {
        view.read(app).state.select_all();
    });
    cx.simulate_keystrokes("ctrl-c");
    cx.run_until_parked();

    let copied = cx.update(|_, app| {
        app.read_from_clipboard()
            .as_ref()
            .and_then(gpui::ClipboardItem::text)
    });
    assert_eq!(copied, None);
}
