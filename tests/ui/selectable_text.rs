//! Behavioral coverage for native selectable transcript text.
//!
//! Pure selection, merge, keystroke, and drag-lifecycle tests need no
//! window. The GPUI tests mount one [`SelectableText`] probe and drive the
//! focused keyboard paths (`control-a` select-all, `control-c` copy through
//! the platform clipboard) exactly like the shared input-surface tests do.

use artisan_ui::selectable_text::{
    SelectableText, SelectableTextState, TextRunOverride, clamp_to_char_boundary,
    compile_text_runs, is_copy_keystroke, is_select_all_keystroke, merge_selection_highlight,
    normalize_selection, selection_style_for_theme,
};
use artisan_ui::theme::{ArtisanTheme, ThemeMode};
use gpui::{
    Context, FocusHandle, FontStyle, FontWeight, HighlightStyle, InteractiveElement, IntoElement,
    Modifiers, ParentElement, Pixels, Point, Render, SharedString, Styled, TestAppContext,
    TextStyle, VisualTestContext, Window, div, point, px,
};
use std::cell::Cell;
use std::rc::Rc;

const BODY: &str = "hello world";
const EMOJI_BODY: &str = "a💡b";

fn base_style() -> HighlightStyle {
    HighlightStyle {
        fade_out: Some(0.5),
        ..Default::default()
    }
}

#[expect(
    clippy::fn_params_excessive_bools,
    reason = "the helper mirrors the four GPUI modifier axes explicitly at each test call site"
)]
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
    assert_eq!(
        normalize_selection(0, BODY.len(), BODY),
        Some(0..BODY.len())
    );
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
    let _ = state.update_drag(0);
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
fn merge_overlays_selection_wash_onto_caller_ranges() {
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

    let overlaid = HighlightStyle {
        color: selection_style.color,
        background_color: selection_style.background_color,
        ..base_style()
    };
    let merged = merge_selection_highlight(BODY, base, Some(4..7), &selection_style);
    assert_eq!(
        merged,
        vec![
            (0..4, base_style()),
            (4..5, overlaid),
            (5..6, selection_style),
            (6..7, overlaid),
            (7..11, base_style()),
        ]
    );

    // Degenerate and out-of-range caller ranges never reach StyledText.
    let messy = vec![(3..3, base_style()), (9..10_000, base_style())];
    let cleaned = merge_selection_highlight(BODY, messy, None, &selection_style);
    assert_eq!(cleaned.len(), 1);
    assert_eq!(cleaned[0].0, 9..BODY.len());
}

#[test]
fn keystroke_predicates_match_copy_and_select_all_only() {
    assert!(is_copy_keystroke(
        "c",
        &modifiers(true, false, false, false)
    ));
    assert!(is_copy_keystroke(
        "C",
        &modifiers(false, true, false, false)
    ));
    assert!(is_select_all_keystroke(
        "a",
        &modifiers(true, false, false, false)
    ));
    assert!(is_select_all_keystroke(
        "A",
        &modifiers(false, true, false, false)
    ));

    assert!(!is_copy_keystroke(
        "c",
        &modifiers(false, false, false, false)
    ));
    assert!(!is_copy_keystroke(
        "c",
        &modifiers(true, false, true, false)
    ));
    assert!(!is_copy_keystroke(
        "c",
        &modifiers(true, false, false, true)
    ));
    assert!(!is_copy_keystroke(
        "x",
        &modifiers(true, false, false, false)
    ));
    assert!(!is_select_all_keystroke(
        "a",
        &modifiers(false, false, false, false)
    ));
    assert!(!is_select_all_keystroke(
        "a",
        &modifiers(true, false, true, false)
    ));
    assert!(!is_select_all_keystroke(
        "c",
        &modifiers(true, false, false, false)
    ));
}

