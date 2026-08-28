use std::cell::RefCell;
use std::rc::Rc;

use artisan_ui::popover::{
    Popover, PopoverAlign, PopoverChangeReason, PopoverOffset, PopoverPlacement, PopoverSide,
    PopoverState, PopoverStyle, PopoverVariant, popover_content,
};
use artisan_ui::theme::{ArtisanTheme, RadiusStep, RadiusTokens, ShadowLayer, ThemeMode};
use gpui::{
    Bounds, Context, FocusHandle, InteractiveElement, IntoElement, Modifiers, MouseButton,
    ParentElement, Render, Styled, TestAppContext, Window, div, point, px, size,
};

const POPOVER_ID: &str = "native-popover-id";
const POPOVER_SELECTOR: &str = "native-popover-under-test";
const TRIGGER_SELECTOR: &str = "native-popover-under-test-trigger";
const CONTENT_SELECTOR: &str = "native-popover-under-test-content";
const CONTENT_CHILD_SELECTOR: &str = "native-popover-content-child";

fn rect(x: f32, y: f32, width: f32, height: f32) -> Bounds<gpui::Pixels> {
    Bounds::new(point(px(x), px(y)), size(px(width), px(height)))
}

fn assert_shadow_stack_matches(actual: &[ShadowLayer; 4], expected: &[ShadowLayer; 4]) {
    for (actual, expected) in actual.iter().zip(expected.iter()) {
        assert_eq!(actual.color, expected.color);
        assert_eq!(actual.offset_x, expected.offset_x);
        assert_eq!(actual.offset_y, expected.offset_y);
        assert_eq!(actual.blur_radius, expected.blur_radius);
        assert_eq!(actual.spread_radius, expected.spread_radius);
    }
}

#[test]
fn state_and_placement_defaults_are_explicit() {
    let closed = PopoverState::new(false, false);
    assert!(!closed.is_open());
    assert!(!closed.is_disabled());
    assert_eq!(closed.requested_toggle(), Some(true));
    assert!(!closed.requests_dismissal());

    let open = PopoverState::new(true, false);
    assert!(open.open());
    assert_eq!(open.requested_toggle(), Some(false));
    assert!(open.requests_dismissal());

    let disabled = PopoverState::new(false, true);
    assert_eq!(disabled.requested_toggle(), None);

    let placement = PopoverPlacement::default();
    assert_eq!(placement.side, PopoverSide::Bottom);
    assert_eq!(placement.align, PopoverAlign::Center);
    assert_eq!(placement.side_offset, px(4.0));
    assert_eq!(placement.align_offset, px(0.0));

    let changed = placement
        .side(PopoverSide::Top)
        .align(PopoverAlign::End)
        .offset(PopoverOffset::new(px(8.0), px(3.0)));
    assert_eq!(changed.side, PopoverSide::Top);
    assert_eq!(changed.align, PopoverAlign::End);
    assert_eq!(changed.offsets(), PopoverOffset::new(px(8.0), px(3.0)));
}

#[test]
fn geometry_resolves_all_sides_alignments_and_offsets() {
    let anchor = rect(200.0, 100.0, 80.0, 40.0);
    let content = size(px(120.0), px(60.0));
    let viewport = size(px(640.0), px(480.0));

    let start = PopoverPlacement::default()
        .align(PopoverAlign::Start)
        .resolve(anchor, content, viewport);
    assert_eq!(start.origin, point(px(200.0), px(144.0)));
    assert_eq!(start.side, PopoverSide::Bottom);

    let center = PopoverPlacement::default().resolve(anchor, content, viewport);
    assert_eq!(center.origin, point(px(180.0), px(144.0)));

    let end = PopoverPlacement::default()
        .align(PopoverAlign::End)
        .resolve(anchor, content, viewport);
    assert_eq!(end.origin, point(px(160.0), px(144.0)));

    let top = PopoverPlacement::new(PopoverSide::Top, PopoverAlign::Center, px(4.0), px(0.0))
        .resolve(anchor, content, viewport);
    assert_eq!(top.origin, point(px(180.0), px(36.0)));

    let left = PopoverPlacement::new(PopoverSide::Left, PopoverAlign::Center, px(4.0), px(0.0))
        .resolve(anchor, content, viewport);
    assert_eq!(left.origin, point(px(76.0), px(90.0)));

    let right = PopoverPlacement::new(PopoverSide::Right, PopoverAlign::Center, px(4.0), px(0.0))
        .resolve(anchor, content, viewport);
    assert_eq!(right.origin, point(px(284.0), px(90.0)));

    let offset = PopoverPlacement::default()
        .align_offset(px(7.0))
        .resolve(anchor, content, viewport);
    assert_eq!(offset.origin, point(px(187.0), px(144.0)));
}

