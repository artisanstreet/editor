//! Behavioral coverage for the native controlled GPUI dialog primitive.

use std::cell::Cell;
use std::rc::Rc;

use artisan_ui::dialog::{
    BACKDROP_OPACITY, DEFAULT_MAX_WIDTH, DEFAULT_VIEWPORT_MARGIN, DIALOG_ROLE, Dialog,
    DialogDismissReason, DialogFocusIntent, DialogFocusTransition, DialogGeometry,
    DialogMotionPlan, DialogState, DialogStyle, focus_transition,
};
use artisan_ui::motion::MotionPolicy;
use artisan_ui::theme::{ArtisanTheme, RadiusStep, RadiusTokens, ThemeMode};
use gpui::{
    Bounds, Context, FocusHandle, Hsla, InteractiveElement, IntoElement, Modifiers, ParentElement,
    Render, Styled, TestAppContext, Window, div, point, px, size,
};

const ROOT_SELECTOR: &str = "native-dialog-under-test";
const CLOSED_SELECTOR: &str = "native-dialog-under-test-closed";
const BACKDROP_SELECTOR: &str = "native-dialog-under-test-backdrop";
const CONTENT_SELECTOR: &str = "native-dialog-under-test-content";
const TITLE_SELECTOR: &str = "native-dialog-under-test-title";
const DESCRIPTION_SELECTOR: &str = "native-dialog-under-test-description";
const CLOSE_SELECTOR: &str = "native-dialog-under-test-close";
const BODY_SELECTOR: &str = "native-dialog-body";

#[derive(Clone)]
struct Dismissals {
    count: Rc<Cell<u32>>,
    last: Rc<Cell<Option<DialogDismissReason>>>,
}

impl Dismissals {
    fn new() -> Self {
        Self {
            count: Rc::new(Cell::new(0)),
            last: Rc::new(Cell::new(None)),
        }
    }

    fn record(&self, reason: DialogDismissReason) {
        self.count.set(self.count.get() + 1);
        self.last.set(Some(reason));
    }
}

struct DialogProbe {
    focus: FocusHandle,
    dismissals: Dismissals,
    open: bool,
}

impl DialogProbe {
    fn new(cx: &mut Context<Self>, dismissals: Dismissals, open: bool) -> Self {
        Self {
            focus: cx.focus_handle(),
            dismissals,
            open,
        }
    }
}

impl Render for DialogProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let dismissals = self.dismissals.clone();
        let dialog = Dialog::new(
            "native-dialog-id",
            self.focus.clone(),
            ArtisanTheme::for_mode(ThemeMode::Dark),
            MotionPolicy::Reduced,
            self.open,
            "Terminal",
            div()
                .w(px(240.0))
                .h(px(96.0))
                .debug_selector(|| BODY_SELECTOR.to_string())
                .child("Choose a terminal"),
        )
        .description("Select where this session should open")
        .debug_selector(ROOT_SELECTOR)
        .on_dismiss(move |reason, _, _| dismissals.record(reason));

        div().relative().size_full().child(dialog)
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
    let open = DialogState::new(true);
    let closed = DialogState::new(false);

    assert!(open.open());
    assert!(open.is_open());
    assert!(!closed.open());
    assert!(!closed.is_open());

    for reason in [
        DialogDismissReason::Escape,
        DialogDismissReason::Backdrop,
        DialogDismissReason::CloseButton,
    ] {
        assert_eq!(open.requested_dismissal(reason), Some(false));
        assert_eq!(closed.requested_dismissal(reason), None);
    }

    assert_eq!(open.requested_close(), Some(false));
    assert_eq!(closed.requested_close(), None);

    assert_eq!(
        focus_transition(false, false),
        DialogFocusTransition::Unchanged
    );
    assert_eq!(focus_transition(false, true), DialogFocusTransition::Enter);
    assert_eq!(
        focus_transition(true, true),
        DialogFocusTransition::Unchanged
    );
    assert_eq!(
        focus_transition(true, false),
        DialogFocusTransition::Restore
    );
}