#[test]
fn merge_overlays_wash_preserving_nested_weight_and_style() {
    let theme = ArtisanTheme::for_mode(ThemeMode::Light);
    let wash = selection_style_for_theme(theme);
    let bold = HighlightStyle {
        font_weight: Some(FontWeight::BOLD),
        ..Default::default()
    };
    let italic = HighlightStyle {
        font_style: Some(FontStyle::Italic),
        ..Default::default()
    };
    let bold_wash = HighlightStyle {
        color: wash.color,
        background_color: wash.background_color,
        ..bold
    };
    let italic_wash = HighlightStyle {
        color: wash.color,
        background_color: wash.background_color,
        ..italic
    };

    let text = "01234567890";
    let base = vec![(0..4, bold), (4..7, italic), (7..11, bold)];
    let merged = merge_selection_highlight(text, base, Some(2..9), &wash);

    assert_eq!(
        merged,
        vec![
            (0..2, bold),
            (2..4, bold_wash),
            (4..7, italic_wash),
            (7..9, bold_wash),
            (9..11, bold),
        ]
    );
}

#[test]
fn same_length_replacement_clears_selection() {
    let state = SelectableTextState::new();
    state.validate_for_text("abc");
    state.select_all();
    assert_eq!(state.selection_range(), Some(0..3));

    // Same byte count, different content: the old range must not survive.
    state.validate_for_text("abd");
    assert!(!state.has_selection());
    assert_eq!(state.selection_range(), None);
    assert_eq!(state.selected_text("abd"), "");
}

#[test]
fn live_selection_tracks_drag_head_before_release() {
    let state = SelectableTextState::new();
    state.validate_for_text(BODY);

    state.begin_drag(2);
    assert_eq!(state.selection_range(), None);

    let _ = state.update_drag(8);
    assert!(state.is_dragging());
    assert_eq!(state.selection_range(), Some(2..8));
    assert_eq!(state.selected_text(BODY), "llo wo");
    assert!(state.suppresses_click());
    assert_eq!(state.copy_text(BODY), Some("llo wo".to_owned()));

    assert!(state.end_drag(BODY));
    assert_eq!(state.selection_range(), Some(2..8));
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

struct RetainedProbe {
    text: String,
    with_link: bool,
    fired: Rc<Cell<(u32, usize)>>,
}

impl RetainedProbe {
    fn new(_cx: &mut Context<Self>, text: &str, with_link: bool) -> Self {
        Self {
            text: text.to_owned(),
            with_link,
            fired: Rc::new(Cell::new((0, 0))),
        }
    }
}

impl Render for RetainedProbe {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let element = SelectableText::retained(
            "retained-text",
            self.text.clone(),
            ArtisanTheme::for_mode(ThemeMode::Light),
            Vec::new(),
        );
        let element = if self.with_link {
            let fired = self.fired.clone();
            let range = 0..self.text.len();
            element.links(vec![range], move |range_index, _, _| {
                let (count, _) = fired.get();
                fired.set((count + 1, range_index));
            })
        } else {
            element
        };
        div()
            .w(px(400.0))
            .debug_selector(|| "retained-wrap".to_owned())
            .child(element)
    }
}

/// Resolves drag points inside the painted text line: just inside the left
/// edge, just inside the right wrapper edge (past the line end, so the head
/// clamps to the text end regardless of font metrics), and mid-line.
fn wrap_points(
    cx: &mut VisualTestContext,
    selector: &'static str,
) -> (Point<Pixels>, Point<Pixels>, Point<Pixels>) {
    let bounds = cx.debug_bounds(selector).expect("wrap must paint");
    let left = point(bounds.origin.x + px(1.0), bounds.origin.y + px(10.0));
    let right = point(bounds.origin.x + px(399.0), bounds.origin.y + px(10.0));
    let inside = point(bounds.origin.x + px(10.0), bounds.origin.y + px(10.0));
    (left, right, inside)
}

fn retained_points(cx: &mut VisualTestContext) -> (Point<Pixels>, Point<Pixels>, Point<Pixels>) {
    wrap_points(cx, "retained-wrap")
}

fn read_clipboard(cx: &mut VisualTestContext) -> Option<String> {
    cx.update(|_, app| {
        app.read_from_clipboard()
            .as_ref()
            .and_then(gpui::ClipboardItem::text)
    })
}

