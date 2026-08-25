//! Packet 7.04 compile-time harness proof for pinned GPUI 0.2.2.
//!
//! These tests exercise only GPUI's own external test harness — `#[gpui::test]`,
//! `TestAppContext`, `VisualTestContext`, `add_window_view`, focus tracking,
//! keybindings, simulated keystrokes/clicks, `debug_selector`/`debug_bounds`,
//! and deterministic run-until-parked executors — against GPUI's in-memory
//! test platform, using a tiny test-only view.
//!
//! Boundary: this is compile-time test-harness capability only. It is not
//! accessibility evidence, a semantic tree, screenshot fidelity, a real
//! Windows native-window proof, an OS-theme proof, a menu proof, or an IME
//! proof.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use gpui::{
    Context, FocusHandle, InteractiveElement, IntoElement, KeyBinding, Modifiers, ParentElement,
    Render, StatefulInteractiveElement, Styled, TestAppContext, Window, actions, div, point, px,
};

/// Selector recorded on the painted probe element.
const PROBE_SELECTOR: &str = "artisan-harness-probe";

/// Key context matched by the test-only binding.
const PROBE_CONTEXT: &str = "harness";

// Test-only, namespaced action; deliberately carries no product meaning.
actions!(gpui_harness_proof, [Tick]);

/// Per-handler invocation counters shared out of the probe view.
#[derive(Clone, Default)]
struct ProbeCounts {
    key_downs: Rc<Cell<u32>>,
    ticks: Rc<Cell<u32>>,
    clicks: Rc<Cell<u32>>,
}

/// Minimal test-only Render view carrying one tracked focus handle and one
/// painted, interactive target element.
struct HarnessProbe {
    focus: FocusHandle,
    counts: ProbeCounts,
}

impl HarnessProbe {
    fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus: cx.focus_handle(),
            counts: ProbeCounts::default(),
        }
    }
}

impl Render for HarnessProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let key_downs = self.counts.key_downs.clone();
        let ticks = self.counts.ticks.clone();
        let clicks = self.counts.clicks.clone();

        div().size_full().child(
            div()
                .id("probe")
                .key_context(PROBE_CONTEXT)
                .track_focus(&self.focus)
                .w(px(120.0))
                .h(px(40.0))
                .on_key_down(move |_, _, _| key_downs.set(key_downs.get() + 1))
                .on_action(move |_: &Tick, _, _| ticks.set(ticks.get() + 1))
                .on_click(move |_, _, _| clicks.set(clicks.get() + 1))
                .debug_selector(|| PROBE_SELECTOR.to_string()),
        )
    }
}

/// A painted element with a stable id/debug selector exposes its debug bounds.
#[gpui::test]
fn painted_element_exposes_debug_bounds(cx: &mut TestAppContext) {
    let (_view, cx) = cx.add_window_view(|_, cx| HarnessProbe::new(cx));

    let Some(bounds) = cx.debug_bounds(PROBE_SELECTOR) else {
        panic!("painted probe element did not record debug bounds");
    };
    assert!(
        f32::from(bounds.size.width) > 0.0 && f32::from(bounds.size.height) > 0.0,
        "probe bounds must be non-empty, got {bounds:?}"
    );
}

/// A real tracked `FocusHandle` can be focused, and unmodified keyboard input
/// reaches its key-down handler exactly once.
#[gpui::test]
fn tracked_focus_and_unmodified_keystroke_reach_handlers_once(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, cx| HarnessProbe::new(cx));

    cx.update(|window, app| {
        let focus: FocusHandle = view.read(app).focus.clone();
        window.focus(&focus);
        assert!(focus.is_focused(window), "tracked handle must take focus");
    });

    cx.simulate_keystrokes("a");

    cx.update(|_, app| {
        let counts = &view.read(app).counts;
        assert_eq!(counts.key_downs.get(), 1, "key down handled exactly once");
        assert_eq!(counts.ticks.get(), 0, "no action may fire for plain 'a'");
        assert_eq!(counts.clicks.get(), 0, "no click may fire for plain 'a'");
    });
}

/// A test-only namespaced action bound through the focused element's key
/// context reaches that element's action handler exactly once.
#[gpui::test]
fn namespaced_test_action_binding_reaches_focused_element_once(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, cx| HarnessProbe::new(cx));

    cx.update(|_, app| {
        app.bind_keys(vec![KeyBinding::new(
            "ctrl-alt-t",
            Tick,
            Some(PROBE_CONTEXT),
        )]);
    });
    cx.update(|window, app| {
        let focus: FocusHandle = view.read(app).focus.clone();
        window.focus(&focus);
        assert!(focus.is_focused(window), "tracked handle must take focus");
    });

    cx.simulate_keystrokes("ctrl-alt-t");

    cx.update(|_, app| {
        let counts = &view.read(app).counts;
        assert_eq!(counts.ticks.get(), 1, "action handled exactly once");
        assert_eq!(counts.clicks.get(), 0, "no click side effects");
    });
}

/// A primary pointer click aimed at the painted element's debug-bounds center
/// reaches its click handler exactly once.
#[gpui::test]
fn primary_pointer_click_reaches_handler_exactly_once(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, cx| HarnessProbe::new(cx));

    let Some(bounds) = cx.debug_bounds(PROBE_SELECTOR) else {
        panic!("painted probe element did not record debug bounds");
    };
    let center = point(
        bounds.origin.x + bounds.size.width / 2.0,
        bounds.origin.y + bounds.size.height / 2.0,
    );

    cx.simulate_click(center, Modifiers::none());

    cx.update(|_, app| {
        let counts = &view.read(app).counts;
        assert_eq!(counts.clicks.get(), 1, "click handled exactly once");
        assert_eq!(counts.ticks.get(), 0, "click must not dispatch actions");
    });
}

/// Background and foreground executors settle deterministically under
/// `run_until_parked`: nothing runs early, everything runs once, and repeated
/// parking changes nothing.
#[gpui::test]
fn executors_settle_deterministically_under_run_until_parked(cx: &mut TestAppContext) {
    let background_done = Arc::new(AtomicUsize::new(0));
    let foreground_done = Arc::new(AtomicUsize::new(0));

    let background_task = {
        let done = Arc::clone(&background_done);
        cx.background_executor.spawn(async move {
            done.fetch_add(1, Ordering::SeqCst);
        })
    };
    let foreground_task = {
        let done = Arc::clone(&foreground_done);
        cx.spawn(|_| async move {
            done.fetch_add(1, Ordering::SeqCst);
        })
    };

    assert_eq!(
        background_done.load(Ordering::SeqCst),
        0,
        "background task must not run before parking"
    );
    assert_eq!(
        foreground_done.load(Ordering::SeqCst),
        0,
        "foreground task must not run before parking"
    );

    cx.run_until_parked();
    background_task.detach();
    foreground_task.detach();

    assert_eq!(
        background_done.load(Ordering::SeqCst),
        1,
        "background task must complete exactly once"
    );
    assert_eq!(
        foreground_done.load(Ordering::SeqCst),
        1,
        "foreground task must complete exactly once"
    );

    cx.run_until_parked();
    assert_eq!(
        background_done.load(Ordering::SeqCst),
        1,
        "repeated parking must stay settled"
    );
    assert_eq!(
        foreground_done.load(Ordering::SeqCst),
        1,
        "repeated parking must stay settled"
    );
}