#[test]
fn centered_geometry_caps_content_and_handles_small_viewports() {
    let geometry = DialogGeometry::centered(
        size(px(800.0), px(600.0)),
        size(px(600.0), px(900.0)),
        px(448.0),
        Some(px(500.0)),
        px(16.0),
    );

    assert_eq!(geometry.margin, px(16.0));
    assert_eq!(
        geometry.content,
        Bounds {
            origin: point(px(176.0), px(50.0)),
            size: size(px(448.0), px(500.0)),
        }
    );
    assert!(geometry.contains(point(px(400.0), px(300.0))));
    assert!(!geometry.contains(point(px(175.0), px(300.0))));

    let tiny = DialogGeometry::centered(
        size(px(20.0), px(10.0)),
        size(px(100.0), px(100.0)),
        px(448.0),
        None,
        px(16.0),
    );
    assert_eq!(tiny.content.size, size(px(0.0), px(0.0)));
    assert_eq!(tiny.content.origin, point(px(10.0), px(5.0)));
}

#[test]
fn style_resolves_overlay_theme_radius_button_and_motion_tokens() {
    let light = ArtisanTheme::for_mode(ThemeMode::Light);
    let dark = ArtisanTheme::for_mode(ThemeMode::Dark);
    let light_style = DialogStyle::resolve(light, MotionPolicy::Full);
    let dark_style = DialogStyle::resolve(dark, MotionPolicy::Reduced);

    assert_eq!(light_style.overlay, Hsla::black().opacity(BACKDROP_OPACITY));
    assert_eq!(
        light_style.overlay_opacity.to_bits(),
        BACKDROP_OPACITY.to_bits()
    );
    assert_eq!(light_style.background, light.colors.popover.to_paint());
    assert_eq!(dark_style.background, dark.colors.popover.to_paint());
    assert_ne!(light_style.background, dark_style.background);
    assert_eq!(
        light_style.corner_radius,
        RadiusTokens::value(RadiusStep::X4l)
    );
    assert_eq!(light_style.padding, light.spacing.steps(6.0));
    assert_eq!(light_style.max_width, DEFAULT_MAX_WIDTH);
    assert_eq!(light_style.viewport_margin, DEFAULT_VIEWPORT_MARGIN);
    assert_eq!(
        light_style.close_button,
        artisan_ui::button::ButtonStyle::resolve(
            light,
            artisan_ui::button::ButtonVariant::Ghost,
            artisan_ui::button::ButtonSize::IconSmall,
            MotionPolicy::Full,
        )
    );
    assert!(matches!(
        light_style.motion,
        DialogMotionPlan::Animate(animation) if animation.duration().as_millis() == 100
    ));
    assert_eq!(dark_style.motion, DialogMotionPlan::Immediate);
}

#[gpui::test]
fn focus_intent_applies_entry_and_restore_once_per_controlled_edge(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, cx| FocusProbe::new(cx));

    cx.update(|window, app| {
        let probe = view.read(app);
        let intent = DialogFocusIntent::new(probe.entry.clone(), probe.restore.clone());

        assert_eq!(
            intent.apply(false, true, window),
            DialogFocusTransition::Enter
        );
        assert!(probe.entry.is_focused(window));

        assert_eq!(
            intent.apply(true, true, window),
            DialogFocusTransition::Unchanged
        );
        assert!(probe.entry.is_focused(window));

        assert_eq!(
            intent.apply(true, false, window),
            DialogFocusTransition::Restore
        );
        assert!(probe.restore.is_focused(window));
    });
}

