//! Native in-memory GPUI probes for the real [`ProjectPickerView`] leaf and
//! the real [`ProofSurface`] host.
//!
//! Two complementary layers, both driving actual product code through pinned
//! GPUI's test platform:
//!
//! - **Host reachability** mounts the genuine [`ProofSurface`] with the
//!   genuine [`bind_proof_actions`] keymap (the exact registration `run()`
//!   performs) and proves that application Tab/Shift+Tab routing reaches the
//!   picker trigger natively — no pointer, no manual trigger-focus setup —
//!   and that Enter/Space/Escape route correctly from there.
//! - **Leaf behavior** uses a minimal geometry fixture to place the picker
//!   deterministically, probing painted bounds/focus evidence for preferred
//!   placement, collision fallback that must stay attached to an off-origin
//!   trigger, bounded scrolling over a 256-project catalog (initial current
//!   row 255 revealed without any manual scrolling), Home/End/wrap/typeahead
//!   reveal, pointer selection through the deferred menu's capture-phase
//!   dismissal boundary, narrow/short clamps, disabled traversal, and
//!   outside-press versus trigger-toggle ordering.
//!
//! Boundary: this is the pinned **in-memory** harness. It is not OS-window,
//! pixel, IME, or platform-accessibility proof; GPUI 0.2.2 exposes no
//! accessibility tree and these probes make no such claim.

use artisan_domain::ProjectId;
use artisan_frontend::project_picker::{
    PickerRow, ProjectOption, ProjectPickerAction, ProjectPickerView,
};
use artisan_frontend::proof::{ProofSurface, bind_proof_actions};
use artisan_ui::theme::ThemeMode;
use gpui::{
    AppContext as _, Bounds, Context, Entity, FocusHandle, InteractiveElement as _, IntoElement,
    KeyBinding, KeyUpEvent, Keystroke, Modifiers, ParentElement as _, Pixels, Render, Styled as _,
    TestAppContext, VisualTestContext, Window, actions, div, point, px, size,
};

/// Selector painted by the leaf on its trigger row.
const TRIGGER_SELECTOR: &str = "artisan-project-picker-trigger";
/// Selector painted by the leaf on the open menu panel.
const MENU_SELECTOR: &str = "artisan-project-picker-menu";
/// Selector of project row 1 (current row of the round-trip fixture).
const ROW_ONE_SELECTOR: &str = "artisan-project-picker-row-1";
/// Selector of the first project row of a catalog.
const ROW_FIRST_SELECTOR: &str = "artisan-project-picker-row-0";
/// Selector of project row 5 (typeahead target in the long catalog).
const ROW_FIVE_SELECTOR: &str = "artisan-project-picker-row-5";
/// Selector of project row 128 (midpoint current row in a 256 catalog).
const ROW_MID_SELECTOR: &str = "artisan-project-picker-row-128";
/// Selector of the last project row (row 255 of the 256-project catalog).
const ROW_LAST_SELECTOR: &str = "artisan-project-picker-row-255";
/// Selector of the distinct final "New project" row.
const ROW_NEW_SELECTOR: &str = "artisan-project-picker-row-new";

// Geometry-fixture-only traversal actions; the REAL host route under
// `proof::NextTabStop`/`proof::PreviousTabStop` is exercised separately
// against `ProofSurface` further down.
actions!(
    project_picker_native_probes,
    [FixtureNextTabStop, FixturePrevTabStop]
);

/// Key context shared by the fixture host's focused elements and its Tab
/// bindings, mirroring the proven harness dispatch shape.
const FIXTURE_HOST_CONTEXT: &str = "picker-fixture-host";

/// A geometry fixture: one ordinary lead tab stop before the embedded
/// picker, plus fixture-local Tab routing, so tests can place the picker
/// deterministically (tall lead stops push it toward window edges). Real
/// host reachability is proven against `ProofSurface`, not this fixture.
struct PickerHost {
    lead_focus: FocusHandle,
    /// Painted height of the lead stop, stored as `f32` so fixtures can tune
    /// picker placement arithmetically.
    lead_height_px: f32,
    picker: Entity<ProjectPickerView>,
}

impl PickerHost {
    /// Builds the fixture over a catalog and focuses the lead stop — never
    /// the trigger — so traversal evidence stays honest.
    fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        projects: Vec<ProjectOption>,
        current: Option<ProjectId>,
        lead_height: f32,
    ) -> Self {
        let picker = cx
            .new(|picker_cx| ProjectPickerView::new(projects, current, ThemeMode::Dark, picker_cx));
        // Same pinned-GPUI rule as the leaf's trigger: an explicitly tracked
        // handle must carry its own tab settings to become traversable.
        let lead_focus = cx.focus_handle().tab_index(0).tab_stop(true);
        lead_focus.focus(window);
        Self {
            lead_focus,
            lead_height_px: lead_height,
            picker,
        }
    }
}

