//! Behavioral coverage for the native alert confirmation dialog primitive.

use std::cell::Cell;
use std::rc::Rc;

use artisan_ui::alert_dialog::{
    ALERT_BACKDROP_OPACITY, ALERT_DIALOG_ROLE, ALERT_MAX_WIDTH, ALERT_VIEWPORT_MARGIN, AlertDialog,
    AlertDialogActionRole, AlertDialogDismissReason, AlertDialogGeometry, AlertDialogIntent,
    AlertDialogState, AlertDialogStyle, DEFAULT_ACTION_LABEL, DEFAULT_CANCEL_LABEL,
};
use artisan_ui::dialog::{DialogFocusTransition, DialogState, focus_transition};
use artisan_ui::motion::MotionPolicy;
use artisan_ui::theme::{ArtisanTheme, RadiusStep, RadiusTokens, ThemeMode};
use gpui::{
    Bounds, ColorExt as _, Context, FocusHandle, Hsla, InteractiveElement, IntoElement,
    ParentElement as _,
    Render, Styled, TestAppContext, Window, div, point, px, size,
};

const ROOT_SELECTOR: &str = "native-alert-dialog-under-test";
const CLOSED_SELECTOR: &str = "native-alert-dialog-under-test-closed";
const OVERLAY_SELECTOR: &str = "native-alert-dialog-under-test-overlay";
const PANEL_SELECTOR: &str = "native-alert-dialog-under-test-panel";
const TITLE_SELECTOR: &str = "native-alert-dialog-under-test-title";
const DESCRIPTION_SELECTOR: &str = "native-alert-dialog-under-test-description";
const ACTION_SELECTOR: &str = "native-alert-dialog-under-test-action";
const CANCEL_SELECTOR: &str = "native-alert-dialog-under-test-cancel";
const FOOTER_SELECTOR: &str = "native-alert-dialog-under-test-footer";
const BODY_SELECTOR: &str = "native-alert-dialog-body";

#[derive(Clone)]
struct Dismissals {
    count: Rc<Cell<u32>>,
    last: Rc<Cell<Option<AlertDialogDismissReason>>>,
}

impl Dismissals {
    fn new() -> Self {
        Self {
            count: Rc::new(Cell::new(0)),
            last: Rc::new(Cell::new(None)),
        }
    }

    fn record(&self, reason: AlertDialogDismissReason) {
        self.count.set(self.count.get() + 1);
        self.last.set(Some(reason));
    }
}

struct AlertDialogProbe {
    focus: FocusHandle,
    dismissals: Dismissals,
    open: bool,
    intent: AlertDialogIntent,
}

impl AlertDialogProbe {
    fn new(
        cx: &mut Context<Self>,
        dismissals: Dismissals,
        open: bool,
        intent: AlertDialogIntent,
    ) -> Self {
        Self {
            focus: cx.focus_handle(),
            dismissals,
            open,
            intent,
        }
    }
}

impl Render for AlertDialogProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let dismissals = self.dismissals.clone();
        let dialog = AlertDialog::new(
            "native-alert-dialog-id",
            self.focus.clone(),
            ArtisanTheme::for_mode(ThemeMode::Dark),
            MotionPolicy::Reduced,
            self.open,
            "Delete project?",
            div()
                .w(px(240.0))
                .h(px(48.0))
                .debug_selector(|| BODY_SELECTOR.to_string())
                .child("This action cannot be undone."),
        )
        .description("This will permanently delete the project and its threads")
        .action_label("Delete")
        .cancel_label("Cancel")
        .intent(self.intent)
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
fn state_is_controlled_and_dismissal_is_deterministic() {
    let open = AlertDialogState::new(true);
    let closed = AlertDialogState::new(false);

    assert!(open.open());
    assert!(open.is_open());
    assert!(!closed.open());
    assert!(!closed.is_open());

    for reason in [
        AlertDialogDismissReason::Action,
        AlertDialogDismissReason::Cancel,
        AlertDialogDismissReason::Escape,
    ] {
        assert_eq!(open.requested_dismissal(reason), Some(false));
        assert_eq!(closed.requested_dismissal(reason), None);
    }

    assert_eq!(open.requested_action(), Some(false));
    assert_eq!(closed.requested_action(), None);
    assert_eq!(open.requested_cancel(), Some(false));
    assert_eq!(closed.requested_cancel(), None);

    // Dismissals map to controlled close, not a toggle.
    let dialog_open = DialogState::new(true);
    assert_eq!(
        dialog_open.requested_dismissal(artisan_ui::dialog::DialogDismissReason::Escape),
        Some(false)
    );
}