#[gpui::test]
fn open_dialog_renders_centered_bounded_semantic_parts(cx: &mut TestAppContext) {
    let dismissals = Dismissals::new();
    let (_, cx) = cx.add_window_view({
        let dismissals = dismissals.clone();
        move |_, cx| DialogProbe::new(cx, dismissals, true)
    });

    let root = cx
        .debug_bounds(ROOT_SELECTOR)
        .expect("open dialog root must expose a debug bound");
    let backdrop = cx
        .debug_bounds(BACKDROP_SELECTOR)
        .expect("dialog backdrop must expose a debug bound");
    let content = cx
        .debug_bounds(CONTENT_SELECTOR)
        .expect("dialog content must expose a debug bound");

    assert_eq!(backdrop, root);
    assert_eq!(root.center(), content.center());
    assert!(content.size.width <= DEFAULT_MAX_WIDTH);
    assert!(content.origin.x >= root.origin.x + DEFAULT_VIEWPORT_MARGIN);
    assert!(content.origin.y >= root.origin.y + DEFAULT_VIEWPORT_MARGIN);
    assert!(cx.debug_bounds(TITLE_SELECTOR).is_some());
    assert!(cx.debug_bounds(DESCRIPTION_SELECTOR).is_some());
    assert!(cx.debug_bounds(BODY_SELECTOR).is_some());
    assert!(cx.debug_bounds(CLOSE_SELECTOR).is_some());
}

#[gpui::test]
fn backdrop_and_escape_request_dismissal_without_mutating_controlled_open_state(
    cx: &mut TestAppContext,
) {
    let dismissals = Dismissals::new();
    let (view, cx) = cx.add_window_view({
        let dismissals = dismissals.clone();
        move |_, cx| DialogProbe::new(cx, dismissals, true)
    });

    let root = cx
        .debug_bounds(ROOT_SELECTOR)
        .expect("open dialog root must expose a debug bound");
    let content = cx
        .debug_bounds(CONTENT_SELECTOR)
        .expect("open dialog content must expose a debug bound");
    let outside = point(root.origin.x + px(4.0), root.origin.y + px(4.0));

    let geometry = DialogGeometry {
        viewport: root.size,
        content,
        margin: px(0.0),
    };
    assert!(!geometry.contains(outside));

    cx.simulate_click(outside, Modifiers::none());
    assert_eq!(dismissals.count.get(), 1);
    assert_eq!(dismissals.last.get(), Some(DialogDismissReason::Backdrop));
    cx.update(|_, app| assert!(view.read(app).open));

    cx.update(|window, app| {
        let focus = view.read(app).focus.clone();
        window.focus(&focus);
    });
    cx.simulate_keystrokes("escape");
    assert_eq!(dismissals.count.get(), 2);
    assert_eq!(dismissals.last.get(), Some(DialogDismissReason::Escape));
    cx.update(|_, app| assert!(view.read(app).open));

    let close = cx
        .debug_bounds(CLOSE_SELECTOR)
        .expect("close control must remain mounted until caller closes dialog");
    cx.simulate_click(close.center(), Modifiers::none());
    assert_eq!(dismissals.count.get(), 3);
    assert_eq!(
        dismissals.last.get(),
        Some(DialogDismissReason::CloseButton)
    );

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
}

#[gpui::test]
fn semantics_retain_title_description_role_and_close_name(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, cx| DialogProbe::new(cx, Dismissals::new(), true));

    let semantics = cx.update(|_, app| {
        Dialog::new(
            "semantics-dialog",
            view.read(app).focus.clone(),
            ArtisanTheme::for_mode(ThemeMode::Light),
            MotionPolicy::Reduced,
            true,
            "Terminal",
            div().child("body"),
        )
        .description("Select a terminal")
        .semantics()
    });

    assert_eq!(semantics.role, DIALOG_ROLE);
    assert_eq!(semantics.title.as_ref(), "Terminal");
    assert_eq!(
        semantics
            .description
            .as_ref()
            .map(gpui::SharedString::as_str),
        Some("Select a terminal")
    );
    assert_eq!(semantics.close_label.as_ref(), "Close");
}
