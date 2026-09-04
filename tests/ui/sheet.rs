//! Behavioral coverage for the native controlled GPUI sheet primitive.

use std::cell::Cell;
use std::rc::Rc;

use artisan_ui::motion::MotionPolicy;
use artisan_ui::sheet::{
    SHEET_CLOSE_LABEL, SHEET_DEFAULT_WIDTH, SHEET_OPEN_ANIMATION_DURATION, SHEET_OVERLAY_OPACITY,
    SHEET_ROLE, SHEET_WIDTH_FRACTION, Sheet, SheetDismissReason, SheetFocusIntent,
    SheetFocusTransition, SheetGeometry, SheetMotionPlan, SheetSide, SheetState, SheetStyle,
    sheet_focus_transition,
};
use artisan_ui::theme::{ArtisanTheme, RadiusStep, RadiusTokens, ThemeMode};
use gpui::{
    Bounds, ColorExt as _, Context, FocusHandle, Hsla, InteractiveElement, IntoElement, Modifiers,
    ParentElement,
    Render, Styled, TestAppContext, Window, div, point, px, size,
};

const ROOT_SELECTOR: &str = "native-sheet-under-test";
const CLOSED_SELECTOR: &str = "native-sheet-under-test-closed";
const OVERLAY_SELECTOR: &str = "native-sheet-under-test-overlay";
const PANEL_SELECTOR: &str = "native-sheet-under-test-panel";
const TITLE_SELECTOR: &str = "native-sheet-under-test-title";
const DESCRIPTION_SELECTOR: &str = "native-sheet-under-test-description";
const CLOSE_SELECTOR: &str = "native-sheet-under-test-close";
const BODY_SELECTOR: &str = "native-sheet-body";

#[derive(Clone)]
struct Dismissals {
    count: Rc<Cell<u32>>,
    last: Rc<Cell<Option<SheetDismissReason>>>,
}

impl Dismissals {
    fn new() -> Self {
        Self {
            count: Rc::new(Cell::new(0)),
            last: Rc::new(Cell::new(None)),
        }
    }

    fn record(&self, reason: SheetDismissReason) {
        self.count.set(self.count.get() + 1);
        self.last.set(Some(reason));
    }
}

struct SheetProbe {
    focus: FocusHandle,
    dismissals: Dismissals,
    open: bool,
    side: SheetSide,
}

impl SheetProbe {
    fn new(cx: &mut Context<Self>, dismissals: Dismissals, open: bool, side: SheetSide) -> Self {
        Self {
            focus: cx.focus_handle(),
            dismissals,
            open,
            side,
        }
    }
}

impl Render for SheetProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let dismissals = self.dismissals.clone();
        let sheet = Sheet::new(
            "native-sheet-id",
            self.focus.clone(),
            ArtisanTheme::for_mode(ThemeMode::Dark),
            self.open,
            self.side,
            "Project settings",
            div()
                .w(px(240.0))
                .h(px(96.0))
                .debug_selector(|| BODY_SELECTOR.to_string())
                .child("Choose a terminal"),
        )
        .motion_policy(MotionPolicy::Reduced)
        .description("Side panel content")
        .debug_selector(ROOT_SELECTOR)
        .on_dismiss(move |reason, _, _| dismissals.record(reason));

        div().relative().size_full().child(sheet)
    }
}

struct FocusProbe {
    entry: FocusHandle,
    restore: FocusHandle,
}

impl FocusProbe {
    fn new(cx: &mut Context<Self>) -> Self {
        Self {
            entry: cx.focus_handle(),
            restore: cx.focus_handle(),
        }
    }
}

impl Render for FocusProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .child(div().w(px(1.0)).h(px(1.0)).track_focus(&self.entry))
            .child(div().w(px(1.0)).h(px(1.0)).track_focus(&self.restore))
    }
}