impl Render for PickerHost {
    fn render(&mut self, _: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .key_context(FIXTURE_HOST_CONTEXT)
            .flex()
            .flex_col()
            .gap(px(8.0))
            .p(px(8.0))
            .on_action(|_: &FixtureNextTabStop, window, _| window.focus_next())
            .on_action(|_: &FixturePrevTabStop, window, _| window.focus_prev())
            .child(
                div()
                    .id("lead-tab-stop")
                    .track_focus(&self.lead_focus)
                    .key_context(FIXTURE_HOST_CONTEXT)
                    .h(px(self.lead_height_px))
                    .w(px(120.0)),
            )
            .child(self.picker.clone())
    }
}

/// Registers the fixture's application-level Tab bindings (GPUI supplies no
/// automatic Tab binding itself).
fn bind_fixture_tab_actions(cx: &mut TestAppContext) {
    cx.update(|app| {
        app.bind_keys([
            KeyBinding::new("tab", FixtureNextTabStop, Some(FIXTURE_HOST_CONTEXT)),
            KeyBinding::new("shift-tab", FixturePrevTabStop, Some(FIXTURE_HOST_CONTEXT)),
        ]);
    });
}

/// Uniform catalog entry: display name `Project NNN`, Forge id `project-nnn`.
fn catalog_entry(index: usize) -> ProjectOption {
    ProjectOption {
        id: ProjectId::parse(format!("project-{index:03}")).expect("catalog ids are valid"),
        name: format!("Project {index:03}").into(),
    }
}

/// The Forge id of [`catalog_entry`] at `index`.
fn catalog_entry_id(index: usize) -> ProjectId {
    ProjectId::parse(format!("project-{index:03}")).expect("catalog ids are valid")
}

/// Painted bounds for a debug selector, failing loudly when absent.
fn painted_bounds(cx: &mut VisualTestContext, selector: &'static str) -> Bounds<Pixels> {
    cx.debug_bounds(selector)
        .unwrap_or_else(|| panic!("selector `{selector}` never painted bounds"))
}

/// Asserts `inner` lies fully inside `outer` (sub-pixel tolerance).
fn assert_visible_within(inner: Bounds<Pixels>, outer: Bounds<Pixels>, label: &str) {
    assert!(
        inner.origin.x >= outer.origin.x - px(0.6)
            && inner.right() <= outer.right() + px(0.6)
            && inner.origin.y >= outer.origin.y - px(0.6)
            && inner.bottom() <= outer.bottom() + px(0.6),
        "{label} must be visible inside the panel: row {inner:?} vs panel {outer:?}"
    );
}

/// Asserts `bounds` lies fully inside the `0..width × 0..height` viewport.
fn assert_inside_viewport(bounds: Bounds<Pixels>, width: f32, height: f32, label: &str) {
    assert!(
        bounds.origin.x >= px(-0.6)
            && bounds.origin.y >= px(-0.6)
            && bounds.right() <= px(width) + px(0.6)
            && bounds.bottom() <= px(height) + px(0.6),
        "{label} must stay inside the {width}×{height} viewport: {bounds:?}"
    );
}

/// Delivers the DOWN half of an unmodified key press. Pinned
/// `Window::dispatch_keystroke` (driven by `simulate_keystrokes`) dispatches
/// key-down only; the release half is delivered separately so probes can
/// observe and assert each phase of the real key lifecycle.
fn press_down(cx: &mut VisualTestContext, key: &'static str) {
    cx.simulate_keystrokes(key);
}

/// Delivers the UP half of a key press. On a focused element with click
/// listeners, pinned GPUI synthesizes an Enter/Space click from exactly this
/// event (div.rs key-up synthesis).
fn release_key(cx: &mut VisualTestContext, key: &'static str) {
    cx.simulate_event(KeyUpEvent {
        keystroke: Keystroke::parse(key).expect("known keyboard activation key"),
    });
}

/// Delivers one complete physical key press: genuine down followed by
/// genuine release, nothing substituted.
fn complete_press(cx: &mut VisualTestContext, key: &'static str) {
    press_down(cx, key);
    release_key(cx, key);
}