#[gpui::test]
fn retained_drag_selects_live_range_before_mouse_up(cx: &mut TestAppContext) {
    let (_view, cx) = cx.add_window_view(|_, cx| RetainedProbe::new(cx, BODY, false));
    cx.run_until_parked();
    let (left, right, _) = retained_points(cx);

    cx.simulate_mouse_down(left, gpui::MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_move(right, gpui::MouseButton::Left, Modifiers::default());
    // No mouse-up yet: copying must serve the live drag range.
    cx.simulate_keystrokes("ctrl-c");
    cx.run_until_parked();

    assert_eq!(read_clipboard(cx), Some(BODY.to_owned()));

    cx.simulate_mouse_up(right, gpui::MouseButton::Left, Modifiers::default());
    cx.run_until_parked();
}

#[gpui::test]
fn retained_drag_finalizes_on_release_then_copies(cx: &mut TestAppContext) {
    let (_view, cx) = cx.add_window_view(|_, cx| RetainedProbe::new(cx, BODY, false));
    cx.run_until_parked();
    let (left, right, _) = retained_points(cx);

    cx.simulate_mouse_down(left, gpui::MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_move(right, gpui::MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_up(right, gpui::MouseButton::Left, Modifiers::default());
    cx.simulate_keystrokes("ctrl-c");
    cx.run_until_parked();

    assert_eq!(read_clipboard(cx), Some(BODY.to_owned()));
}

#[gpui::test]
fn retained_streaming_text_change_resets_selection(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, cx| RetainedProbe::new(cx, BODY, false));
    cx.run_until_parked();
    let (left, right, _) = retained_points(cx);

    cx.simulate_mouse_down(left, gpui::MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_move(right, gpui::MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_up(right, gpui::MouseButton::Left, Modifiers::default());
    cx.run_until_parked();

    cx.update(|_, app| {
        view.update(app, |probe, cx| {
            probe.text = "revised".to_owned();
            cx.notify();
        });
    });
    cx.run_until_parked();

    // The press focused the retained handle, so this copy would serve the
    // old bytes if the fingerprint reset had not cleared them.
    cx.simulate_keystrokes("ctrl-c");
    cx.run_until_parked();
    assert_eq!(read_clipboard(cx), None);

    // The element stays healthy: select-all copies the revised text.
    cx.simulate_keystrokes("ctrl-a");
    cx.run_until_parked();
    cx.simulate_keystrokes("ctrl-c");
    cx.run_until_parked();
    assert_eq!(read_clipboard(cx), Some("revised".to_owned()));
}

#[gpui::test]
fn retained_link_click_fires_on_clean_press(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, cx| RetainedProbe::new(cx, BODY, true));
    cx.run_until_parked();
    let (_, _, inside) = retained_points(cx);

    cx.simulate_mouse_down(inside, gpui::MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_up(inside, gpui::MouseButton::Left, Modifiers::default());
    cx.run_until_parked();

    cx.update(|_, app| {
        assert_eq!(view.read(app).fired.get(), (1, 0));
    });
}

#[gpui::test]
fn retained_link_drag_suppresses_activation(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, cx| RetainedProbe::new(cx, BODY, true));
    cx.run_until_parked();
    let (left, right, _) = retained_points(cx);

    cx.simulate_mouse_down(left, gpui::MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_move(right, gpui::MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_up(right, gpui::MouseButton::Left, Modifiers::default());
    cx.simulate_keystrokes("ctrl-c");
    cx.run_until_parked();

    cx.update(|_, app| {
        assert_eq!(view.read(app).fired.get(), (0, 0));
    });
    assert_eq!(read_clipboard(cx), Some(BODY.to_owned()));
}

#[gpui::test]
fn retained_click_focuses_then_keyboard_selects_all(cx: &mut TestAppContext) {
    let (_view, cx) = cx.add_window_view(|_, cx| RetainedProbe::new(cx, BODY, false));
    cx.run_until_parked();
    let (_, _, inside) = retained_points(cx);

    cx.simulate_mouse_down(inside, gpui::MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_up(inside, gpui::MouseButton::Left, Modifiers::default());
    cx.simulate_keystrokes("ctrl-a");
    cx.run_until_parked();
    cx.simulate_keystrokes("ctrl-c");
    cx.run_until_parked();

    assert_eq!(read_clipboard(cx), Some(BODY.to_owned()));
}

/// Retained text nested under a separately focusable parent, mirroring the
/// transcript `ScrollArea` around message bodies. The child's press must
/// keep focus (via the native `Div` prevent-default convention) instead of
/// letting the ancestor steal it during bubbling.
struct NestedFocusProbe {
    parent_focus: FocusHandle,
    text: String,
}

impl NestedFocusProbe {
    fn new(cx: &mut Context<Self>) -> Self {
        Self {
            parent_focus: cx.focus_handle(),
            text: BODY.to_owned(),
        }
    }
}

impl Render for NestedFocusProbe {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .w(px(400.0))
            .track_focus(&self.parent_focus)
            .debug_selector(|| "nested-wrap".to_owned())
            .child(SelectableText::retained(
                "nested-text",
                self.text.clone(),
                ArtisanTheme::for_mode(ThemeMode::Light),
                Vec::new(),
            ))
    }
}

#[gpui::test]
fn nested_child_keeps_focus_against_focusable_parent(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, cx| NestedFocusProbe::new(cx));
    cx.run_until_parked();
    cx.update(|window, app| {
        let parent = view.read(app).parent_focus.clone();
        window.focus(&parent, app);
    });
    cx.run_until_parked();
    let (left, right, _) = wrap_points(cx, "nested-wrap");

    cx.simulate_mouse_down(left, gpui::MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_move(right, gpui::MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_up(right, gpui::MouseButton::Left, Modifiers::default());
    cx.run_until_parked();

    cx.update(|window, app| {
        assert!(
            !view.read(app).parent_focus.is_focused(window),
            "the focusable ancestor must not steal the child's press focus"
        );
    });
    cx.simulate_keystrokes("ctrl-c");
    cx.run_until_parked();

    assert_eq!(read_clipboard(cx), Some(BODY.to_owned()));
}

/// The real element pipeline: caller code ranges (BOLD weight in the
/// highlights, mono family with zero spacing in the overrides) merged with
/// a selection wash that splits the code range. Every mono sub-run must
/// keep its family, spacing, and weight — the wash only touches
/// foreground/background — and coverage must stay exact.
#[test]
fn text_run_pipeline_keeps_mono_through_selection_split() {
    let theme = ArtisanTheme::for_mode(ThemeMode::Light);
    let wash = selection_style_for_theme(theme);
    let body_family: SharedString = "Body".into();
    let mono_family: SharedString = "Mono".into();
    let text = "body `code` tail";
    let start = text.find("code").expect("fixture contains code");
    let code = start..start + "code".len();
    let select = start + 2..text.len();
    let default = TextStyle {
        font_family: body_family.clone(),
        letter_spacing: Some(px(1.5)),
        ..TextStyle::default()
    };
    let highlights = vec![(
        code.clone(),
        HighlightStyle {
            font_weight: Some(FontWeight::BOLD),
            ..Default::default()
        },
    )];
    let merged = merge_selection_highlight(text, highlights, Some(select.clone()), &wash);
    let overrides = vec![TextRunOverride {
        range: code.clone(),
        font_family: Some(mono_family.clone()),
        letter_spacing: Some(px(0.0)),
    }];
    let runs = compile_text_runs(text, &default, &merged, &overrides);
    assert_eq!(runs.iter().map(|run| run.len).sum::<usize>(), text.len());

    let mut offset = 0_usize;
    let mut saw_mono_selected = false;
    for run in &runs {
        let end = offset + run.len;
        if code.start <= offset && end <= code.end {
            assert_eq!(run.font.family, mono_family);
            assert_eq!(run.letter_spacing, Some(px(0.0)));
            assert_eq!(run.font.weight, FontWeight::BOLD);
            if select.start <= offset && end <= select.end {
                assert_eq!(run.background_color, wash.background_color);
                saw_mono_selected = true;
            }
        } else {
            assert_eq!(run.font.family, body_family);
            assert_eq!(run.letter_spacing, Some(px(1.5)));
        }
        offset = end;
    }
    assert_eq!(offset, text.len());
    assert!(
        saw_mono_selected,
        "the selection must split the mono range to prove metric stability"
    );
}

/// Pure compiler coverage through the frozen consumer path: a mono
/// override splits body runs with exact byte coverage across
/// unicode/emoji, and every boundary stays on a character boundary.
#[test]
fn compiler_splits_mono_with_exact_coverage() {
    let body_family: SharedString = "Body".into();
    let mono_family: SharedString = "Mono".into();
    let default = TextStyle {
        font_family: body_family.clone(),
        letter_spacing: Some(px(1.5)),
        ..TextStyle::default()
    };
    let text = "a💡b code";
    let code = text
        .find("code")
        .map(|s| s..s + "code".len())
        .unwrap_or(0..0);
    let runs = compile_text_runs(
        text,
        &default,
        &[],
        &[TextRunOverride {
            range: code.clone(),
            font_family: Some(mono_family.clone()),
            letter_spacing: Some(px(0.0)),
        }],
    );
    assert_eq!(runs.iter().map(|run| run.len).sum::<usize>(), text.len());
    let mut offset = 0_usize;
    let mut saw_mono = false;
    let mut saw_body = false;
    for run in &runs {
        let end = offset + run.len;
        assert!(text.is_char_boundary(offset));
        assert!(text.is_char_boundary(end));
        if code.start <= offset && end <= code.end {
            assert_eq!(run.font.family, mono_family);
            assert_eq!(run.letter_spacing, Some(px(0.0)));
            saw_mono = true;
        } else {
            assert_eq!(run.font.family, body_family);
            assert_eq!(run.letter_spacing, Some(px(1.5)));
            saw_body = true;
        }
        offset = end;
    }
    assert!(saw_mono && saw_body);
}

/// Same-weight overlaps merge deterministically under a mono override:
/// every run keeps BOLD, the override span keeps its family and zero
/// spacing, and coverage stays exact.
#[test]
fn compiler_merges_agreeing_overlaps_deterministically() {
    let body_family: SharedString = "Body".into();
    let mono_family: SharedString = "Mono".into();
    let default = TextStyle {
        font_family: body_family.clone(),
        letter_spacing: Some(px(1.5)),
        ..TextStyle::default()
    };
    let text = "0123456789";
    let bold = HighlightStyle {
        font_weight: Some(FontWeight::BOLD),
        ..Default::default()
    };
    let runs = compile_text_runs(
        text,
        &default,
        &[(0..6, bold), (4..10, bold)],
        &[TextRunOverride {
            range: 2..8,
            font_family: Some(mono_family.clone()),
            letter_spacing: Some(px(0.0)),
        }],
    );
    assert_eq!(runs.iter().map(|run| run.len).sum::<usize>(), text.len());
    let mut offset = 0_usize;
    for run in &runs {
        let end = offset + run.len;
        assert_eq!(run.font.weight, FontWeight::BOLD);
        if 2 <= offset && end <= 8 {
            assert_eq!(run.font.family, mono_family);
            assert_eq!(run.letter_spacing, Some(px(0.0)));
        } else {
            assert_eq!(run.font.family, body_family);
        }
        offset = end;
    }
    assert_eq!(offset, text.len());
}

/// Unsorted highlight input normalizes by position: the same ranges in
/// either order compile to identical runs.
#[test]
fn compiler_normalizes_unsorted_highlights() {
    let default = TextStyle {
        font_family: "Body".into(),
        letter_spacing: None,
        ..TextStyle::default()
    };
    let text = "0123456789";
    let ordered = vec![
        (
            0..4,
            HighlightStyle {
                font_weight: Some(FontWeight::NORMAL),
                ..Default::default()
            },
        ),
        (
            6..10,
            HighlightStyle {
                font_weight: Some(FontWeight::BOLD),
                ..Default::default()
            },
        ),
    ];
    let mut reversed = ordered.clone();
    reversed.reverse();
    assert_eq!(
        compile_text_runs(text, &default, &ordered, &[]),
        compile_text_runs(text, &default, &reversed, &[])
    );
    let runs = compile_text_runs(text, &default, &reversed, &[]);
    assert_eq!(runs.iter().map(|run| run.len).sum::<usize>(), text.len());
    let mut offset = 0_usize;
    for run in &runs {
        let end = offset + run.len;
        if end <= 4 {
            assert_eq!(run.font.weight, FontWeight::NORMAL);
        } else if offset >= 6 {
            assert_eq!(run.font.weight, FontWeight::BOLD);
        } else {
            assert_eq!(run.font.weight, TextStyle::default().font_weight);
        }
        offset = end;
    }
    assert_eq!(offset, text.len());
}

/// Conflicting overlap (BOLD vs `EXTRA_BOLD`) stays fail-closed: exact
/// coverage, deterministic outer regions, and the overlap winner is one
/// of the two inputs — which one wins is unspecified by contract, so no
/// winner is asserted.
#[test]
fn compiler_conflicting_overlap_resolves_without_shift() {
    let default = TextStyle {
        font_family: "Body".into(),
        letter_spacing: None,
        ..TextStyle::default()
    };
    let text = "0123456789";
    let highlights = vec![
        (
            0..6,
            HighlightStyle {
                font_weight: Some(FontWeight::BOLD),
                ..Default::default()
            },
        ),
        (
            4..10,
            HighlightStyle {
                font_weight: Some(FontWeight::EXTRA_BOLD),
                ..Default::default()
            },
        ),
    ];
    let runs = compile_text_runs(text, &default, &highlights, &[]);
    assert_eq!(runs.iter().map(|run| run.len).sum::<usize>(), text.len());
    let mut offset = 0_usize;
    for run in &runs {
        let end = offset + run.len;
        if end <= 4 {
            assert_eq!(run.font.weight, FontWeight::BOLD);
        } else if offset >= 6 {
            assert_eq!(run.font.weight, FontWeight::EXTRA_BOLD);
        } else {
            assert!(
                run.font.weight == FontWeight::BOLD || run.font.weight == FontWeight::EXTRA_BOLD,
                "overlap winner must be one of the two inputs"
            );
        }
        offset = end;
    }
    assert_eq!(offset, text.len());
}

/// Invalid ranges drop entirely without shifting surviving text: the
/// mid-emoji and out-of-bounds markers must not clamp onto the emoji,
/// and the overlapping override loses to the earliest range.
#[test]
#[expect(
    clippy::reversed_empty_ranges,
    reason = "reversed and empty ranges are deliberate invalid inputs for the fail-closed compiler"
)]
fn compiler_drops_invalid_ranges_entirely() {
    let body_family: SharedString = "Body".into();
    let mono_family: SharedString = "Mono".into();
    let default = TextStyle {
        font_family: body_family.clone(),
        letter_spacing: Some(px(1.5)),
        ..TextStyle::default()
    };
    // "a💡b": `a` is 0..1, `💡` is 1..5, `b` is 5..6.
    let text = "a💡b";
    let marker = HighlightStyle {
        font_weight: Some(FontWeight::BOLD),
        ..Default::default()
    };
    let highlights = vec![
        (2..4, marker),
        (0..99, marker),
        (4..2, marker),
        (1..1, marker),
        (0..1, marker),
    ];
    let overrides = vec![
        TextRunOverride {
            range: 1..2,
            font_family: Some(mono_family.clone()),
            letter_spacing: Some(px(0.0)),
        },
        TextRunOverride {
            range: 5..99,
            font_family: Some(mono_family.clone()),
            letter_spacing: Some(px(0.0)),
        },
        TextRunOverride {
            range: 0..1,
            font_family: Some(mono_family.clone()),
            letter_spacing: Some(px(0.0)),
        },
        TextRunOverride {
            range: 0..5,
            font_family: Some("Other".into()),
            letter_spacing: None,
        },
    ];
    let runs = compile_text_runs(text, &default, &highlights, &overrides);
    assert_eq!(runs.iter().map(|run| run.len).sum::<usize>(), text.len());
    let mut offset = 0_usize;
    for run in &runs {
        let end = offset + run.len;
        if end <= 1 {
            // The one valid highlight plus the one valid override.
            assert_eq!(run.font.weight, FontWeight::BOLD);
            assert_eq!(run.font.family, mono_family);
        } else {
            assert_eq!(run.font.weight, TextStyle::default().font_weight);
            assert_eq!(run.font.family, body_family);
            assert_eq!(run.letter_spacing, Some(px(1.5)));
        }
        offset = end;
    }
    assert_eq!(offset, text.len());
}

/// Empty text yields zero runs without touching invalid ranges; plain
/// text coalesces to a single default run.
#[test]
fn compiler_empty_and_plain_edge_cases() {
    let default = TextStyle {
        font_family: "Body".into(),
        letter_spacing: Some(px(1.5)),
        ..TextStyle::default()
    };
    let bold = HighlightStyle {
        font_weight: Some(FontWeight::BOLD),
        ..Default::default()
    };
    let empty = compile_text_runs(
        "",
        &default,
        &[(0..1, bold)],
        &[TextRunOverride {
            range: 0..1,
            font_family: Some("Mono".into()),
            letter_spacing: Some(px(0.0)),
        }],
    );
    assert!(empty.is_empty());

    let plain = compile_text_runs("hello", &default, &[], &[]);
    assert_eq!(plain.len(), 1);
    let first = plain.first().expect("plain text compiles to one run");
    assert_eq!(first.len, 5);
    assert_eq!(first.font.family, SharedString::from("Body"));
    assert_eq!(first.letter_spacing, Some(px(1.5)));
}

/// Selectable text with a mono override, mirroring a transcript body with
/// one inline-code span. Drag-selecting across the family boundary must
/// still hit-test every byte and copy the exact plaintext.
struct OverrideProbe {
    text: String,
    code_end: usize,
    with_link: bool,
    fired: Rc<Cell<(u32, usize)>>,
}

impl OverrideProbe {
    fn new(_cx: &mut Context<Self>, text: &str, code_end: usize, with_link: bool) -> Self {
        Self {
            text: text.to_owned(),
            code_end,
            with_link,
            fired: Rc::new(Cell::new((0, 0))),
        }
    }
}

impl Render for OverrideProbe {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let code = 0..self.code_end;
        let element = SelectableText::retained(
            "override-text",
            self.text.clone(),
            ArtisanTheme::for_mode(ThemeMode::Light),
            vec![(
                code.clone(),
                HighlightStyle {
                    font_weight: Some(FontWeight::BOLD),
                    ..Default::default()
                },
            )],
        )
        .with_text_run_overrides(vec![TextRunOverride {
            range: code,
            font_family: Some("Mono".into()),
            letter_spacing: Some(px(0.0)),
        }]);
        let element = if self.with_link {
            let fired = self.fired.clone();
            let range = 0..self.text.len();
            element.links(vec![range], move |range_index, _, _| {
                let (count, _) = fired.get();
                fired.set((count + 1, range_index));
            })
        } else {
            element
        };
        div()
            .w(px(400.0))
            .debug_selector(|| "override-wrap".to_owned())
            .child(element)
    }
}

fn override_points(cx: &mut VisualTestContext) -> (Point<Pixels>, Point<Pixels>, Point<Pixels>) {
    wrap_points(cx, "override-wrap")
}

#[gpui::test]
fn overrides_keep_drag_copy_exact(cx: &mut TestAppContext) {
    const TEXT: &str = "code body here";
    let (_view, cx) = cx.add_window_view(|_, cx| OverrideProbe::new(cx, TEXT, 4, false));
    cx.run_until_parked();
    let (left, right, _) = override_points(cx);

    cx.simulate_mouse_down(left, gpui::MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_move(right, gpui::MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_up(right, gpui::MouseButton::Left, Modifiers::default());
    cx.simulate_keystrokes("ctrl-c");
    cx.run_until_parked();

    assert_eq!(read_clipboard(cx), Some(TEXT.to_owned()));
}

#[gpui::test]
fn overrides_keep_link_click_firing(cx: &mut TestAppContext) {
    const TEXT: &str = "code body here";
    let (view, cx) = cx.add_window_view(|_, cx| OverrideProbe::new(cx, TEXT, 4, true));
    cx.run_until_parked();
    let (_, _, inside) = override_points(cx);

    cx.simulate_mouse_down(inside, gpui::MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_up(inside, gpui::MouseButton::Left, Modifiers::default());
    cx.run_until_parked();

    cx.update(|_, app| {
        assert_eq!(view.read(app).fired.get(), (1, 0));
    });
}

#[gpui::test]
fn overrides_keep_link_drag_suppressed(cx: &mut TestAppContext) {
    const TEXT: &str = "code body here";
    let (view, cx) = cx.add_window_view(|_, cx| OverrideProbe::new(cx, TEXT, 4, true));
    cx.run_until_parked();
    let (left, right, _) = override_points(cx);

    cx.simulate_mouse_down(left, gpui::MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_move(right, gpui::MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_up(right, gpui::MouseButton::Left, Modifiers::default());
    cx.simulate_keystrokes("ctrl-c");
    cx.run_until_parked();

    cx.update(|_, app| {
        assert_eq!(view.read(app).fired.get(), (0, 0));
    });
    assert_eq!(read_clipboard(cx), Some(TEXT.to_owned()));
}