#[test]
fn action_roles_and_intent_policy_are_stable() {
    assert_eq!(AlertDialogActionRole::Action.as_str(), "action");
    assert_eq!(AlertDialogActionRole::Cancel.as_str(), "cancel");
    assert_ne!(AlertDialogActionRole::Action, AlertDialogActionRole::Cancel);

    let default = AlertDialogIntent::Default;
    let destructive = AlertDialogIntent::Destructive;
    assert!(default.is_default());
    assert!(!default.is_destructive());
    assert!(destructive.is_destructive());
    assert!(!destructive.is_default());
    assert_ne!(default, destructive);

    assert_eq!(DEFAULT_ACTION_LABEL, "Continue");
    assert_eq!(DEFAULT_CANCEL_LABEL, "Cancel");
    assert_eq!(ALERT_DIALOG_ROLE, "alertdialog");
    assert_eq!(ALERT_BACKDROP_OPACITY.to_bits(), 0.8_f32.to_bits());
    assert_eq!(ALERT_MAX_WIDTH, px(448.0));
    assert_eq!(ALERT_VIEWPORT_MARGIN, px(16.0));
}

#[test]
fn geometry_reuses_dialog_centering_policy() {
    let geometry = AlertDialogGeometry::centered(
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
}

#[test]
fn style_resolves_overlay_theme_radius_and_intent_tokens() {
    let light = ArtisanTheme::for_mode(ThemeMode::Light);
    let dark = ArtisanTheme::for_mode(ThemeMode::Dark);
    let default_style =
        AlertDialogStyle::resolve(light, MotionPolicy::Full, AlertDialogIntent::Default);
    let destructive_style =
        AlertDialogStyle::resolve(light, MotionPolicy::Full, AlertDialogIntent::Destructive);
    let reduced =
        AlertDialogStyle::resolve(dark, MotionPolicy::Reduced, AlertDialogIntent::Default);

    assert_eq!(
        default_style.overlay,
        Hsla::black().opacity(ALERT_BACKDROP_OPACITY)
    );
    assert_eq!(
        default_style.overlay_opacity.to_bits(),
        ALERT_BACKDROP_OPACITY.to_bits()
    );
    assert_eq!(default_style.background, light.colors.popover.to_paint());
    assert_eq!(reduced.background, dark.colors.popover.to_paint());
    assert_ne!(default_style.background, reduced.background);
    assert_eq!(
        default_style.corner_radius,
        RadiusTokens::value(RadiusStep::X4l)
    );
    assert_eq!(default_style.padding, light.spacing.steps(6.0));
    assert_eq!(default_style.max_width, ALERT_MAX_WIDTH);
    assert_eq!(default_style.viewport_margin, ALERT_VIEWPORT_MARGIN);
    assert_eq!(default_style.footer_gap, light.spacing.steps(2.0));
    assert_eq!(
        default_style.cancel_button,
        artisan_ui::button::ButtonStyle::resolve(
            light,
            artisan_ui::button::ButtonVariant::Outline,
            artisan_ui::button::ButtonSize::Small,
            MotionPolicy::Full,
        )
    );
    assert_eq!(
        default_style.action_button,
        artisan_ui::button::ButtonStyle::resolve(
            light,
            artisan_ui::button::ButtonVariant::Default,
            artisan_ui::button::ButtonSize::Small,
            MotionPolicy::Full,
        )
    );
    assert_ne!(
        destructive_style.action_button.background,
        default_style.action_button.background
    );
    assert_eq!(
        destructive_style.action_button.background,
        light.colors.destructive.to_paint()
    );
    assert_ne!(
        destructive_style.action_button.background,
        destructive_style.cancel_button.background
    );
    assert!(matches!(
        default_style.motion,
        artisan_ui::dialog::DialogMotionPlan::Animate(a) if a.duration().as_millis() == 100
    ));
    assert_eq!(
        reduced.motion,
        artisan_ui::dialog::DialogMotionPlan::Immediate
    );
}

#[test]
fn focus_transition_alias_matches_dialog_policy() {
    assert_eq!(
        AlertDialog::focus_transition(false, false),
        DialogFocusTransition::Unchanged
    );
    assert_eq!(
        AlertDialog::focus_transition(false, true),
        DialogFocusTransition::Enter
    );
    assert_eq!(
        AlertDialog::focus_transition(true, true),
        DialogFocusTransition::Unchanged
    );
    assert_eq!(
        AlertDialog::focus_transition(true, false),
        DialogFocusTransition::Restore
    );
    assert_eq!(
        focus_transition(false, true),
        AlertDialog::focus_transition(false, true)
    );
}

#[gpui::test]
fn focus_intent_applies_entry_and_restore_once_per_controlled_edge(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, cx| FocusProbe::new(cx));

    cx.update(|window, app| {
        let probe = view.read(app);
        let intent =
            artisan_ui::dialog::DialogFocusIntent::new(probe.entry.clone(), probe.restore.clone());

        assert_eq!(
            intent.apply(false, true, window, app),
            DialogFocusTransition::Enter
        );
        assert!(probe.entry.is_focused(window));

        assert_eq!(
            intent.apply(true, true, window, app),
            DialogFocusTransition::Unchanged
        );
        assert!(probe.entry.is_focused(window));

        assert_eq!(
            intent.apply(true, false, window, app),
            DialogFocusTransition::Restore
        );
        assert!(probe.restore.is_focused(window));
    });
}