/// Opens the fixture menu from pure keyboard input — Tab to the trigger,
/// then a COMPLETE Enter press whose halves are individually asserted: the
/// down half alone must stay inert and the release must synthesize exactly
/// one opening click. Parks afterwards so the frames consuming the pending
/// scroll settle.
fn open_menu_from_keyboard(host: &Entity<PickerHost>, cx: &mut VisualTestContext) {
    cx.simulate_keystrokes("tab");
    // Keyboard-click synthesis registers per paint while the trigger is
    // focused, so a frame must pass before the press can reach that listener.
    cx.run_until_parked();
    cx.update(|window, app| {
        let trigger = host.read(app).picker.read(app).trigger_focus().clone();
        assert!(
            trigger.is_focused(window),
            "traversal must reach the trigger"
        );
        let picker = host.read(app).picker.clone();
        assert!(!picker.read(app).state().is_open());
    });
    press_down(cx, "enter");
    cx.update(|_, app| {
        let picker = host.read(app).picker.clone();
        assert!(
            !picker.read(app).state().is_open(),
            "the down half alone must not activate"
        );
    });
    release_key(cx, "enter");
    cx.run_until_parked();
    cx.update(|_, app| {
        let picker = host.read(app).picker.clone();
        assert!(
            picker.read(app).state().is_open(),
            "the complete press must have opened the menu"
        );
    });
}

#[gpui::test]
fn real_proof_host_traversal_reaches_the_trigger_and_routes_keys(cx: &mut TestAppContext) {
    // The exact keymap the real entry point registers, through the exact
    // shared registration function.
    cx.update(bind_proof_actions);
    let (surface, cx) = cx.add_window_view(ProofSurface::new);
    cx.run_until_parked();

    // The proof surface starts focused on its own root, never the trigger.
    cx.update(|window, app| {
        let root = surface.read(app).root_focus().clone();
        assert!(
            root.is_focused(window),
            "fixture must start on the proof root"
        );
        let trigger = surface.read(app).picker().read(app).trigger_focus().clone();
        assert!(!trigger.is_focused(window));
    });

    // One real Tab press lands natively on the picker trigger through the
    // actual proof-host routing (proof::NextTabStop -> focus_next).
    cx.simulate_keystrokes("tab");
    cx.run_until_parked();
    cx.update(|window, app| {
        let trigger = surface.read(app).picker().read(app).trigger_focus().clone();
        assert!(
            trigger.is_focused(window),
            "the real proof host must traverse onto the picker trigger"
        );
    });

    // A COMPLETE Enter press opens exactly once — the down half is inert and
    // only the release synthesizes the opening click — and addresses the
    // proof catalog's current row (core, index 0).
    press_down(cx, "enter");
    cx.update(|_, app| {
        let picker = surface.read(app).picker().clone();
        assert!(
            !picker.read(app).state().is_open(),
            "the down half alone must not activate"
        );
    });
    release_key(cx, "enter");
    cx.run_until_parked();
    cx.update(|_, app| {
        let picker = surface.read(app).picker().clone();
        let state = picker.read(app).state();
        assert!(state.is_open(), "the complete press must open exactly once");
        assert_eq!(state.highlighted_row(), Some(PickerRow::Project(0)));
    });

    // Escape dismisses and restores the trigger.
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    cx.update(|window, app| {
        let picker = surface.read(app).picker().clone();
        assert!(!picker.read(app).state().is_open());
        let trigger = picker.read(app).trigger_focus().clone();
        assert!(
            trigger.is_focused(window),
            "escape must restore trigger focus"
        );
    });

    // Shift+Tab from the trigger wraps within the host's stop set (the
    // trigger is currently the only stop): focus stays put, sensibly.
    cx.simulate_keystrokes("shift-tab");
    cx.update(|window, app| {
        let trigger = surface.read(app).picker().read(app).trigger_focus().clone();
        assert!(trigger.is_focused(window), "shift-tab must not lose focus");
    });

    // Space reopens from the focused trigger.
    complete_press(cx, "space");
    cx.run_until_parked();
    cx.update(|_, app| {
        let picker = surface.read(app).picker().clone();
        assert!(
            picker.read(app).state().is_open(),
            "space must open from the focused trigger"
        );
    });
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();

    // The proof shell's click-counter presentation is intact: a neutral
    // press on the background records exactly one click.
    cx.simulate_click(point(px(40.0), px(40.0)), Modifiers::none());
    cx.run_until_parked();
    cx.update(|_, app| {
        assert_eq!(surface.read(app).clicks(), 1);
    });
}