#[test]
fn state_and_focus_transitions_are_controlled_and_deterministic() {
    let open = SheetState::new(true);
    let closed = SheetState::new(false);

    assert!(open.open());
    assert!(open.is_open());
    assert!(!closed.open());
    assert!(!closed.is_open());
    assert_eq!(SheetSide::default(), SheetSide::Right);
    assert_eq!(SheetSide::Right.opposite(), SheetSide::Left);
    assert_eq!(SheetSide::Left.opposite(), SheetSide::Right);
    assert_eq!(SheetSide::Top.opposite(), SheetSide::Bottom);
    assert!(SheetSide::Right.is_horizontal());
    assert!(SheetSide::Left.is_horizontal());
    assert!(!SheetSide::Top.is_horizontal());
    assert!(SheetSide::Top.is_vertical());
    assert_eq!(SheetSide::Right.as_str(), "right");

    for reason in [
        SheetDismissReason::Escape,
        SheetDismissReason::Overlay,
        SheetDismissReason::CloseButton,
    ] {
        assert_eq!(open.requested_dismissal(reason), Some(false));
        assert_eq!(closed.requested_dismissal(reason), None);
    }

    assert_eq!(open.requested_close(), Some(false));
    assert_eq!(closed.requested_close(), None);

    assert_eq!(
        sheet_focus_transition(false, false),
        SheetFocusTransition::Unchanged
    );
    assert_eq!(
        sheet_focus_transition(false, true),
        SheetFocusTransition::Enter
    );
    assert_eq!(
        sheet_focus_transition(true, true),
        SheetFocusTransition::Unchanged
    );
    assert_eq!(
        sheet_focus_transition(true, false),
        SheetFocusTransition::Restore
    );
}

#[test]
fn side_geometry_resolves_panel_and_overlay_deterministically() {
    let viewport = size(px(800.0), px(600.0));

    let right = SheetGeometry::resolve(
        viewport,
        SheetSide::Right,
        SHEET_DEFAULT_WIDTH,
        SHEET_DEFAULT_WIDTH,
        SHEET_WIDTH_FRACTION,
    );
    assert_eq!(right.side, SheetSide::Right);
    assert_eq!(right.thickness, px(384.0));
    assert_eq!(
        right.panel,
        Bounds::new(point(px(416.0), px(0.0)), size(px(384.0), px(600.0)))
    );
    assert_eq!(
        right.overlay,
        Bounds::new(point(px(0.0), px(0.0)), viewport)
    );
    assert!(right.panel_contains(point(px(700.0), px(300.0))));
    assert!(!right.panel_contains(point(px(100.0), px(300.0))));
    assert!(right.overlay_contains(point(px(100.0), px(100.0))));

    let left = SheetGeometry::resolve(
        viewport,
        SheetSide::Left,
        SHEET_DEFAULT_WIDTH,
        SHEET_DEFAULT_WIDTH,
        SHEET_WIDTH_FRACTION,
    );
    assert_eq!(
        left.panel,
        Bounds::new(point(px(0.0), px(0.0)), size(px(384.0), px(600.0)))
    );

    let top = SheetGeometry::resolve(
        viewport,
        SheetSide::Top,
        px(200.0),
        SHEET_DEFAULT_WIDTH,
        SHEET_WIDTH_FRACTION,
    );
    assert_eq!(
        top.panel,
        Bounds::new(point(px(0.0), px(0.0)), size(px(800.0), px(200.0)))
    );

    let bottom = SheetGeometry::resolve(
        viewport,
        SheetSide::Bottom,
        px(200.0),
        SHEET_DEFAULT_WIDTH,
        SHEET_WIDTH_FRACTION,
    );
    assert_eq!(
        bottom.panel,
        Bounds::new(point(px(0.0), px(400.0)), size(px(800.0), px(200.0)))
    );
}