#[gpui::test]
fn open_alert_dialog_renders_overlay_panel_and_composition(cx: &mut TestAppContext) {
    let dismissals = Dismissals::new();
    let (_, cx) = cx.add_window_view({
        let dismissals = dismissals.clone();
        move |_, cx| AlertDialogProbe::new(cx, dismissals, true, AlertDialogIntent::Default)
    });

    let root = cx
        .debug_bounds(ROOT_SELECTOR)
        .expect("open alert dialog root must expose a debug bound");
    let overlay = cx
        .debug_bounds(OVERLAY_SELECTOR)
        .expect("alert dialog overlay must expose a debug bound");
    let panel = cx
        .debug_bounds(PANEL_SELECTOR)
        .expect("alert dialog panel must expose a debug bound");

    assert_eq!(overlay, root);
    assert_eq!(root.center(), panel.center());
    assert!(panel.size.width <= ALERT_MAX_WIDTH);
    assert!(panel.origin.x >= root.origin.x + ALERT_VIEWPORT_MARGIN);
    assert!(panel.origin.y >= root.origin.y + ALERT_VIEWPORT_MARGIN);
    assert!(cx.debug_bounds(TITLE_SELECTOR).is_some());
    assert!(cx.debug_bounds(DESCRIPTION_SELECTOR).is_some());
    assert!(cx.debug_bounds(BODY_SELECTOR).is_some());
    assert!(cx.debug_bounds(FOOTER_SELECTOR).is_some());
    assert!(cx.debug_bounds(ACTION_SELECTOR).is_some());
    assert!(cx.debug_bounds(CANCEL_SELECTOR).is_some());
}

