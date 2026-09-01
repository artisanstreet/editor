//! Focused coverage for the native thread picker and its application boundary.
//!
//! The pure tests exercise the presentation mirror and the existing
//! `ThreadListSelection` state machine. The GPUI test drives the real deferred
//! menu so row geometry, focus, scrolling, selectors, and pointer activation
//! are covered on the pinned in-memory surface.

use artisan_domain::{ProjectId, ThreadId, ThreadListing, ThreadSummary, ThreadTitle, UnixMillis};
use artisan_frontend::native_thread_picker::{
    NativeThreadPicker, NativeThreadPickerState, THREAD_PICKER_MAX_ROWS,
    THREAD_PICKER_MENU_SELECTOR, THREAD_PICKER_ROW_SELECTOR_PREFIX, THREAD_PICKER_TRIGGER_SELECTOR,
    ThreadPickerAction,
};
use artisan_frontend::thread_list_selection::{
    EnterComposition, ListKey, ThreadListGroup, ThreadListSelection, ThreadRow,
};
use artisan_ui::theme::ThemeMode;
use gpui::{
    AppContext as _, Bounds, Context, Entity, FocusHandle, InteractiveElement as _, IntoElement,
    Modifiers, ParentElement as _, Pixels, Render, Styled as _, TestAppContext, VisualTestContext,
    Window, div, px, size,
};

const ROW_FIRST_SELECTOR: &str = "artisan-native-thread-picker-row-0";
const ROW_SECOND_SELECTOR: &str = "artisan-native-thread-picker-row-1";
const ROW_LAST_SELECTOR: &str = "artisan-native-thread-picker-row-255";

fn project(index: usize) -> ProjectId {
    ProjectId::parse(format!("picker-project-{index}")).expect("fixture project id")
}

fn thread_id(index: usize) -> ThreadId {
    ThreadId::parse(format!("picker-thread-{index}")).expect("fixture thread id")
}

fn summary(index: usize) -> ThreadSummary {
    ThreadSummary {
        thread_id: thread_id(index),
        project_id: project(index / 2),
        title: ThreadTitle::parse(format!("Thread {index:03}")).expect("fixture title"),
        created_at: UnixMillis::EPOCH,
        updated_at: UnixMillis::EPOCH,
    }
}

fn listing(indices: impl IntoIterator<Item = usize>) -> ThreadListing {
    ThreadListing::new(indices.into_iter().map(summary).collect()).expect("fixture listing")
}

fn painted_bounds(cx: &mut VisualTestContext, selector: &'static str) -> Bounds<Pixels> {
    cx.debug_bounds(selector)
        .unwrap_or_else(|| panic!("selector `{selector}` did not paint bounds"))
}

fn assert_visible_within(inner: Bounds<Pixels>, outer: Bounds<Pixels>, label: &str) {
    assert!(
        inner.origin.x >= outer.origin.x - px(0.6)
            && inner.right() <= outer.right() + px(0.6)
            && inner.origin.y >= outer.origin.y - px(0.6)
            && inner.bottom() <= outer.bottom() + px(0.6),
        "{label} must be visible inside the menu: {inner:?} vs {outer:?}"
    );
}

#[test]
fn mirror_preserves_authoritative_order_and_domain_bound() {
    let source = listing(0..256);
    let state = NativeThreadPickerState::new(source.clone(), Some(thread_id(128)));

    assert_eq!(state.row_count(), 256);
    assert_eq!(THREAD_PICKER_MAX_ROWS, 256);
    assert_eq!(state.rows(), source.threads());
    assert_eq!(state.rows()[0].thread_id, thread_id(0));
    assert_eq!(state.rows()[255].thread_id, thread_id(255));
    assert_eq!(state.selected_thread(), Some(&thread_id(128)));
    assert_eq!(state.trigger_label(), "Thread: Thread 128");

    let too_many = ThreadListing::new((0..257).map(summary).collect());
    assert!(
        too_many.is_err(),
        "the domain must enforce the 256-row bound"
    );
}

#[test]
fn empty_state_and_selectors_are_stable() {
    let state = NativeThreadPickerState::new(listing(std::iter::empty()), None);

    assert_eq!(state.row_count(), 0);
    assert_eq!(state.selected_thread(), None);
    assert_eq!(state.trigger_label(), "Thread: Choose a thread");
    assert!(!state.is_open());
    assert!(!state.is_disabled());
    assert_eq!(
        THREAD_PICKER_TRIGGER_SELECTOR,
        "artisan-native-thread-picker-trigger"
    );
    assert_eq!(
        THREAD_PICKER_MENU_SELECTOR,
        "artisan-native-thread-picker-menu"
    );
    assert_eq!(
        THREAD_PICKER_ROW_SELECTOR_PREFIX,
        "artisan-native-thread-picker-row"
    );
}