#[gpui::test]
fn keyboard_only_activation_emits_choose_new_project_then_current_noop(cx: &mut TestAppContext) {
    bind_fixture_tab_actions(cx);
    let (host, cx) = cx.add_window_view(|window, cx| {
        PickerHost::new(
            window,
            cx,
            (0..3).map(catalog_entry).collect(),
            Some(catalog_entry_id(0)),
            240.0,
        )
    });
    cx.simulate_resize(size(px(800.0), px(600.0)));
    cx.run_until_parked();

    // Open on the current row 0, move down to row 1, activate with a
    // COMPLETE Enter press: Choose closes and STAYS CLOSED through release,
    // with focus restored only after that release.
    open_menu_from_keyboard(&host, cx);
    cx.simulate_keystrokes("down");
    cx.run_until_parked();
    press_down(cx, "enter");
    cx.run_until_parked();
    cx.update(|_window, app| {
        let picker = host.read(app).picker.clone();
        assert!(!picker.read(app).state().is_open(), "choosing closes first");
        assert_eq!(
            picker.read(app).last_action(),
            Some(ProjectPickerAction::Choose(catalog_entry_id(1))),
        );
        // The down half already restored trigger focus eagerly; the release
        // below must not toggle anything.
    });
    release_key(cx, "enter");
    cx.run_until_parked();
    cx.update(|window, app| {
        let picker = host.read(app).picker.clone();
        assert!(
            !picker.read(app).state().is_open(),
            "the release must not reopen the menu"
        );
        assert_eq!(
            picker.read(app).last_action(),
            Some(ProjectPickerAction::Choose(catalog_entry_id(1))),
            "release must not emit a second action"
        );
        let trigger = picker.read(app).trigger_focus().clone();
        assert!(trigger.is_focused(window), "release keeps trigger focus");
    });

    // Reopening addresses the repointed current row; End reaches the final
    // row behind its separator; a COMPLETE Space press activates it:
    // New project closes and stays closed through release.
    complete_press(cx, "enter");
    cx.run_until_parked();
    cx.update(|_, app| {
        let picker = host.read(app).picker.clone();
        assert_eq!(
            picker.read(app).state().highlighted_row(),
            Some(PickerRow::Project(1)),
            "the repointed current project owns the reopen focus"
        );
    });
    cx.simulate_keystrokes("end");
    cx.run_until_parked();
    let menu = painted_bounds(cx, MENU_SELECTOR);
    let new_row = painted_bounds(cx, ROW_NEW_SELECTOR);
    assert_visible_within(new_row, menu, "final row after End in a short catalog");
    press_down(cx, "space");
    cx.run_until_parked();
    cx.update(|_, app| {
        let picker = host.read(app).picker.clone();
        assert!(!picker.read(app).state().is_open());
        assert_eq!(
            picker.read(app).last_action(),
            Some(ProjectPickerAction::NewProject),
        );
    });
    release_key(cx, "space");
    cx.run_until_parked();
    cx.update(|_, app| {
        let picker = host.read(app).picker.clone();
        assert!(
            !picker.read(app).state().is_open(),
            "the release must not reopen the menu"
        );
        assert_eq!(
            picker.read(app).last_action(),
            Some(ProjectPickerAction::NewProject),
            "release must not emit a second action"
        );
    });

    // Reopen (highlights current row 1) and activate the current project
    // with a COMPLETE Enter press: closes as a no-op through release.
    complete_press(cx, "enter");
    cx.run_until_parked();
    press_down(cx, "enter");
    cx.run_until_parked();
    release_key(cx, "enter");
    cx.run_until_parked();
    cx.update(|_, app| {
        let picker = host.read(app).picker.clone();
        assert!(!picker.read(app).state().is_open());
        assert_eq!(
            picker.read(app).last_action(),
            Some(ProjectPickerAction::NewProject),
            "activating the current project must stay a no-op"
        );
    });
}