#[gpui::test]
fn action_cancel_and_escape_request_dismissal_without_mutating_controlled_state(
    cx: &mut TestAppContext,
) {
    let dismissals = Dismissals::new();
    let (view, cx) = cx.add_window_view({
        let dismissals = dismissals.clone();
        move |_, cx| AlertDialogProbe::new(cx, dismissals, true, AlertDialogIntent::Destructive)
    });

    let action = cx
        .debug_bounds(ACTION_SELECTOR)
        .expect("action control must be mounted");
    cx.simulate_click(action.center(), gpui::Modifiers::none());
    assert_eq!(dismissals.count.get(), 1);
    assert_eq!(
        dismissals.last.get(),
        Some(AlertDialogDismissReason::Action)
    );
    cx.update(|_, app| assert!(view.read(app).open));

    let cancel = cx
        .debug_bounds(CANCEL_SELECTOR)
        .expect("cancel control must be mounted");
    cx.simulate_click(cancel.center(), gpui::Modifiers::none());
    assert_eq!(dismissals.count.get(), 2);
    assert_eq!(
        dismissals.last.get(),
        Some(AlertDialogDismissReason::Cancel)
    );
    cx.update(|_, app| assert!(view.read(app).open));

    cx.update(|window, app| {
        let focus = view.read(app).focus.clone();
        window.focus(&focus, app);
    });
    cx.simulate_keystrokes("escape");
    assert_eq!(dismissals.count.get(), 3);
    assert_eq!(
        dismissals.last.get(),
        Some(AlertDialogDismissReason::Escape)
    );
    cx.update(|_, app| assert!(view.read(app).open));

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
fn semantics_retain_role_title_description_labels_and_intent(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, cx| {
        AlertDialogProbe::new(cx, Dismissals::new(), true, AlertDialogIntent::Default)
    });

    let semantics = cx.update(|_, app| {
        AlertDialog::new(
            "semantics-alert-dialog",
            view.read(app).focus.clone(),
            ArtisanTheme::for_mode(ThemeMode::Light),
            MotionPolicy::Reduced,
            true,
            "Delete project?",
            div().child("body"),
        )
        .description("This will permanently delete the project")
        .action_label("Delete")
        .cancel_label("Keep")
        .intent(AlertDialogIntent::Destructive)
        .semantics()
    });

    assert_eq!(semantics.role, ALERT_DIALOG_ROLE);
    assert_eq!(semantics.title.as_ref(), "Delete project?");
    assert_eq!(
        semantics
            .description
            .as_ref()
            .map(gpui::SharedString::as_str),
        Some("This will permanently delete the project")
    );
    assert_eq!(semantics.action_label.as_ref(), "Delete");
    assert_eq!(semantics.cancel_label.as_ref(), "Keep");
    assert_eq!(semantics.intent, AlertDialogIntent::Destructive);
    let probe_focus = cx.update(|_, app| view.read(app).focus.clone());
    assert_eq!(
        AlertDialog::new(
            "role-check",
            probe_focus,
            ArtisanTheme::for_mode(ThemeMode::Light),
            MotionPolicy::Reduced,
            true,
            "T",
            div(),
        )
        .role(),
        "alertdialog"
    );
}

#[gpui::test]
fn identity_is_stable_for_focus_and_debug_selectors(cx: &mut TestAppContext) {
    let dismissals = Dismissals::new();
    let (view, cx) = cx.add_window_view({
        let dismissals = dismissals.clone();
        move |_, cx| AlertDialogProbe::new(cx, dismissals, true, AlertDialogIntent::Default)
    });

    let focus_handle = cx.update(|_, app| view.read(app).focus.clone());
    let semantics_focus = cx.update(|_, _app| {
        AlertDialog::new(
            "identity-check",
            focus_handle.clone(),
            ArtisanTheme::for_mode(ThemeMode::Dark),
            MotionPolicy::Reduced,
            true,
            "Title",
            div().child("content"),
        )
        .focus_handle()
        .clone()
    });
    // The focus handle identity is retained by the component.
    assert_eq!(format!("{semantics_focus:?}"), format!("{focus_handle:?}"));

    // All debug selectors are present under the same prefix and remain stable
    // across rerenders.
    for selector in [
        ROOT_SELECTOR,
        OVERLAY_SELECTOR,
        PANEL_SELECTOR,
        TITLE_SELECTOR,
        DESCRIPTION_SELECTOR,
        FOOTER_SELECTOR,
        ACTION_SELECTOR,
        CANCEL_SELECTOR,
    ] {
        assert!(
            cx.debug_bounds(selector).is_some(),
            "selector {selector} must be present"
        );
    }

    // Close and reopen preserves selector prefix contract.
    cx.update(|_, app| {
        view.update(app, |probe, cx| {
            probe.open = false;
            cx.notify();
        });
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds(CLOSED_SELECTOR).is_some());
    assert!(cx.debug_bounds(OVERLAY_SELECTOR).is_none());

    cx.update(|_, app| {
        view.update(app, |probe, cx| {
            probe.open = true;
            cx.notify();
        });
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds(OVERLAY_SELECTOR).is_some());
    assert!(cx.debug_bounds(PANEL_SELECTOR).is_some());
}
