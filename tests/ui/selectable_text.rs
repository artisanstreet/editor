//! Behavioral coverage for native selectable transcript text.
//!
//! Pure keystroke and run-compiler tests need no window. The GPUI tests
//! mount retained [`SelectableText`] probes and drive real pointer
//! gestures and the focused keyboard paths (`control-a` select-all,
//! `control-c` copy through the platform clipboard), including selections
//! that span several text leaves.

use artisan_ui::selectable_text::{
    SelectableText, TextRunOverride, clamp_to_char_boundary, compile_text_runs, is_copy_keystroke,
    is_select_all_keystroke,
};
use artisan_ui::theme::{ArtisanTheme, ThemeMode};
use gpui::{
    Context, FocusHandle, FontWeight, HighlightStyle, InteractiveElement, IntoElement, Modifiers,
    ParentElement, Pixels, Point, Render, SharedString, Styled, TestAppContext, TextStyle,
    VisualTestContext, Window, div, point, px,
};
use std::cell::Cell;
use std::rc::Rc;

const BODY: &str = "hello world";
const EMOJI_BODY: &str = "a💡b";

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

#[gpui::test]
fn copy_without_a_selection_leaves_clipboard_alone(cx: &mut TestAppContext) {
    let (_view, cx) = cx.add_window_view(|_, cx| RetainedProbe::new(cx, BODY, false));
    cx.run_until_parked();
    let (_, _, inside) = retained_points(cx);

    // A click focuses the leaf but selects nothing.
    cx.simulate_mouse_down(inside, gpui::MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_up(inside, gpui::MouseButton::Left, Modifiers::default());
    cx.simulate_keystrokes("ctrl-c");
    cx.run_until_parked();

    assert_eq!(read_clipboard(cx), None);
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
        letter_spacing: Some(px(1.5).into()),
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
        letter_spacing: Some(px(1.5).into()),
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
        letter_spacing: Some(px(1.5).into()),
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
        letter_spacing: Some(px(1.5).into()),
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

/// Two stacked paragraphs, each its own leaf, like a transcript body.
struct ParagraphsProbe {
    first: String,
    second: String,
}

impl ParagraphsProbe {
    fn new(_cx: &mut Context<Self>, first: &str, second: &str) -> Self {
        Self {
            first: first.to_owned(),
            second: second.to_owned(),
        }
    }
}

impl Render for ParagraphsProbe {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let theme = ArtisanTheme::for_mode(ThemeMode::Dark);
        div()
            .w(px(400.0))
            .h(px(300.0))
            .flex()
            .flex_col()
            .gap(px(16.0))
            .debug_selector(|| "paragraphs".to_owned())
            .child(div().debug_selector(|| "paragraph-one".to_owned()).child(
                SelectableText::retained("paragraph-one", self.first.clone(), theme, Vec::new()),
            ))
            .child(div().debug_selector(|| "paragraph-two".to_owned()).child(
                SelectableText::retained("paragraph-two", self.second.clone(), theme, Vec::new()),
            ))
    }
}

fn press_with_count(cx: &mut VisualTestContext, position: Point<Pixels>, click_count: usize) {
    cx.simulate_event(gpui::MouseDownEvent {
        button: gpui::MouseButton::Left,
        position,
        modifiers: Modifiers::default(),
        click_count,
        first_mouse: false,
    });
    cx.simulate_event(gpui::MouseUpEvent {
        button: gpui::MouseButton::Left,
        position,
        modifiers: Modifiers::default(),
        click_count,
    });
}

fn copy(cx: &mut VisualTestContext) -> Option<String> {
    cx.simulate_keystrokes("ctrl-c");
    cx.run_until_parked();
    read_clipboard(cx)
}

#[gpui::test]
fn drag_from_one_paragraph_into_the_next_selects_across(cx: &mut TestAppContext) {
    let (_view, cx) =
        cx.add_window_view(|_, cx| ParagraphsProbe::new(cx, "Two things", "Slow saves at idle"));
    cx.run_until_parked();
    let one = cx
        .debug_bounds("paragraph-one")
        .expect("first paragraph paints");
    let two = cx
        .debug_bounds("paragraph-two")
        .expect("second paragraph paints");
    let start = point(one.origin.x + px(1.0), one.origin.y + px(5.0));
    let end = point(two.origin.x + px(399.0), two.origin.y + px(5.0));

    cx.simulate_mouse_down(start, gpui::MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_move(end, gpui::MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_up(end, gpui::MouseButton::Left, Modifiers::default());
    cx.run_until_parked();

    assert_eq!(
        copy(cx),
        Some("Two things\n\nSlow saves at idle".to_owned())
    );
}

#[gpui::test]
fn dragging_into_the_gap_below_text_selects_to_its_end(cx: &mut TestAppContext) {
    let (_view, cx) =
        cx.add_window_view(|_, cx| ParagraphsProbe::new(cx, "Two things", "Slow saves at idle"));
    cx.run_until_parked();
    let one = cx
        .debug_bounds("paragraph-one")
        .expect("first paragraph paints");
    let two = cx
        .debug_bounds("paragraph-two")
        .expect("second paragraph paints");
    let start = point(one.origin.x + px(1.0), one.origin.y + px(5.0));
    let below = point(
        two.origin.x + px(20.0),
        two.origin.y + two.size.height + px(40.0),
    );

    cx.simulate_mouse_down(start, gpui::MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_move(below, gpui::MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_up(below, gpui::MouseButton::Left, Modifiers::default());
    cx.run_until_parked();

    assert_eq!(
        copy(cx),
        Some("Two things\n\nSlow saves at idle".to_owned())
    );
}

#[gpui::test]
fn double_click_selects_the_word_under_the_pointer(cx: &mut TestAppContext) {
    let (_view, cx) = cx.add_window_view(|_, cx| ParagraphsProbe::new(cx, "hello world", "second"));
    cx.run_until_parked();
    let one = cx
        .debug_bounds("paragraph-one")
        .expect("first paragraph paints");
    let on_hello = point(one.origin.x + px(6.0), one.origin.y + px(5.0));

    press_with_count(cx, on_hello, 1);
    press_with_count(cx, on_hello, 2);
    cx.run_until_parked();

    // Windows (and WSL) also take the trailing space, like Chrome there.
    let copied = copy(cx).expect("a word is selected");
    assert_eq!(copied.trim_end(), "hello");
}

#[gpui::test]
fn triple_click_selects_the_paragraph(cx: &mut TestAppContext) {
    let (_view, cx) = cx.add_window_view(|_, cx| ParagraphsProbe::new(cx, "hello world", "second"));
    cx.run_until_parked();
    let one = cx
        .debug_bounds("paragraph-one")
        .expect("first paragraph paints");
    let on_hello = point(one.origin.x + px(6.0), one.origin.y + px(5.0));

    press_with_count(cx, on_hello, 1);
    press_with_count(cx, on_hello, 2);
    press_with_count(cx, on_hello, 3);
    cx.run_until_parked();

    assert_eq!(copy(cx), Some("hello world".to_owned()));
}

#[gpui::test]
fn press_outside_the_text_clears_the_selection(cx: &mut TestAppContext) {
    let (_view, cx) =
        cx.add_window_view(|_, cx| ParagraphsProbe::new(cx, "Two things", "Slow saves at idle"));
    cx.run_until_parked();
    let one = cx
        .debug_bounds("paragraph-one")
        .expect("first paragraph paints");
    let two = cx
        .debug_bounds("paragraph-two")
        .expect("second paragraph paints");
    let start = point(one.origin.x + px(1.0), one.origin.y + px(5.0));
    let end = point(two.origin.x + px(399.0), two.origin.y + px(5.0));
    cx.simulate_mouse_down(start, gpui::MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_move(end, gpui::MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_up(end, gpui::MouseButton::Left, Modifiers::default());
    cx.run_until_parked();

    let empty = point(two.origin.x + px(20.0), two.origin.y + px(200.0));
    cx.simulate_click(empty, Modifiers::default());
    cx.run_until_parked();

    assert_eq!(copy(cx), None);
}

#[gpui::test]
fn streaming_append_keeps_the_selection(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, cx| ParagraphsProbe::new(cx, "hello world", "x"));
    cx.run_until_parked();
    let one = cx
        .debug_bounds("paragraph-one")
        .expect("first paragraph paints");
    let on_hello = point(one.origin.x + px(6.0), one.origin.y + px(5.0));
    press_with_count(cx, on_hello, 1);
    press_with_count(cx, on_hello, 2);
    cx.run_until_parked();

    cx.update(|_, app| {
        view.update(app, |probe, cx| {
            probe.first = "hello world, and more".to_owned();
            cx.notify();
        });
    });
    cx.run_until_parked();

    assert_eq!(copy(cx).as_deref().map(str::trim_end), Some("hello"));
}
