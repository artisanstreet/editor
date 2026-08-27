use std::cell::Cell;
use std::rc::Rc;

use artisan_ui::collapsible::{Collapsible, CollapsibleState};
use gpui::{
    Context, FocusHandle, InteractiveElement, IntoElement, KeyUpEvent, Keystroke, Modifiers,
    MouseButton, ParentElement, Render, Styled, TestAppContext, Window, div, px,
};

const ROOT_SELECTOR: &str = "native-collapsible-under-test";
const TRIGGER_SELECTOR: &str = "native-collapsible-under-test-trigger";
const CONTENT_SELECTOR: &str = "native-collapsible-under-test-content";
const TRIGGER_CHILD_SELECTOR: &str = "native-collapsible-trigger-child";
const CONTENT_CHILD_SELECTOR: &str = "native-collapsible-content-child";
const COLLAPSIBLE_ID: &str = "native-collapsible-id";
const AFTER_UNMOUNT_ROOT_SELECTOR: &str = "native-collapsible-after-unmount";
const AFTER_UNMOUNT_TRIGGER_SELECTOR: &str = "native-collapsible-after-unmount-trigger";
const AFTER_UNMOUNT_CONTENT_SELECTOR: &str = "native-collapsible-after-unmount-content";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MountMode {
    Unmounted,
    ForceMountedHidden,
    HiddenUntilFound,
}

#[derive(Clone)]
struct ChangeState {
    activations: Rc<Cell<u32>>,
    next: Rc<Cell<Option<bool>>>,
}

impl ChangeState {
    fn new() -> Self {
        Self {
            activations: Rc::new(Cell::new(0)),
            next: Rc::new(Cell::new(None)),
        }
    }

    fn record(&self, next: bool) {
        self.activations.set(self.activations.get() + 1);
        self.next.set(Some(next));
    }
}

struct CollapsibleProbe {
    focus: FocusHandle,
    changes: ChangeState,
    open: bool,
    disabled: bool,
    refined: bool,
    root_selector: &'static str,
    mount_mode: MountMode,
}

impl CollapsibleProbe {
    fn new(
        cx: &mut Context<Self>,
        changes: ChangeState,
        open: bool,
        disabled: bool,
        refined: bool,
        mount_mode: MountMode,
    ) -> Self {
        Self {
            focus: cx.focus_handle(),
            changes,
            open,
            disabled,
            refined,
            root_selector: ROOT_SELECTOR,
            mount_mode,
        }
    }
}

impl Render for CollapsibleProbe {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let changes = self.changes.clone();
        let (force_mount, hidden_until_found) = match self.mount_mode {
            MountMode::Unmounted => (false, false),
            MountMode::ForceMountedHidden => (true, false),
            MountMode::HiddenUntilFound => (true, true),
        };
        let trigger = div()
            .w(px(180.0))
            .h(px(24.0))
            .debug_selector(|| TRIGGER_CHILD_SELECTOR.to_string())
            .child("Route group");
        let content = div()
            .w(px(180.0))
            .h(px(48.0))
            .debug_selector(|| CONTENT_CHILD_SELECTOR.to_string())
            .child("Model");

        let mut collapsible = Collapsible::new(
            COLLAPSIBLE_ID,
            self.focus.clone(),
            self.open,
            trigger,
            content,
        )
        .disabled(self.disabled)
        .force_mount(force_mount)
        .hidden_until_found(hidden_until_found)
        .debug_selector(self.root_selector)
        .on_change(move |next, _, _, _| changes.record(next));

        if self.refined {
            collapsible = collapsible.w(px(220.0)).h(px(100.0));
        }

        div().size_full().child(collapsible)
    }
}

#[test]
fn state_reports_open_closed_and_disabled_toggle_outcomes() {
    let open = CollapsibleState::new(true, false);
    assert!(open.open());
    assert!(open.is_open());
    assert!(!open.disabled());
    assert_eq!(open.requested_toggle(), Some(false));

    let closed = CollapsibleState::new(false, false);
    assert!(!closed.open());
    assert_eq!(closed.requested_toggle(), Some(true));

    let disabled = CollapsibleState::new(false, true);
    assert!(disabled.is_disabled());
    assert_eq!(disabled.requested_toggle(), None);
}