#[test]
fn refresh_preserves_exact_selection_and_clears_disappeared_selection() {
    let mut state = NativeThreadPickerState::new(listing(0..6), Some(thread_id(3)));
    state.press_trigger();
    assert_eq!(state.highlighted_thread(), Some(&thread_id(3)));

    state.replace_listing(listing([5, 3, 1]));
    assert_eq!(state.selected_thread(), Some(&thread_id(3)));
    assert_eq!(state.highlighted_index(), Some(1));

    state.replace_listing(listing([5, 1]));
    assert_eq!(state.selected_thread(), None);
    assert_eq!(state.highlighted_thread(), Some(&thread_id(5)));
}

#[test]
fn pointer_activation_and_same_thread_activation_are_distinct() {
    let mut state = NativeThreadPickerState::new(listing(0..3), Some(thread_id(0)));
    state.press_trigger();
    assert_eq!(
        state.activate_row(2),
        Some(ThreadPickerAction::OpenThread {
            thread_id: thread_id(2)
        })
    );
    let expected = Some(ThreadPickerAction::OpenThread {
        thread_id: thread_id(2),
    });
    assert_eq!(state.pending_action(), expected.as_ref());
    assert_eq!(state.take_pending_action(), expected);
    assert_eq!(state.take_pending_action(), None);

    let mut same = NativeThreadPickerState::new(listing(0..3), Some(thread_id(1)));
    same.press_trigger();
    assert_eq!(same.activate_row(1), None);
    assert_eq!(same.pending_action(), None);
    assert!(!same.is_open());
}

#[test]
fn keyboard_edges_groups_enter_and_ime_delegate_to_existing_selection() {
    let mut state = NativeThreadPickerState::new(listing(0..6), Some(thread_id(1)));
    state.press_trigger();

    state.handle_key(ListKey::ArrowUp);
    assert_eq!(state.highlighted_index(), Some(0));
    state.handle_key(ListKey::ArrowDown);
    assert_eq!(state.highlighted_index(), Some(1));
    state.handle_key(ListKey::AltArrowDown);
    assert_eq!(state.highlighted_index(), Some(2));
    state.handle_key(ListKey::AltArrowUp);
    assert_eq!(state.highlighted_index(), Some(0));
    state.handle_key(ListKey::MetaArrowDown);
    assert_eq!(state.highlighted_index(), Some(5));
    state.handle_key(ListKey::MetaArrowUp);
    assert_eq!(state.highlighted_index(), Some(0));
    state.handle_key(ListKey::End);
    assert_eq!(state.highlighted_index(), Some(5));
    state.handle_key(ListKey::Home);
    assert_eq!(state.highlighted_index(), Some(0));

    assert_eq!(
        state.handle_key(ListKey::Enter(EnterComposition::Composing)),
        None
    );
    assert_eq!(
        state.handle_key(ListKey::Enter(EnterComposition::KeyCode229)),
        None
    );
    assert!(state.is_open(), "IME suppression must not close the menu");

    assert_eq!(
        state.handle_key(ListKey::Enter(EnterComposition::Clear)),
        Some(ThreadPickerAction::OpenThread {
            thread_id: thread_id(0)
        })
    );
    assert!(!state.is_open());
}

#[test]
fn disabled_rows_are_skipped_and_disabled_picker_drops_actions() {
    let groups = vec![ThreadListGroup::new(
        Some(project(0)),
        vec![
            ThreadRow::disabled(thread_id(0)),
            ThreadRow::enabled(thread_id(1)),
            ThreadRow::disabled(thread_id(2)),
        ],
    )];
    let mut selection = ThreadListSelection::new();
    selection.mount(&groups);
    assert_eq!(selection.selected_thread(&groups), Some(&thread_id(1)));
    selection.handle_key(&groups, ListKey::ArrowDown);
    assert_eq!(selection.selected_thread(&groups), Some(&thread_id(1)));

    let mut state = NativeThreadPickerState::new(listing(0..3), Some(thread_id(0)));
    state.press_trigger();
    state.set_disabled(true);
    assert!(!state.is_open());
    assert_eq!(state.pending_action(), None);
    state.press_trigger();
    assert!(!state.is_open());
}

struct PickerHost {
    picker: Entity<NativeThreadPicker>,
    lead_focus: FocusHandle,
}

impl PickerHost {
    fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        listing: ThreadListing,
        selected_thread: Option<ThreadId>,
    ) -> Self {
        let picker = cx.new(|picker_cx| {
            NativeThreadPicker::new(listing, selected_thread, ThemeMode::Dark, picker_cx)
        });
        let lead_focus = cx.focus_handle().tab_index(0).tab_stop(true);
        lead_focus.focus(window);
        Self { picker, lead_focus }
    }
}