#[gpui::test]
fn pointer_selection_survives_the_capture_phase_dismissal_boundary(cx: &mut TestAppContext) {
    bind_fixture_tab_actions(cx);
    let (host, cx) = cx.add_window_view(|window, cx| {
        PickerHost::new(
            window,
            cx,
            (0..3).map(catalog_entry).collect(),
            Some(catalog_entry_id(0)),
            60.0,
        )
    });
    cx.simulate_resize(size(px(800.0), px(600.0)));
    cx.run_until_parked();

    // The menu paints as a deferred layer OUTSIDE the root hitbox; the
    // capture-phase outside-press handler must let presses inside the live
    // menu bounds through to row activation.
    open_menu_from_keyboard(&host, cx);

    // Pointer-select a non-current project through its painted bounds:
    // exactly one Choose, emitted after the close.
    let menu = painted_bounds(cx, MENU_SELECTOR);
    let row = painted_bounds(cx, ROW_ONE_SELECTOR);
    assert_visible_within(row, menu, "target row before pointer selection");
    cx.simulate_click(row.center(), Modifiers::none());
    cx.run_until_parked();
    cx.update(|window, app| {
        let picker = host.read(app).picker.clone();
        assert!(
            !picker.read(app).state().is_open(),
            "selection closes first"
        );
        assert_eq!(
            picker.read(app).last_action(),
            Some(ProjectPickerAction::Choose(catalog_entry_id(1))),
            "the deferred menu must not swallow row presses"
        );
        let trigger = picker.read(app).trigger_focus().clone();
        assert!(trigger.is_focused(window));
    });

    // Pointer-select the final New project row behind its separator.
    complete_press(cx, "enter");
    cx.run_until_parked();
    let menu = painted_bounds(cx, MENU_SELECTOR);
    let new_row = painted_bounds(cx, ROW_NEW_SELECTOR);
    assert_visible_within(new_row, menu, "final row before pointer selection");
    cx.simulate_click(new_row.center(), Modifiers::none());
    cx.run_until_parked();
    cx.update(|_, app| {
        let picker = host.read(app).picker.clone();
        assert!(!picker.read(app).state().is_open());
        assert_eq!(
            picker.read(app).last_action(),
            Some(ProjectPickerAction::NewProject),
        );
    });

    // Reopen (current row 1 highlighted) and pointer-select the CURRENT
    // project: closing no-op, no further action.
    complete_press(cx, "enter");
    cx.run_until_parked();
    let row = painted_bounds(cx, ROW_ONE_SELECTOR);
    cx.simulate_click(row.center(), Modifiers::none());
    cx.run_until_parked();
    cx.update(|_, app| {
        let picker = host.read(app).picker.clone();
        assert!(!picker.read(app).state().is_open());
        assert_eq!(
            picker.read(app).last_action(),
            Some(ProjectPickerAction::NewProject),
            "pointer-activating the current project must stay a no-op"
        );
    });
}

#[gpui::test]
fn long_catalog_reveals_current_home_end_wrap_typeahead_and_new_project(cx: &mut TestAppContext) {
    bind_fixture_tab_actions(cx);
    let (host, cx) = cx.add_window_view(|window, cx| {
        PickerHost::new(
            window,
            cx,
            (0..256).map(catalog_entry).collect(),
            Some(catalog_entry_id(255)),
            240.0,
        )
    });
    cx.simulate_resize(size(px(800.0), px(600.0)));
    cx.run_until_parked();

    open_menu_from_keyboard(&host, cx);

    // The menu body is bounded even though all 256 rows exist...
    let menu = painted_bounds(cx, MENU_SELECTOR);
    assert!(
        menu.size.height <= px(360.0) + px(0.6),
        "the panel must bound its height instead of growing unbounded: {menu:?}"
    );

    // ...and the initial current row (255, the very last project) is
    // scrolled into view by the leaf itself — no manual scrolling anywhere.
    let last_row = painted_bounds(cx, ROW_LAST_SELECTOR);
    assert_visible_within(last_row, menu, "initial current row 255");

    // End lands on the final "New project" row behind its separator...
    cx.simulate_keystrokes("end");
    cx.run_until_parked();
    let menu = painted_bounds(cx, MENU_SELECTOR);
    let new_row = painted_bounds(cx, ROW_NEW_SELECTOR);
    assert_visible_within(new_row, menu, "New project row after End");

    // ...Home returns to the first project row...
    cx.simulate_keystrokes("home");
    cx.run_until_parked();
    let menu = painted_bounds(cx, MENU_SELECTOR);
    let first_row = painted_bounds(cx, ROW_FIRST_SELECTOR);
    assert_visible_within(first_row, menu, "first row after Home");

    // ...Up wraps backwards onto the final row...
    cx.simulate_keystrokes("up");
    cx.run_until_parked();
    let menu = painted_bounds(cx, MENU_SELECTOR);
    let new_row = painted_bounds(cx, ROW_NEW_SELECTOR);
    assert_visible_within(new_row, menu, "New project row after wrapping Up");

    // ...Home plus five real printable 'p' keystrokes typeahead onto row 5
    // (`simulate_keystrokes` splits its argument on spaces, so each "p" is
    // one genuine repeated press cycling the prefix buffer)...
    cx.simulate_keystrokes("home");
    cx.run_until_parked();
    cx.simulate_keystrokes("p p p p p");
    cx.run_until_parked();
    cx.update(|_, app| {
        let picker = host.read(app).picker.clone();
        assert_eq!(
            picker.read(app).state().highlighted_row(),
            Some(PickerRow::Project(5)),
            "repeated prefix typeahead must advance row by row"
        );
    });
    let menu = painted_bounds(cx, MENU_SELECTOR);
    let typed_row = painted_bounds(cx, ROW_FIVE_SELECTOR);
    assert_visible_within(typed_row, menu, "typeahead target row 5");

    // ...and the final row stays selectable straight from the keyboard: a
    // COMPLETE Space press closes and stays closed through release.
    cx.simulate_keystrokes("end");
    cx.run_until_parked();
    press_down(cx, "space");
    cx.run_until_parked();
    cx.update(|_, app| {
        let picker = host.read(app).picker.clone();
        assert!(!picker.read(app).state().is_open());
        assert_eq!(
            picker.read(app).last_action(),
            Some(ProjectPickerAction::NewProject),
        );
    });
    release_key(cx, "space");
    cx.run_until_parked();
    cx.update(|_, app| {
        let picker = host.read(app).picker.clone();
        assert!(
            !picker.read(app).state().is_open(),
            "the release must not reopen the menu"
        );
        assert_eq!(
            picker.read(app).last_action(),
            Some(ProjectPickerAction::NewProject),
            "release must not emit a second action"
        );
    });
}