#[test]
fn geometry_caps_thickness_by_viewport_fraction_and_max_width() {
    // Narrow viewport: 75% cap is smaller than max width.
    let narrow = SheetGeometry::resolve(
        size(px(400.0), px(600.0)),
        SheetSide::Right,
        SHEET_DEFAULT_WIDTH,
        SHEET_DEFAULT_WIDTH,
        SHEET_WIDTH_FRACTION,
    );
    assert_eq!(narrow.thickness, px(300.0));
    assert_eq!(narrow.panel.origin.x, px(100.0));

    // Tiny viewport: thickness clamps to viewport width.
    let tiny = SheetGeometry::resolve(
        size(px(20.0), px(600.0)),
        SheetSide::Right,
        px(400.0),
        SHEET_DEFAULT_WIDTH,
        SHEET_WIDTH_FRACTION,
    );
    assert_eq!(tiny.thickness, px(15.0));

    // Zero or negative desired thickness resolves to zero.
    let zero = SheetGeometry::resolve(
        size(px(800.0), px(600.0)),
        SheetSide::Right,
        px(-10.0),
        SHEET_DEFAULT_WIDTH,
        SHEET_WIDTH_FRACTION,
    );
    assert_eq!(zero.thickness, px(0.0));
    assert_eq!(zero.panel.size.width, px(0.0));

    // Top clamped by viewport height fraction.
    let short = SheetGeometry::resolve(
        size(px(800.0), px(200.0)),
        SheetSide::Bottom,
        px(400.0),
        SHEET_DEFAULT_WIDTH,
        SHEET_WIDTH_FRACTION,
    );
    assert_eq!(short.thickness, px(150.0));

    // with_defaults uses legacy tokens.
    let defaults = SheetGeometry::with_defaults(size(px(800.0), px(600.0)), SheetSide::Right);
    assert_eq!(defaults.thickness, SHEET_DEFAULT_WIDTH);
    assert_eq!(defaults.side, SheetSide::Right);
}

#[test]
fn style_resolves_overlay_theme_border_button_and_motion_tokens() {
    let light = ArtisanTheme::for_mode(ThemeMode::Light);
    let dark = ArtisanTheme::for_mode(ThemeMode::Dark);
    let light_style = SheetStyle::resolve(light, MotionPolicy::Full);
    let dark_style = SheetStyle::resolve(dark, MotionPolicy::Reduced);

    assert_eq!(
        light_style.overlay,
        Hsla::black().opacity(SHEET_OVERLAY_OPACITY)
    );
    assert_eq!(
        light_style.overlay_opacity.to_bits(),
        SHEET_OVERLAY_OPACITY.to_bits()
    );
    assert_eq!(light_style.background, light.colors.popover.to_paint());
    assert_eq!(dark_style.background, dark.colors.popover.to_paint());
    assert_ne!(light_style.background, dark_style.background);
    assert_eq!(light_style.border, light.colors.border.to_paint());
    assert_eq!(dark_style.border, dark.colors.border.to_paint());
    assert_eq!(light_style.padding, light.spacing.steps(6.0));
    assert_eq!(light_style.header_gap, light.spacing.steps(1.5));
    assert_eq!(light_style.content_gap, light.spacing.steps(4.0));
    assert_eq!(light_style.close_inset, light.spacing.steps(4.0));
    assert_eq!(light_style.max_width, SHEET_DEFAULT_WIDTH);
    assert!((light_style.width_fraction - SHEET_WIDTH_FRACTION).abs() < 1e-6);
    assert_eq!(
        light_style.close_button,
        artisan_ui::button::ButtonStyle::resolve(
            light,
            artisan_ui::button::ButtonVariant::Ghost,
            artisan_ui::button::ButtonSize::IconSmall,
            MotionPolicy::Full,
        )
    );
    assert_eq!(dark_style.close_button, {
        artisan_ui::button::ButtonStyle::resolve(
            dark,
            artisan_ui::button::ButtonVariant::Ghost,
            artisan_ui::button::ButtonSize::IconSmall,
            MotionPolicy::Reduced,
        )
    });

    assert!(matches!(
        light_style.motion,
        SheetMotionPlan::Animate(animation) if animation.duration() == SHEET_OPEN_ANIMATION_DURATION
    ));
    assert_eq!(
        light_style.motion.animation().unwrap().duration(),
        SHEET_OPEN_ANIMATION_DURATION
    );
    assert!(matches!(
        light_style.motion,
        SheetMotionPlan::Animate(a) if a.duration().as_millis() == 200
    ));
    assert_eq!(dark_style.motion, SheetMotionPlan::Immediate);
    assert_eq!(dark_style.motion.animation(), None);
    assert_eq!(
        SheetMotionPlan::for_policy(MotionPolicy::Full),
        SheetMotionPlan::Animate(artisan_ui::sheet::SheetAnimation)
    );
    assert_eq!(
        SheetMotionPlan::for_policy(MotionPolicy::Reduced),
        SheetMotionPlan::Immediate
    );

    // Corner radius is not rounded for edge-docked panels, but theme still pins
    // popover surface correctly.
    let _ = RadiusTokens::value(RadiusStep::X4l);
}