impl Render for PickerHost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .p(px(12.0))
            .child(div().track_focus(&self.lead_focus).size(px(1.0)))
            .child(self.picker.clone())
    }
}

#[gpui::test]
fn real_gpui_surface_scrolls_256_rows_and_restores_focus_after_pointer_activation(
    cx: &mut TestAppContext,
) {
    let (host, cx) = cx.add_window_view(|window, view_cx| {
        PickerHost::new(window, view_cx, listing(0..256), Some(thread_id(255)))
    });
    cx.simulate_resize(size(px(800.0), px(600.0)));
    cx.run_until_parked();

    cx.update(|window, app| {
        host.read(app)
            .picker
            .read(app)
            .trigger_focus()
            .focus(window);
    });
    cx.run_until_parked();

    let trigger = painted_bounds(cx, THREAD_PICKER_TRIGGER_SELECTOR);
    cx.simulate_click(trigger.center(), Modifiers::none());
    cx.run_until_parked();

    let menu = painted_bounds(cx, THREAD_PICKER_MENU_SELECTOR);
    assert!(
        menu.size.height <= px(360.0) + px(0.6),
        "the menu must remain height-bounded: {menu:?}"
    );
    let last_row = painted_bounds(cx, ROW_LAST_SELECTOR);
    assert_visible_within(last_row, menu, "the selected last row");
    cx.update(|window, app| {
        let picker = host.read(app).picker.clone();
        assert!(
            picker.read(app).menu_focus().is_focused(window),
            "opening must move focus into the menu"
        );
    });

    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    cx.update(|window, app| {
        let picker = host.read(app).picker.clone();
        assert!(!picker.read(app).state().is_open());
        assert!(
            picker.read(app).trigger_focus().is_focused(window),
            "Escape must restore focus to the trigger"
        );
    });

    let trigger = painted_bounds(cx, THREAD_PICKER_TRIGGER_SELECTOR);
    cx.simulate_click(trigger.center(), Modifiers::none());
    cx.run_until_parked();

    cx.simulate_keystrokes("home");
    cx.run_until_parked();
    let menu = painted_bounds(cx, THREAD_PICKER_MENU_SELECTOR);
    let first_row = painted_bounds(cx, ROW_FIRST_SELECTOR);
    assert_visible_within(first_row, menu, "the first row after Home");
    let second_row = painted_bounds(cx, ROW_SECOND_SELECTOR);
    assert_visible_within(second_row, menu, "the pointer target row");
    cx.simulate_click(second_row.center(), Modifiers::none());
    cx.run_until_parked();

    cx.update(|window, app| {
        let picker = host.read(app).picker.clone();
        let action = picker.update(app, |picker, _| picker.take_pending_action());
        assert_eq!(
            action,
            Some(ThreadPickerAction::OpenThread {
                thread_id: thread_id(1)
            })
        );
        assert!(!picker.read(app).state().is_open());
        assert!(
            picker.read(app).trigger_focus().is_focused(window),
            "closing must restore focus to the trigger"
        );
    });
}

#[test]
fn application_source_keeps_switch_order_and_generation_fences() {
    let source = include_str!("../../modules/frontend/src/native_application.rs");

    assert!(source.contains("thread_listing: Option<ThreadListing>"));
    assert!(source.contains("pending_thread: Option<ThreadId>"));
    assert!(source.contains("struct ThreadSwitchFlight"));
    assert!(source.contains("AwaitingUnsubscribeStop"));
    assert!(source.contains("HostRetirement"));
    assert!(source.contains("AwaitingSubscriptionStart"));
    assert!(source.contains("retained_switch_patch_ids"));
    assert!(source.contains("retained_switch_listings"));

    let mount_start = source
        .find("fn try_mount_pending_thread")
        .expect("find pending-thread mount path");
    let mount = &source[mount_start..];
    let switch_branch = mount
        .rfind("if switch_generation.is_some()")
        .and_then(|start| {
            mount[start..]
                .find("} else {")
                .map(|end| &mount[start..start + end])
        })
        .expect("find switch mount branch");
    assert!(switch_branch.contains("discard_initial_snapshot_request"));
    assert!(switch_branch.contains("submit_thread_switch_subscribe"));
    assert!(!switch_branch.contains("NativeTransportCommand::RequestSnapshot"));

    let unsubscribe = source
        .find("NativeTransportCommand::Unsubscribe")
        .expect("find source unsubscribe");
    let subscribe = source
        .find("NativeTransportCommand::Subscribe")
        .expect("find target subscribe");
    assert!(
        unsubscribe < subscribe,
        "unsubscribe must precede subscribe"
    );
    assert!(
        source.contains("after: None"),
        "switch subscribe must be fresh"
    );
}