#[gpui::test]
fn narrow_and_short_viewports_clamp_the_menu_body(cx: &mut TestAppContext) {
    bind_fixture_tab_actions(cx);
    let (host, cx) = cx.add_window_view(|window, cx| {
        PickerHost::new(window, cx, (0..5).map(catalog_entry).collect(), None, 60.0)
    });
    // Deliberately small: narrower than the 320 px preferred width and
    // shorter than the 360 px preferred height.
    cx.simulate_resize(size(px(240.0), px(300.0)));
    cx.run_until_parked();

    open_menu_from_keyboard(&host, cx);

    let menu = painted_bounds(cx, MENU_SELECTOR);
    assert!(
        (menu.size.width - px(208.0)).abs() < px(0.6),
        "width must clamp to viewport minus the legacy inset: {menu:?}"
    );
    assert!(
        menu.size.height <= px(268.0) + px(0.6),
        "height must clamp so the panel plus inset fits the window: {menu:?}"
    );
    assert_inside_viewport(menu, 240.0, 300.0, "clamped menu");

    let first_row = painted_bounds(cx, ROW_FIRST_SELECTOR);
    assert_visible_within(first_row, menu, "fallback current row in a tiny window");
}

#[gpui::test]
fn preferred_top_start_placement_with_gap_holds_when_it_fits(cx: &mut TestAppContext) {
    bind_fixture_tab_actions(cx);
    let (host, cx) = cx.add_window_view(|window, cx| {
        PickerHost::new(
            window,
            cx,
            (0..6).map(catalog_entry).collect(),
            Some(catalog_entry_id(2)),
            320.0,
        )
    });
    cx.simulate_resize(size(px(800.0), px(600.0)));
    cx.run_until_parked();

    open_menu_from_keyboard(&host, cx);

    let trigger = painted_bounds(cx, TRIGGER_SELECTOR);
    let menu = painted_bounds(cx, MENU_SELECTOR);

    // Preferred placement: side="top" align="start" sideOffset={10}. The
    // lead stop is tall enough that the whole panel fits above the trigger.
    assert!(
        ((menu.bottom() - trigger.top()) + px(10.0)).abs() < px(0.6),
        "the panel must sit 10px above the trigger: menu {menu:?} trigger {trigger:?}"
    );
    assert!(
        (menu.origin.x - trigger.origin.x).abs() < px(0.6),
        "start alignment keeps left edges flush: {menu:?} vs {trigger:?}"
    );
    assert!(
        (menu.size.width - px(320.0)).abs() < px(0.6),
        "the preferred width bound applies when the viewport allows it: {menu:?}"
    );
    assert_inside_viewport(menu, 800.0, 600.0, "preferred placement");
}