#[gpui::test]
fn primary_pointer_is_controlled_and_non_primary_click_is_ignored(cx: &mut TestAppContext) {
    let changes = ChangeState::new();
    let changes_for_view = changes.clone();
    let (view, cx) = cx.add_window_view(move |_, cx| {
        CollapsibleProbe::new(
            cx,
            changes_for_view,
            false,
            false,
            false,
            MountMode::Unmounted,
        )
    });
    let trigger = cx
        .debug_bounds(TRIGGER_SELECTOR)
        .expect("trigger must expose a debug bound");

    cx.simulate_click(trigger.center(), Modifiers::none());
    cx.update(|window, app| {
        let probe = view.read(app);
        assert!(probe.focus.is_focused(window));
        assert_eq!(changes.activations.get(), 1);
        assert_eq!(changes.next.get(), Some(true));
        assert!(!probe.open, "the component must wait for caller rerender");
    });

    cx.simulate_mouse_down(trigger.center(), MouseButton::Right, Modifiers::none());
    cx.simulate_mouse_up(trigger.center(), MouseButton::Right, Modifiers::none());
    assert_eq!(changes.activations.get(), 1);

    let next = changes.next.get().expect("primary click must request open");
    cx.update(|_, app| {
        view.update(app, |probe, cx| {
            probe.open = next;
            cx.notify();
        });
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds(TRIGGER_SELECTOR).is_some());
    assert!(cx.debug_bounds(CONTENT_SELECTOR).is_some());
}

#[gpui::test]
fn enter_and_space_activate_once_while_unrelated_keys_are_suppressed(cx: &mut TestAppContext) {
    let changes = ChangeState::new();
    let changes_for_view = changes.clone();
    let (view, cx) = cx.add_window_view(move |window, cx| {
        let probe = CollapsibleProbe::new(
            cx,
            changes_for_view,
            false,
            false,
            false,
            MountMode::Unmounted,
        );
        window.focus(&probe.focus);
        probe
    });
    cx.run_until_parked();

    cx.simulate_event(KeyUpEvent {
        keystroke: Keystroke::parse("a").expect("known unrelated key"),
    });
    cx.run_until_parked();
    assert_eq!(changes.activations.get(), 0);

    cx.simulate_event(KeyUpEvent {
        keystroke: Keystroke::parse("enter").expect("known keyboard activation key"),
    });
    cx.run_until_parked();
    assert_eq!(changes.activations.get(), 1);
    assert_eq!(changes.next.get(), Some(true));

    let next = changes.next.get().expect("Enter must request open");
    cx.update(|_, app| {
        view.update(app, |probe, cx| {
            probe.open = next;
            cx.notify();
        });
    });
    cx.run_until_parked();

    cx.simulate_event(KeyUpEvent {
        keystroke: Keystroke::parse("space").expect("known keyboard activation key"),
    });
    cx.run_until_parked();
    cx.update(|window, app| {
        let probe = view.read(app);
        assert!(probe.focus.is_focused(window));
        assert_eq!(probe.changes.activations.get(), 2);
        assert_eq!(probe.changes.next.get(), Some(false));
    });
}

#[gpui::test]
fn disabled_collapsible_suppresses_input_and_does_not_acquire_focus(cx: &mut TestAppContext) {
    let changes = ChangeState::new();
    let changes_for_view = changes.clone();
    let (view, cx) = cx.add_window_view(move |_, cx| {
        CollapsibleProbe::new(
            cx,
            changes_for_view,
            false,
            true,
            false,
            MountMode::Unmounted,
        )
    });
    let trigger = cx
        .debug_bounds(TRIGGER_SELECTOR)
        .expect("disabled trigger must remain mounted");

    cx.simulate_click(trigger.center(), Modifiers::none());
    cx.update(|window, app| {
        let focus = view.read(app).focus.clone();
        assert!(!focus.is_focused(window));
        window.focus(&focus);
        view.update(app, |_, cx| cx.notify());
    });
    cx.run_until_parked();

    for key in ["enter", "space"] {
        cx.simulate_event(KeyUpEvent {
            keystroke: Keystroke::parse(key).expect("known keyboard activation key"),
        });
    }
    cx.update(|_, app| {
        assert_eq!(view.read(app).changes.activations.get(), 0);
        assert_eq!(view.read(app).changes.next.get(), None);
        assert!(!view.read(app).open);
    });
}

#[gpui::test]
fn trigger_persists_while_default_content_tracks_controlled_state(cx: &mut TestAppContext) {
    let changes = ChangeState::new();
    let changes_for_view = changes.clone();
    let (view, cx) = cx.add_window_view(move |_, cx| {
        CollapsibleProbe::new(
            cx,
            changes_for_view,
            false,
            false,
            false,
            MountMode::Unmounted,
        )
    });

    let closed_trigger = cx
        .debug_bounds(TRIGGER_SELECTOR)
        .expect("closed trigger must remain mounted");
    assert!(cx.debug_bounds(CONTENT_SELECTOR).is_none());
    assert!(cx.debug_bounds(CONTENT_CHILD_SELECTOR).is_none());

    cx.update(|_, app| {
        view.update(app, |probe, cx| {
            probe.open = true;
            cx.notify();
        });
    });
    cx.run_until_parked();
    assert_eq!(cx.debug_bounds(TRIGGER_SELECTOR), Some(closed_trigger));
    assert!(cx.debug_bounds(CONTENT_SELECTOR).is_some());
    assert!(cx.debug_bounds(CONTENT_CHILD_SELECTOR).is_some());

    cx.update(|_, app| {
        view.update(app, |probe, cx| {
            probe.open = false;
            cx.notify();
        });
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds(TRIGGER_SELECTOR).is_some());
    assert!(cx.debug_bounds(CONTENT_SELECTOR).is_none());
    assert!(cx.debug_bounds(CONTENT_CHILD_SELECTOR).is_none());
    assert_eq!(changes.activations.get(), 0);
}

#[gpui::test]
fn force_mounted_closed_content_is_hidden_and_hidden_until_found_needs_force_mount(
    cx: &mut TestAppContext,
) {
    let changes = ChangeState::new();
    let changes_for_view = changes.clone();
    let (view, cx) = cx.add_window_view(move |_, cx| {
        CollapsibleProbe::new(
            cx,
            changes_for_view,
            false,
            false,
            false,
            MountMode::ForceMountedHidden,
        )
    });

    let force_mounted = cx
        .debug_bounds(CONTENT_SELECTOR)
        .expect("force-mounted content must expose a bound");
    assert_eq!(force_mounted.size.height, px(0.0));
    assert!(cx.debug_bounds(CONTENT_CHILD_SELECTOR).is_none());

    cx.update(|_, app| {
        view.update(app, |probe, cx| {
            probe.mount_mode = MountMode::HiddenUntilFound;
            cx.notify();
        });
    });
    cx.run_until_parked();
    let hidden_until_found = cx
        .debug_bounds(CONTENT_SELECTOR)
        .expect("hidden-until-found content remains mounted");
    assert_eq!(hidden_until_found, force_mounted);
    assert!(cx.debug_bounds(CONTENT_CHILD_SELECTOR).is_none());

    cx.update(|_, app| {
        view.update(app, |probe, cx| {
            probe.mount_mode = MountMode::Unmounted;
            probe.root_selector = AFTER_UNMOUNT_ROOT_SELECTOR;
            cx.notify();
        });
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds(AFTER_UNMOUNT_ROOT_SELECTOR).is_some());
    assert!(cx.debug_bounds(AFTER_UNMOUNT_TRIGGER_SELECTOR).is_some());
    assert!(cx.debug_bounds(AFTER_UNMOUNT_CONTENT_SELECTOR).is_none());
    assert!(cx.debug_bounds(CONTENT_CHILD_SELECTOR).is_none());
    assert_eq!(changes.activations.get(), 0);
}

#[gpui::test]
fn caller_root_refinement_and_debug_geometry_are_stable(cx: &mut TestAppContext) {
    let changes = ChangeState::new();
    let changes_for_view = changes.clone();
    let (view, cx) = cx.add_window_view(move |_, cx| {
        CollapsibleProbe::new(
            cx,
            changes_for_view,
            true,
            false,
            true,
            MountMode::Unmounted,
        )
    });

    let root = cx
        .debug_bounds(ROOT_SELECTOR)
        .expect("root must expose a debug bound");
    let trigger = cx
        .debug_bounds(TRIGGER_SELECTOR)
        .expect("trigger must expose a debug bound");
    let content = cx
        .debug_bounds(CONTENT_SELECTOR)
        .expect("open content must expose a debug bound");
    assert_eq!(root.size.width, px(220.0));
    assert_eq!(root.size.height, px(100.0));
    assert!(trigger.origin.y >= root.origin.y);
    assert!(content.origin.y >= trigger.origin.y);

    cx.update(|_, app| {
        view.update(app, |_, cx| cx.notify());
    });
    cx.run_until_parked();
    assert_eq!(cx.debug_bounds(ROOT_SELECTOR), Some(root));
    assert_eq!(cx.debug_bounds(TRIGGER_SELECTOR), Some(trigger));
    assert_eq!(cx.debug_bounds(CONTENT_SELECTOR), Some(content));
}