#[gpui::test]
fn focus_intent_applies_entry_and_restore_once_per_controlled_edge(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, cx| FocusProbe::new(cx));

    cx.update(|window, app| {
        let probe = view.read(app);
        let (entry, restore) = (probe.entry.clone(), probe.restore.clone());
        let intent = SheetFocusIntent::new(entry.clone(), restore.clone());

        assert_eq!(
            intent.apply(false, true, window, app),
            SheetFocusTransition::Enter
        );
        assert!(entry.is_focused(window));

        assert_eq!(
            intent.apply(true, true, window, app),
            SheetFocusTransition::Unchanged
        );
        assert!(entry.is_focused(window));

        assert_eq!(
            intent.apply(true, false, window, app),
            SheetFocusTransition::Restore
        );
        assert!(restore.is_focused(window));
    });
}

#[gpui::test]
fn open_sheet_renders_overlay_panel_and_semantic_parts(cx: &mut TestAppContext) {
    let dismissals = Dismissals::new();
    let (_, cx) = cx.add_window_view({
        let dismissals = dismissals.clone();
        move |_, cx| SheetProbe::new(cx, dismissals, true, SheetSide::Right)
    });

    let root = cx
        .debug_bounds(ROOT_SELECTOR)
        .expect("open sheet root must expose a debug bound");
    let overlay = cx
        .debug_bounds(OVERLAY_SELECTOR)
        .expect("sheet overlay must expose a debug bound");
    let panel = cx
        .debug_bounds(PANEL_SELECTOR)
        .expect("sheet panel must expose a debug bound");

    assert_eq!(overlay, root);
    // Right-docked panel touches the right edge and spans full height.
    assert_eq!(panel.origin.y, root.origin.y);
    assert_eq!(panel.size.height, root.size.height);
    assert_eq!(
        panel.origin.x + panel.size.width,
        root.origin.x + root.size.width
    );
    assert!(panel.size.width <= SHEET_DEFAULT_WIDTH);
    assert!(panel.size.width > px(0.0));
    assert!(cx.debug_bounds(TITLE_SELECTOR).is_some());
    assert!(cx.debug_bounds(DESCRIPTION_SELECTOR).is_some());
    assert!(cx.debug_bounds(BODY_SELECTOR).is_some());
    assert!(cx.debug_bounds(CLOSE_SELECTOR).is_some());
}

#[gpui::test]
fn left_sheet_docks_to_left_edge(cx: &mut TestAppContext) {
    let (_, cx) =
        cx.add_window_view(|_, cx| SheetProbe::new(cx, Dismissals::new(), true, SheetSide::Left));
    let root = cx.debug_bounds(ROOT_SELECTOR).expect("root");
    let panel = cx.debug_bounds(PANEL_SELECTOR).expect("left panel");
    assert_eq!(panel.origin.x, root.origin.x);
    assert_eq!(panel.size.height, root.size.height);
}