#[test]
fn geometry_flips_and_shifts_deterministically_at_viewport_edges() {
    let bottom_edge = PopoverPlacement::default().resolve(
        rect(200.0, 430.0, 80.0, 30.0),
        size(px(120.0), px(80.0)),
        size(px(640.0), px(480.0)),
    );
    assert_eq!(bottom_edge.side, PopoverSide::Top);
    assert_eq!(bottom_edge.origin, point(px(180.0), px(346.0)));
    assert!(bottom_edge.is_flipped());
    assert!(!bottom_edge.is_shifted());

    let cross_axis = PopoverPlacement::default().resolve(
        rect(8.0, 100.0, 40.0, 30.0),
        size(px(200.0), px(60.0)),
        size(px(640.0), px(480.0)),
    );
    assert_eq!(cross_axis.side, PopoverSide::Bottom);
    assert_eq!(cross_axis.origin, point(px(0.0), px(134.0)));
    assert!(!cross_axis.is_flipped());
    assert!(cross_axis.is_shifted());

    let right_edge =
        PopoverPlacement::new(PopoverSide::Right, PopoverAlign::Center, px(4.0), px(0.0)).resolve(
            rect(590.0, 200.0, 30.0, 40.0),
            size(px(100.0), px(80.0)),
            size(px(640.0), px(480.0)),
        );
    assert_eq!(right_edge.side, PopoverSide::Left);
    assert_eq!(right_edge.origin, point(px(486.0), px(180.0)));
    assert!(right_edge.is_flipped());
    assert!(!right_edge.is_shifted());

    let oversized = PopoverPlacement::default().resolve(
        rect(200.0, 200.0, 20.0, 20.0),
        size(px(700.0), px(500.0)),
        size(px(640.0), px(480.0)),
    );
    assert_eq!(oversized.side, PopoverSide::Bottom);
    assert_eq!(oversized.origin, point(px(0.0), px(0.0)));
    assert!(!oversized.is_flipped());
    assert!(oversized.is_shifted());
}

#[test]
fn themed_styles_pin_default_card_and_bare_variants() {
    for mode in [ThemeMode::Light, ThemeMode::Dark] {
        let theme = ArtisanTheme::for_mode(mode);
        let style = PopoverStyle::resolve(theme, PopoverVariant::Default);

        assert_eq!(style.variant, PopoverVariant::Default);
        assert!(style.has_card_chrome());
        assert!(!style.is_bare());
        assert_eq!(style.width, px(288.0));
        assert_eq!(style.padding, px(16.0));
        assert_eq!(style.gap, px(16.0));
        assert_eq!(style.corner_radius, RadiusTokens::value(RadiusStep::X2l));
        assert_eq!(style.corner_radius, px(18.0));
        assert_eq!(style.text_size, theme.typography.control_text);
        assert_eq!(style.text_size, px(14.0));
        assert_eq!(style.line_height, px(20.0));
        assert_eq!(style.background, theme.colors.popover.to_paint());
        assert_eq!(style.foreground, theme.colors.popover_foreground.to_paint());
        assert_shadow_stack_matches(&style.card_shadow, &theme.elevation.card_shadow);
        assert_eq!(style.shadows().len(), 4);

        let bare = PopoverStyle::bare(theme);
        assert_eq!(bare.variant, PopoverVariant::Bare);
        assert!(!bare.has_card_chrome());
        assert!(bare.is_bare());
        assert_eq!(bare.background, style.background);
        assert_eq!(bare.foreground, style.foreground);
    }
}

#[test]
fn content_recipe_accepts_default_and_caller_material_variants() {
    let theme = ArtisanTheme::for_mode(ThemeMode::Dark);

    let default_content = popover_content(
        PopoverStyle::default_card(theme),
        div().h(px(24.0)).child("default"),
    );
    let bare_content = popover_content(PopoverStyle::bare(theme), div().h(px(24.0)).child("bare"));

    let _ = (default_content, bare_content);
}

struct PopoverProbe {
    focus: FocusHandle,
    open: bool,
    disabled: bool,
    changes: Rc<RefCell<Vec<(bool, PopoverChangeReason)>>>,
}

impl PopoverProbe {
    fn new(
        cx: &mut Context<Self>,
        changes: Rc<RefCell<Vec<(bool, PopoverChangeReason)>>>,
        open: bool,
        disabled: bool,
    ) -> Self {
        Self {
            focus: cx.focus_handle(),
            open,
            disabled,
            changes,
        }
    }
}