#[gpui::test]
fn collision_fallback_stays_attached_to_an_off_origin_trigger(cx: &mut TestAppContext) {
    bind_fixture_tab_actions(cx);
    // The trigger sits well away from the window origin (lead stop 240px +
    // padding puts it around y=256, x=8) and the bounded 256-row panel
    // cannot fit between the window top and the trigger: pinned SwitchAnchor
    // flips. In window position mode every flip candidate keeps the real
    // trigger origin — unlike the pinned Local-mode quirk that collapses
    // flipped candidates onto the viewport origin.
    let (host, cx) = cx.add_window_view(|window, cx| {
        PickerHost::new(
            window,
            cx,
            (0..256).map(catalog_entry).collect(),
            Some(catalog_entry_id(255)),
            240.0,
        )
    });
    cx.simulate_resize(size(px(480.0), px(320.0)));
    cx.run_until_parked();

    open_menu_from_keyboard(&host, cx);

    let trigger = painted_bounds(cx, TRIGGER_SELECTOR);
    let menu = painted_bounds(cx, MENU_SELECTOR);

    // Containment is necessary but NOT sufficient: the fallback must also
    // remain meaningfully attached to the trigger. Pinned SwitchAnchor snaps
    // the flipped panel against the window's top edge when the space below
    // the trigger cannot hold it either — vertical origin at 0 is correct —
    // but the horizontal attachment distinguishes real anchor behavior from
    // the pinned Local-mode quirk that collapsed candidates onto x≈0.
    assert_inside_viewport(menu, 480.0, 320.0, "collided menu");
    let left_attached = (menu.origin.x - trigger.origin.x).abs() < px(0.6);
    let right_attached = (menu.right() - trigger.right()).abs() < px(0.6);
    assert!(
        left_attached || right_attached,
        "collision fallback must stay horizontally attached to the trigger: \
         menu {menu:?} trigger {trigger:?}"
    );
    assert!(
        menu.origin.x > px(4.0),
        "flipped placement must not collapse onto the viewport origin: {menu:?}"
    );

    // The trigger still toggles normally after collision handling.
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    cx.update(|_, app| {
        let picker = host.read(app).picker.clone();
        assert!(!picker.read(app).state().is_open());
    });
}

#[gpui::test]
fn disabled_trigger_leaves_tab_traversal_and_refuses_to_open(cx: &mut TestAppContext) {
    bind_fixture_tab_actions(cx);
    let (host, cx) = cx.add_window_view(|window, cx| {
        PickerHost::new(window, cx, (0..4).map(catalog_entry).collect(), None, 240.0)
    });
    cx.simulate_resize(size(px(800.0), px(600.0)));
    cx.run_until_parked();

    cx.update(|_, app| {
        host.update(app, |host, cx| {
            host.picker.update(cx, |picker, picker_cx| {
                picker.set_disabled(true, picker_cx);
            });
        });
    });
    cx.run_until_parked();

    // Traversal skips the disabled trigger entirely and wraps back to the
    // only remaining stop.
    cx.simulate_keystrokes("tab");
    cx.update(|window, app| {
        let lead = host.read(app).lead_focus.clone();
        let trigger = host.read(app).picker.read(app).trigger_focus().clone();
        assert!(lead.is_focused(window), "disabled triggers leave traversal");
        assert!(!trigger.is_focused(window));
    });

    // Enter on the lead stop does nothing: the picker stays closed.
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    cx.update(|_, app| {
        let picker = host.read(app).picker.clone();
        assert!(!picker.read(app).state().is_open());
    });

    // Re-enabling makes the trigger traversable again.
    cx.update(|_, app| {
        host.update(app, |host, cx| {
            host.picker.update(cx, |picker, picker_cx| {
                picker.set_disabled(false, picker_cx);
            });
        });
    });
    cx.run_until_parked();
    cx.simulate_keystrokes("tab");
    cx.update(|window, app| {
        let trigger = host.read(app).picker.read(app).trigger_focus().clone();
        assert!(
            trigger.is_focused(window),
            "re-enabled triggers rejoin traversal"
        );
    });
}

#[gpui::test]
fn outside_presses_dismiss_while_trigger_clicks_still_toggle(cx: &mut TestAppContext) {
    bind_fixture_tab_actions(cx);
    let (host, cx) = cx.add_window_view(|window, cx| {
        PickerHost::new(window, cx, (0..4).map(catalog_entry).collect(), None, 240.0)
    });
    cx.simulate_resize(size(px(800.0), px(600.0)));
    cx.run_until_parked();

    open_menu_from_keyboard(&host, cx);

    // A press on the trigger itself toggles the open menu closed (the press
    // is inside the root hitbox, so the outside-press handler never runs).
    let trigger = painted_bounds(cx, TRIGGER_SELECTOR);
    let center = trigger.center();
    cx.simulate_click(center, Modifiers::none());
    cx.run_until_parked();
    cx.update(|_, app| {
        let picker = host.read(app).picker.clone();
        assert!(
            !picker.read(app).state().is_open(),
            "pressing the trigger while open must toggle it closed"
        );
    });

    // Reopen, then press far outside the leaf: outside dismissal fires and
    // focus settles back on the trigger.
    complete_press(cx, "enter");
    cx.run_until_parked();
    cx.update(|_, app| {
        let picker = host.read(app).picker.clone();
        assert!(picker.read(app).state().is_open(), "fixture must be open");
    });
    cx.simulate_click(point(px(760.0), px(560.0)), Modifiers::none());
    cx.run_until_parked();
    cx.update(|window, app| {
        let picker = host.read(app).picker.clone();
        assert!(
            !picker.read(app).state().is_open(),
            "outside presses dismiss"
        );
        let trigger_focus = picker.read(app).trigger_focus().clone();
        assert!(trigger_focus.is_focused(window));
    });
}