#[gpui::test]
fn overlay_and_escape_and_close_request_dismissal_without_mutating_controlled_state(
    cx: &mut TestAppContext,
) {
    let dismissals = Dismissals::new();
    let (view, cx) = cx.add_window_view({
        let dismissals = dismissals.clone();
        move |_, cx| SheetProbe::new(cx, dismissals, true, SheetSide::Right)
    });

    let root = cx.debug_bounds(ROOT_SELECTOR).expect("root");
    let panel = cx.debug_bounds(PANEL_SELECTOR).expect("panel");

    // Click inside panel must not dismiss.
    cx.simulate_click(panel.center(), Modifiers::none());
    assert_eq!(dismissals.count.get(), 0);

    // Click on overlay but outside panel must dismiss with Overlay reason.
    let outside = point(root.origin.x + px(4.0), root.origin.y + px(4.0));
    // Ensure outside is not inside panel for right-docked panel.
    assert!(outside.x < panel.origin.x);
    cx.simulate_click(outside, Modifiers::none());
    assert_eq!(dismissals.count.get(), 1);
    assert_eq!(dismissals.last.get(), Some(SheetDismissReason::Overlay));
    cx.update(|_, app| assert!(view.read(app).open));

    // Escape dismissal
    cx.update(|window, app| {
        let focus = view.read(app).focus.clone();
        window.focus(&focus, app);
    });
    cx.simulate_keystrokes("escape");
    assert_eq!(dismissals.count.get(), 2);
    assert_eq!(dismissals.last.get(), Some(SheetDismissReason::Escape));
    cx.update(|_, app| assert!(view.read(app).open));

    // Close button dismissal
    let close = cx
        .debug_bounds(CLOSE_SELECTOR)
        .expect("close control must remain mounted until caller closes sheet");
    cx.simulate_click(close.center(), Modifiers::none());
    assert_eq!(dismissals.count.get(), 3);
    assert_eq!(dismissals.last.get(), Some(SheetDismissReason::CloseButton));

    cx.update(|_, app| {
        view.update(app, |probe, cx| {
            probe.open = false;
            cx.notify();
        });
    });
    cx.run_until_parked();

    cx.update(|_, app| assert!(!view.read(app).open));
    let closed = cx
        .debug_bounds(CLOSED_SELECTOR)
        .expect("controlled close must paint the zero-sized closed sentinel");
    assert_eq!(closed.size, size(px(0.0), px(0.0)));
    // Pinned GPUI retains painted debug bounds for the window lifetime, so a
    // dynamically unmounted panel still resolves here; unmount-absence is
    // proven by the fresh-window closed test below instead.
}

#[gpui::test]
fn closed_sheet_renders_sentinel_without_panel_or_overlay(cx: &mut TestAppContext) {
    let (_, cx) =
        cx.add_window_view(|_, cx| SheetProbe::new(cx, Dismissals::new(), false, SheetSide::Right));
    assert!(cx.debug_bounds(CLOSED_SELECTOR).is_some());
    assert!(cx.debug_bounds(PANEL_SELECTOR).is_none());
    assert!(cx.debug_bounds(OVERLAY_SELECTOR).is_none());
    assert!(cx.debug_bounds(CLOSE_SELECTOR).is_none());
    let closed = cx.debug_bounds(CLOSED_SELECTOR).unwrap();
    assert_eq!(closed.size, size(px(0.0), px(0.0)));
}

#[gpui::test]
fn semantics_retain_title_description_side_role_and_close_name(cx: &mut TestAppContext) {
    let (view, cx) =
        cx.add_window_view(|_, cx| SheetProbe::new(cx, Dismissals::new(), true, SheetSide::Left));

    let semantics = cx.update(|_, app| {
        Sheet::new(
            "semantics-sheet",
            view.read(app).focus.clone(),
            ArtisanTheme::for_mode(ThemeMode::Light),
            true,
            SheetSide::Bottom,
            "Sheet title",
            div().child("body"),
        )
        .motion_policy(MotionPolicy::Reduced)
        .description("Sheet description")
        .semantics()
    });

    assert_eq!(semantics.role, SHEET_ROLE);
    assert_eq!(semantics.title.as_ref(), "Sheet title");
    assert_eq!(
        semantics
            .description
            .as_ref()
            .map(gpui::SharedString::as_str),
        Some("Sheet description")
    );
    assert_eq!(semantics.close_label.as_ref(), SHEET_CLOSE_LABEL);
    assert_eq!(semantics.side, SheetSide::Bottom);
}

#[test]
fn reduced_motion_resolves_to_immediate_without_animation() {
    let animation = SheetMotionPlan::for_policy(MotionPolicy::Reduced);
    assert_eq!(animation, SheetMotionPlan::Immediate);
    assert_eq!(animation.animation(), None);

    let full = SheetMotionPlan::for_policy(MotionPolicy::Full);
    assert!(matches!(full, SheetMotionPlan::Animate(_)));
    assert_eq!(
        full.animation().unwrap().duration(),
        SHEET_OPEN_ANIMATION_DURATION
    );
    assert_eq!(
        full.animation().unwrap().gpui_clock().duration,
        SHEET_OPEN_ANIMATION_DURATION
    );
}