impl Render for PopoverProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let changes = Rc::clone(&self.changes);
        let popover = Popover::new(
            POPOVER_ID,
            self.focus.clone(),
            ArtisanTheme::for_mode(ThemeMode::Dark),
            self.open,
            div()
                .w(px(96.0))
                .h(px(32.0))
                .debug_selector(|| TRIGGER_SELECTOR.to_string())
                .child("Open"),
            div()
                .h(px(64.0))
                .debug_selector(|| CONTENT_CHILD_SELECTOR.to_string())
                .child("Content"),
        )
        .disabled(self.disabled)
        .debug_selector(POPOVER_SELECTOR)
        .on_open_change(move |open, reason, _, _| {
            changes.borrow_mut().push((open, reason));
        });

        div().size_full().p(px(100.0)).child(popover)
    }
}

#[gpui::test]
fn render_probe_exercises_controlled_trigger_content_and_outside_policy(cx: &mut TestAppContext) {
    let changes = Rc::new(RefCell::new(Vec::new()));
    let changes_for_view = Rc::clone(&changes);
    let (view, cx) =
        cx.add_window_view(move |_, cx| PopoverProbe::new(cx, changes_for_view, false, false));
    cx.run_until_parked();

    assert!(cx.debug_bounds(POPOVER_SELECTOR).is_some());
    let trigger = cx
        .debug_bounds(TRIGGER_SELECTOR)
        .expect("closed popover trigger must paint");
    assert!(cx.debug_bounds(CONTENT_SELECTOR).is_none());

    cx.simulate_click(trigger.center(), Modifiers::none());
    assert_eq!(
        changes.borrow().clone(),
        vec![(true, PopoverChangeReason::Trigger)]
    );

    cx.update(|_, app| {
        view.update(app, |probe, cx| {
            probe.open = true;
            cx.notify();
        });
    });
    cx.run_until_parked();

    let trigger = cx
        .debug_bounds(TRIGGER_SELECTOR)
        .expect("open popover trigger must paint");
    let content = cx
        .debug_bounds(CONTENT_SELECTOR)
        .expect("controlled open state must mount content");
    let content_child = cx
        .debug_bounds(CONTENT_CHILD_SELECTOR)
        .expect("content child must paint through the deferred layer");

    assert_eq!(content.origin.y - trigger.bottom(), px(4.0));
    assert_eq!(content.center().x, trigger.center().x);
    assert_eq!(content_child.origin.y - content.origin.y, px(16.0));

    cx.simulate_click(content.center(), Modifiers::none());
    cx.run_until_parked();
    assert_eq!(changes.borrow().len(), 1);

    cx.simulate_click(point(px(20.0), px(20.0)), Modifiers::none());
    assert_eq!(
        changes.borrow().clone(),
        vec![
            (true, PopoverChangeReason::Trigger),
            (false, PopoverChangeReason::Outside),
        ]
    );

    cx.update(|_, app| {
        view.update(app, |probe, cx| {
            probe.open = false;
            cx.notify();
        });
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds(CONTENT_SELECTOR).is_none());
}

#[gpui::test]
fn escape_requests_controlled_dismissal_once(cx: &mut TestAppContext) {
    let changes = Rc::new(RefCell::new(Vec::new()));
    let changes_for_view = Rc::clone(&changes);
    let (view, cx) = cx.add_window_view(move |window, cx| {
        let probe = PopoverProbe::new(cx, changes_for_view, true, false);
        window.focus(&probe.focus);
        probe
    });
    cx.run_until_parked();

    assert!(cx.debug_bounds(CONTENT_SELECTOR).is_some());
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();

    assert_eq!(
        changes.borrow().clone(),
        vec![(false, PopoverChangeReason::Escape)]
    );
    cx.update(|window, app| {
        assert!(view.read(app).focus.is_focused(window));
    });
}

#[gpui::test]
fn disabled_trigger_ignores_pointer_input_and_focus(cx: &mut TestAppContext) {
    let changes = Rc::new(RefCell::new(Vec::new()));
    let changes_for_view = Rc::clone(&changes);
    let (view, cx) =
        cx.add_window_view(move |_, cx| PopoverProbe::new(cx, changes_for_view, false, true));
    cx.run_until_parked();

    let trigger = cx
        .debug_bounds(TRIGGER_SELECTOR)
        .expect("disabled trigger must remain mounted");

    cx.simulate_mouse_down(trigger.center(), MouseButton::Right, Modifiers::none());
    cx.simulate_mouse_up(trigger.center(), MouseButton::Right, Modifiers::none());
    cx.simulate_click(trigger.center(), Modifiers::none());

    assert!(changes.borrow().is_empty());
    cx.update(|window, app| {
        assert!(!view.read(app).focus.is_focused(window));
    });
}