#[gpui::test]
fn midpoint_current_row_is_revealed_on_first_open(cx: &mut TestAppContext) {
    bind_fixture_tab_actions(cx);
    // A midpoint current row exercises the general first-open reveal: it is
    // neither the visible-by-default top of the catalog nor the tail, so
    // only a genuine scroll request consumed with real handle geometry can
    // make it visible. Ordinary row sizes, no manual scrolling anywhere.
    let (host, cx) = cx.add_window_view(|window, cx| {
        PickerHost::new(
            window,
            cx,
            (0..256).map(catalog_entry).collect(),
            Some(catalog_entry_id(128)),
            240.0,
        )
    });
    cx.simulate_resize(size(px(800.0), px(600.0)));
    cx.run_until_parked();

    open_menu_from_keyboard(&host, cx);

    let menu = painted_bounds(cx, MENU_SELECTOR);
    assert!(
        menu.size.height <= px(360.0) + px(0.6),
        "the panel must stay height-bounded: {menu:?}"
    );
    let mid_row = painted_bounds(cx, ROW_MID_SELECTOR);
    assert_visible_within(mid_row, menu, "midpoint current row 128 on first open");
}

#[gpui::test]
fn open_menu_follows_host_reflow_and_stays_clickable(cx: &mut TestAppContext) {
    // The REAL proof host: its centered column genuinely repositions the
    // picker whenever the window size changes, so resizing while the menu is
    // already open exercises production follow-the-trigger placement rather
    // than a pre-open resize.
    cx.update(bind_proof_actions);
    let (surface, cx) = cx.add_window_view(ProofSurface::new);
    cx.simulate_resize(size(px(800.0), px(600.0)));
    cx.run_until_parked();

    // Tab to the trigger through the actual proof-host route and open with
    // one complete Enter press.
    cx.simulate_keystrokes("tab");
    cx.run_until_parked();
    cx.update(|window, app| {
        let trigger = surface.read(app).picker().read(app).trigger_focus().clone();
        assert!(trigger.is_focused(window));
    });
    complete_press(cx, "enter");
    cx.run_until_parked();

    let trigger_before = painted_bounds(cx, TRIGGER_SELECTOR);
    let menu_before = painted_bounds(cx, MENU_SELECTOR);
    assert!(
        (menu_before.bottom() - trigger_before.top() + px(10.0)).abs() < px(0.6),
        "fixture must open in the preferred placement before reflow"
    );

    // Resize the host while the menu is open.
    cx.simulate_resize(size(px(1000.0), px(760.0)));
    cx.run_until_parked();

    let trigger_after = painted_bounds(cx, TRIGGER_SELECTOR);
    let menu_after = painted_bounds(cx, MENU_SELECTOR);

    // The host really moved the trigger — this is not a no-op resize.
    assert!(
        (trigger_after.center().x - trigger_before.center().x).abs() > px(40.0)
            && (trigger_after.center().y - trigger_before.center().y).abs() > px(40.0),
        "the fixture must move the trigger substantially: \
         {trigger_before:?} -> {trigger_after:?}"
    );

    // The open menu followed the moved trigger with audited top/start +10px
    // preferred placement (space allows here) and stays viewport bounded.
    assert!(
        ((menu_after.bottom() - trigger_after.top()) + px(10.0)).abs() < px(0.6),
        "the reopened geometry must sit 10px above the moved trigger: \
         menu {menu_after:?} trigger {trigger_after:?}"
    );
    assert!(
        (menu_after.origin.x - trigger_after.origin.x).abs() < px(0.6),
        "start alignment must follow the moved trigger"
    );
    assert_inside_viewport(menu_after, 1000.0, 760.0, "reflowed menu");

    // The settled geometry is genuinely visible AND clickable: pointer-select
    // the non-current docs-site row through its new painted center.
    let row = painted_bounds(cx, ROW_ONE_SELECTOR);
    assert_visible_within(row, menu_after, "target row after reflow");
    cx.simulate_click(row.center(), Modifiers::none());
    cx.run_until_parked();
    cx.update(|_, app| {
        let picker = surface.read(app).picker().clone();
        assert!(
            !picker.read(app).state().is_open(),
            "post-reflow selection closes the menu"
        );
        assert_eq!(
            picker.read(app).last_action(),
            Some(ProjectPickerAction::Choose(
                ProjectId::parse("proof-docs-site").expect("proof ids are valid")
            )),
            "the post-reflow click must choose docs-site exactly once"
        );
    });
}
