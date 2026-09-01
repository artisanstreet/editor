//! Native GPUI presentation for the bounded project thread picker.
//!
//! The picker is intentionally a mirror of application-owned state. It keeps
//! only the rows and interaction state needed to paint the menu; the mounted
//! project, mounted thread, and transport transition remain owned by
//! [`crate::native_application::NativeApplication`]. Keyboard selection is
//! delegated to [`crate::thread_list_selection::ThreadListSelection`] so the
//! native surface and the audited command-list state machine cannot drift.

#![forbid(unsafe_code)]
#![allow(clippy::module_name_repetitions)]

use std::{cell::RefCell, mem, rc::Rc};

use artisan_domain::{THREAD_LISTING_MAX_THREADS, ThreadId, ThreadListing};
use artisan_ui::list_row::{
    ListRowContent, ListRowGeometry, ListRowSlots, ListRowStyle, ListRowTone, list_row,
};
use artisan_ui::theme::{ArtisanTheme, ThemeMode};
use gpui::{
    AnyElement, ClickEvent, Context, Corner, Div, FocusHandle, FontWeight, InteractiveElement as _,
    KeyDownEvent, MouseDownEvent, ParentElement as _, Pixels, Point, Render, ScrollHandle,
    SharedString, Size, Stateful, StatefulInteractiveElement as _, Styled as _, Window, anchored,
    canvas, deferred, div, point, prelude::FluentBuilder as _, prelude::IntoElement, px,
};

use crate::thread_list_selection::{
    EnterComposition, ListKey, ThreadActivationIntent, ThreadListGroup, ThreadListSelection,
    ThreadRow,
};

/// Stable selector painted on the thread-picker trigger.
pub const THREAD_PICKER_TRIGGER_SELECTOR: &str = "artisan-native-thread-picker-trigger";

/// Stable selector painted on the open thread-picker menu.
pub const THREAD_PICKER_MENU_SELECTOR: &str = "artisan-native-thread-picker-menu";

/// Prefix for the stable selectors painted on thread rows.
pub const THREAD_PICKER_ROW_SELECTOR_PREFIX: &str = "artisan-native-thread-picker-row";

/// Maximum number of rows accepted by the authoritative domain listing.
pub const THREAD_PICKER_MAX_ROWS: usize = THREAD_LISTING_MAX_THREADS;

const EMPTY_THREAD_LABEL: &str = "Choose a thread";
const NO_THREADS_LABEL: &str = "No threads";
const THREAD_PICKER_KEY_CONTEXT: &str = "artisan-native-thread-picker";
const TRIGGER_WIDTH_PX: f32 = 300.0;
const MENU_WIDTH_PX: f32 = 360.0;
const MENU_VIEWPORT_INSET_X_PX: f32 = 32.0;
const MENU_MAX_HEIGHT_PX: f32 = 360.0;
const MENU_VIEWPORT_INSET_Y_PX: f32 = 32.0;
const MENU_GAP_PX: f32 = 10.0;

/// One action emitted after a listed thread row has been activated.
///
/// The application consumes this action and owns all validation and transport
/// choreography. The picker never routes a thread or mutates a host.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ThreadPickerAction {
    /// Request that the application mount the listed thread.
    OpenThread { thread_id: ThreadId },
}

/// Explicit presentation state for [`NativeThreadPicker`].
///
/// `listing` is replaced wholesale from the authoritative domain value. The
/// derived groups contain no new data and preserve source order, while the
/// selection object remains the existing audited navigation state machine.
#[derive(Debug)]
pub struct NativeThreadPickerState {
    listing: ThreadListing,
    groups: Vec<ThreadListGroup>,
    selection: ThreadListSelection,
    selected_thread: Option<ThreadId>,
    open: bool,
    disabled: bool,
    pending_action: Option<ThreadPickerAction>,
}

impl NativeThreadPickerState {
    /// Builds a closed picker over one bounded listing.
    #[must_use]
    pub fn new(listing: ThreadListing, selected_thread: Option<ThreadId>) -> Self {
        let groups = groups_from_listing(&listing);
        let mut selection = ThreadListSelection::new();
        selection.mount(&groups);
        let selected_thread = selected_thread.filter(|id| contains_thread(&listing, id));
        sync_selection(&mut selection, &groups, selected_thread.as_ref());
        Self {
            listing,
            groups,
            selection,
            selected_thread,
            open: false,
            disabled: false,
            pending_action: None,
        }
    }

    /// Replaces the bounded listing while preserving an exact selected row
    /// whenever its [`ThreadId`] still exists.
    pub fn replace_listing(&mut self, listing: ThreadListing) {
        self.listing = listing;
        self.groups = groups_from_listing(&self.listing);
        self.selection.resync(&self.groups);
        let selected_thread = self.selected_thread.take();
        self.selected_thread = selected_thread.filter(|id| contains_thread(&self.listing, id));
        sync_selection(
            &mut self.selection,
            &self.groups,
            self.selected_thread.as_ref(),
        );
        if let Some(ThreadPickerAction::OpenThread { thread_id }) = &self.pending_action
            && !contains_thread(&self.listing, thread_id)
        {
            self.pending_action = None;
        }
    }

    /// Repoints the mirror at the application's mounted thread.
    ///
    /// A thread absent from the current listing cannot be presented as
    /// selected; it is represented as no selected row until the application
    /// completes its fenced retirement.
    pub fn set_selected_thread(&mut self, selected_thread: Option<ThreadId>) {
        self.selected_thread = selected_thread.filter(|id| contains_thread(&self.listing, id));
        sync_selection(
            &mut self.selection,
            &self.groups,
            self.selected_thread.as_ref(),
        );
    }

    /// Sets the trigger/menu disabled state. Disabling closes the menu and
    /// discards the one action that has not yet been consumed.
    pub fn set_disabled(&mut self, disabled: bool) {
        self.disabled = disabled;
        if disabled {
            self.close_without_action();
            self.pending_action = None;
        }
    }

    /// Returns the authoritative bounded listing mirrored by this surface.
    #[must_use]
    pub fn listing(&self) -> &ThreadListing {
        &self.listing
    }

    /// Returns the rows in exact authoritative source order.
    #[must_use]
    pub fn rows(&self) -> &[artisan_domain::ThreadSummary] {
        self.listing.threads()
    }

    /// Returns the application's selected thread, if it is present in the
    /// current listing.
    #[must_use]
    pub fn selected_thread(&self) -> Option<&ThreadId> {
        self.selected_thread.as_ref()
    }

    /// Returns the row currently highlighted by the delegated selection
    /// machine while the menu is open.
    #[must_use]
    pub fn highlighted_thread(&self) -> Option<&ThreadId> {
        self.open
            .then(|| self.selection.selected_thread(&self.groups))
            .flatten()
    }

    /// Returns the highlighted row's source-order address while open.
    #[must_use]
    pub fn highlighted_index(&self) -> Option<usize> {
        let highlighted = self.highlighted_thread()?;
        self.rows()
            .iter()
            .position(|summary| &summary.thread_id == highlighted)
    }

    /// Returns whether the menu is open.
    #[must_use]
    pub const fn is_open(&self) -> bool {
        self.open
    }

    /// Returns whether the trigger refuses interaction.
    #[must_use]
    pub const fn is_disabled(&self) -> bool {
        self.disabled
    }

    /// Returns the count of bounded rows.
    #[must_use]
    pub fn row_count(&self) -> usize {
        self.rows().len()
    }

    /// Returns the visible trigger label derived from the selected summary.
    #[must_use]
    pub fn trigger_label(&self) -> String {
        self.selected_thread
            .as_ref()
            .and_then(|selected| {
                self.rows()
                    .iter()
                    .find(|summary| &summary.thread_id == selected)
            })
            .map_or_else(
                || format!("Thread: {EMPTY_THREAD_LABEL}"),
                |summary| format!("Thread: {}", summary.title.as_str()),
            )
    }

    /// Toggles the menu unless disabled. Opening preserves the exact
    /// application-selected row when it remains listed.
    pub fn press_trigger(&mut self) {
        if self.disabled || self.pending_action.is_some() {
            return;
        }
        if self.open {
            self.close_without_action();
        } else {
            sync_selection(
                &mut self.selection,
                &self.groups,
                self.selected_thread.as_ref(),
            );
            self.open = true;
        }
    }

    /// Dismisses the open menu without emitting an action.
    pub fn dismiss(&mut self) {
        self.close_without_action();
    }

    /// Delegates a keyboard gesture to the existing selection state machine.
    ///
    /// Enter is converted from the machine's activation intent into the sole
    /// frozen picker action. Same-thread activation still closes the menu but
    /// does not enqueue an action.
    pub fn handle_key(&mut self, key: ListKey) -> Option<ThreadPickerAction> {
        if !self.open || self.disabled || self.pending_action.is_some() {
            return None;
        }
        let intent = self.selection.handle_key(&self.groups, key);
        let Some(ThreadActivationIntent::OpenThread { thread_id }) = intent else {
            return None;
        };

        self.close_without_action();
        if self.selected_thread.as_ref() == Some(&thread_id) {
            return None;
        }
        let action = ThreadPickerAction::OpenThread { thread_id };
        if self.pending_action.is_none() {
            self.pending_action = Some(action.clone());
        }
        Some(action)
    }

    /// Activates one source-order row from a pointer press.
    pub fn activate_row(&mut self, index: usize) -> Option<ThreadPickerAction> {
        if !self.open || self.disabled || self.pending_action.is_some() {
            return None;
        }
        let thread_id = self.rows().get(index)?.thread_id.clone();
        self.close_without_action();
        if self.selected_thread.as_ref() == Some(&thread_id) {
            return None;
        }
        let action = ThreadPickerAction::OpenThread { thread_id };
        if self.pending_action.is_none() {
            self.pending_action = Some(action.clone());
        }
        Some(action)
    }

    /// Returns and clears the one action waiting for application observation.
    pub fn take_pending_action(&mut self) -> Option<ThreadPickerAction> {
        self.pending_action.take()
    }

    /// Returns the pending action without consuming it.
    #[must_use]
    pub fn pending_action(&self) -> Option<&ThreadPickerAction> {
        self.pending_action.as_ref()
    }

    fn close_without_action(&mut self) {
        self.open = false;
    }
}

/// A real GPUI trigger and deferred, bounded menu for [`ThreadListing`].
pub struct NativeThreadPicker {
    state: NativeThreadPickerState,
    theme: ArtisanTheme,
    trigger_focus: FocusHandle,
    menu_focus: FocusHandle,
    menu_scroll: ScrollHandle,
    trigger_origin: Rc<RefCell<Option<Point<Pixels>>>>,
    initial_reveal_flat: Rc<RefCell<Option<usize>>>,
    suppress_trigger_release: bool,
}

impl NativeThreadPicker {
    /// Builds the picker over one authoritative listing and selected thread.
    pub fn new(
        listing: ThreadListing,
        selected_thread: Option<ThreadId>,
        mode: ThemeMode,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            state: NativeThreadPickerState::new(listing, selected_thread),
            theme: ArtisanTheme::for_mode(mode),
            trigger_focus: cx.focus_handle().tab_index(0).tab_stop(true),
            menu_focus: cx.focus_handle(),
            menu_scroll: ScrollHandle::new(),
            trigger_origin: Rc::new(RefCell::new(None)),
            initial_reveal_flat: Rc::new(RefCell::new(None)),
            suppress_trigger_release: false,
        }
    }

    /// Read-only access to the presentation state.
    #[must_use]
    pub fn state(&self) -> &NativeThreadPickerState {
        &self.state
    }

    /// The tab stop used by the closed trigger and focus restoration.
    #[must_use]
    pub fn trigger_focus(&self) -> &FocusHandle {
        &self.trigger_focus
    }

    /// The focus target used while the menu is open.
    #[must_use]
    pub fn menu_focus(&self) -> &FocusHandle {
        &self.menu_focus
    }

    /// Replaces the mirrored listing.
    pub fn replace_listing(&mut self, listing: ThreadListing, cx: &mut Context<Self>) {
        self.state.replace_listing(listing);
        self.initial_reveal_flat.borrow_mut().take();
        cx.notify();
    }

    /// Synchronizes the mirrored selected thread.
    pub fn set_selected_thread(
        &mut self,
        selected_thread: Option<ThreadId>,
        cx: &mut Context<Self>,
    ) {
        self.state.set_selected_thread(selected_thread);
        cx.notify();
    }

    /// Sets the disabled state and closes/discards pending picker work when
    /// disabled.
    pub fn set_disabled(&mut self, disabled: bool, cx: &mut Context<Self>) {
        self.state.set_disabled(disabled);
        if disabled {
            self.initial_reveal_flat.borrow_mut().take();
        }
        cx.notify();
    }

    /// Consumes the one pending picker action.
    pub fn take_pending_action(&mut self) -> Option<ThreadPickerAction> {
        self.state.take_pending_action()
    }

    fn handle_trigger_click(
        &mut self,
        event: &ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if matches!(event, ClickEvent::Keyboard(_)) && self.suppress_trigger_release {
            self.suppress_trigger_release = false;
            return;
        }
        self.suppress_trigger_release = false;
        self.state.press_trigger();
        self.sync_focus_after_transition(window);
        cx.notify();
    }

    fn disarm_stale_release_fence(
        &mut self,
        event: &KeyDownEvent,
        _: &mut Window,
        _: &mut Context<Self>,
    ) {
        if !event.is_held {
            self.suppress_trigger_release = false;
        }
    }

    fn handle_menu_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.keystroke.key == "escape" {
            self.state.dismiss();
            self.initial_reveal_flat.borrow_mut().take();
            window.focus(&self.trigger_focus);
            cx.notify();
            return;
        }
        let Some(key) = list_key_for_keystroke(event) else {
            return;
        };
        let was_open = self.state.is_open();
        self.state.handle_key(key);
        if matches!(key, ListKey::Enter(_)) && was_open {
            if !self.state.is_open() {
                self.suppress_trigger_release = true;
            }
            self.sync_focus_after_transition(window);
        } else {
            self.reveal_highlight();
        }
        cx.notify();
    }

    fn handle_outside_press(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.state.is_open() || self.menu_scroll.bounds().contains(&event.position) {
            return;
        }
        self.state.dismiss();
        self.initial_reveal_flat.borrow_mut().take();
        window.focus(&self.trigger_focus);
        cx.notify();
    }

    fn choose_row(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.state.activate_row(index);
        self.sync_focus_after_transition(window);
        cx.notify();
    }

    fn sync_focus_after_transition(&mut self, window: &mut Window) {
        if self.state.is_open() {
            window.focus(&self.menu_focus);
            let flat = self.state.highlighted_index().unwrap_or(0);
            *self.initial_reveal_flat.borrow_mut() = Some(flat);
            self.reveal_highlight();
        } else {
            self.initial_reveal_flat.borrow_mut().take();
            window.focus(&self.trigger_focus);
        }
    }

    fn reveal_highlight(&mut self) {
        if let Some(index) = self.state.highlighted_index() {
            self.menu_scroll.scroll_to_item(index);
        }
    }

    fn render_trigger(&self, cx: &Context<Self>) -> Stateful<Div> {
        let foreground = self.theme.colors.foreground.to_paint();
        let muted_foreground = self.theme.colors.muted_foreground.to_paint();
        let surface = self.theme.colors.muted.to_paint();
        let disabled = self.state.is_disabled();
        self.trigger_focus.clone().tab_stop(!disabled);

        div()
            .id("native-thread-picker-trigger")
            .track_focus(&self.trigger_focus)
            .key_context(THREAD_PICKER_KEY_CONTEXT)
            .debug_selector(|| THREAD_PICKER_TRIGGER_SELECTOR.to_string())
            .on_key_down(cx.listener(Self::disarm_stale_release_fence))
            .when(!disabled, |row| {
                row.on_click(cx.listener(Self::handle_trigger_click))
            })
            .flex()
            .items_center()
            .gap(px(10.0))
            .w(px(TRIGGER_WIDTH_PX))
            .px(px(8.0))
            .py(px(8.0))
            .rounded(px(8.0))
            .bg(surface)
            .when(disabled, |row| row.opacity(0.5))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(px(14.0))
                    .text_color(foreground)
                    .child(self.state.trigger_label()),
            )
            .child(
                div()
                    .text_size(px(14.0))
                    .text_color(muted_foreground)
                    .child("▾"),
            )
    }

    fn render_menu(&self, viewport: Size<Pixels>, cx: &Context<Self>) -> Option<AnyElement> {
        if !self.state.is_open() {
            return None;
        }
        let trigger_origin = self.trigger_origin.borrow().as_ref().copied()?;
        let popover = self.theme.colors.popover.to_paint();
        let border = self.theme.colors.border.to_paint();
        let mut body = div()
            .id("native-thread-picker-menu")
            .track_focus(&self.menu_focus)
            .key_context(THREAD_PICKER_KEY_CONTEXT)
            .debug_selector(|| THREAD_PICKER_MENU_SELECTOR.to_string())
            .on_key_down(cx.listener(Self::handle_menu_key))
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .track_scroll(&self.menu_scroll)
            .w(menu_width_for_viewport(viewport))
            .max_h(menu_max_height_for_viewport(viewport))
            .p(px(4.0))
            .rounded(px(16.0))
            .bg(popover)
            .border_1()
            .border_color(border);

        if self.state.rows().is_empty() {
            body = body.child(
                div()
                    .id("native-thread-picker-empty")
                    .debug_selector(|| "artisan-native-thread-picker-empty".to_string())
                    .p(px(8.0))
                    .text_sm()
                    .text_color(self.theme.colors.muted_foreground.to_paint())
                    .child(NO_THREADS_LABEL),
            );
        } else {
            for index in 0..self.state.rows().len() {
                body = body.child(self.render_thread_row(index, cx));
            }
        }

        Some(
            anchored()
                .anchor(Corner::BottomLeft)
                .position(trigger_origin)
                .offset(point(px(0.0), px(-MENU_GAP_PX)))
                .child(body)
                .into_any_element(),
        )
    }

    fn render_thread_row(&self, index: usize, cx: &Context<Self>) -> Stateful<Div> {
        let summary = &self.state.rows()[index];
        let selected = self
            .state
            .selected_thread()
            .is_some_and(|thread_id| thread_id == &summary.thread_id);
        let highlighted = self
            .state
            .highlighted_thread()
            .is_some_and(|thread_id| thread_id == &summary.thread_id);
        let style = ListRowStyle::resolve(
            self.theme,
            ListRowGeometry::Menu,
            ListRowTone::Foreground,
            FontWeight::NORMAL,
        );
        let selector = format!("{THREAD_PICKER_ROW_SELECTOR_PREFIX}-{index}");
        let mut slots = ListRowSlots::new();
        if selected {
            slots = slots.trailing(
                div()
                    .text_size(px(16.0))
                    .line_height(style.title_line_height)
                    .text_color(self.theme.colors.muted_foreground.to_paint())
                    .child("✓"),
            );
        }
        let presentation = list_row(
            style,
            ListRowContent::one_line(SharedString::from(summary.title.as_str().to_owned())),
            slots,
        );
        let disabled = self.state.is_disabled();
        let row_id = SharedString::from(format!("native-thread-picker-row-{index}"));
        let mut row = presentation
            .id(row_id)
            .debug_selector(move || selector.clone())
            .when(highlighted, |row| {
                row.bg(self.theme.colors.accent.to_paint())
            })
            .when(disabled, |row| row.opacity(0.5));
        if !disabled {
            row = row.on_click(cx.listener(move |picker, _: &ClickEvent, window, cx| {
                picker.choose_row(index, window, cx);
            }));
        }
        row
    }
}

impl Render for NativeThreadPicker {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let viewport = window.viewport_size();
        let menu = self.render_menu(viewport, cx);
        let trigger = self.render_trigger(cx);
        let probe_origin = Rc::clone(&self.trigger_origin);
        let probe_reveal = Rc::clone(&self.initial_reveal_flat);
        let probe_scroll = self.menu_scroll.clone();
        let probe = canvas(
            move |_, _, _| {},
            move |bounds, (), window, cx| {
                let moved = *probe_origin.borrow_mut() != Some(bounds.origin);
                *probe_origin.borrow_mut() = Some(bounds.origin);
                if let Some(flat) = probe_reveal.borrow_mut().take() {
                    let scroll = probe_scroll.clone();
                    window.defer(cx, move |window, _| {
                        scroll.scroll_to_item(flat);
                        window.refresh();
                    });
                } else if moved {
                    window.defer(cx, |window, _| window.refresh());
                }
            },
        )
        .absolute()
        .size_full();

        div()
            .id("native-thread-picker-root")
            .tab_group()
            .on_mouse_down_out(cx.listener(Self::handle_outside_press))
            .flex()
            .flex_col()
            .child(
                div()
                    .relative()
                    .flex()
                    .flex_col()
                    .child(probe)
                    .children(menu.map(deferred))
                    .child(trigger),
            )
    }
}

/// Derives contiguous project groups without reordering or ranking rows.
fn groups_from_listing(listing: &ThreadListing) -> Vec<ThreadListGroup> {
    let mut groups = Vec::new();
    let mut project_id = None;
    let mut rows = Vec::new();
    for summary in listing.threads() {
        if !rows.is_empty() && project_id.as_ref() != Some(&summary.project_id) {
            groups.push(ThreadListGroup::new(
                project_id.take(),
                mem::take(&mut rows),
            ));
        }
        if rows.is_empty() {
            project_id = Some(summary.project_id.clone());
        }
        rows.push(ThreadRow::enabled(summary.thread_id.clone()));
    }
    if !rows.is_empty() {
        groups.push(ThreadListGroup::new(project_id, rows));
    }
    groups
}

/// Synchronizes the delegated selection to an exact application-selected id
/// using only its public navigation operations.
fn sync_selection(
    selection: &mut ThreadListSelection,
    groups: &[ThreadListGroup],
    selected_thread: Option<&ThreadId>,
) {
    if let Some(selected_thread) = selected_thread {
        if selection.selected_thread(groups) == Some(selected_thread) {
            return;
        }
        let _ = selection.handle_key(groups, ListKey::Home);
        let maximum_steps = groups.iter().map(|group| group.rows().len()).sum::<usize>();
        for _ in 0..maximum_steps {
            if selection.selected_thread(groups) == Some(selected_thread) {
                break;
            }
            let _ = selection.handle_key(groups, ListKey::ArrowDown);
        }
    } else {
        selection.resync(groups);
    }
}

fn contains_thread(listing: &ThreadListing, thread_id: &ThreadId) -> bool {
    listing
        .threads()
        .iter()
        .any(|summary| &summary.thread_id == thread_id)
}

fn list_key_for_keystroke(event: &KeyDownEvent) -> Option<ListKey> {
    let keystroke = &event.keystroke;
    match keystroke.key.as_str() {
        "down" if keystroke.modifiers.platform => Some(ListKey::MetaArrowDown),
        "up" if keystroke.modifiers.platform => Some(ListKey::MetaArrowUp),
        "down" if keystroke.modifiers.alt => Some(ListKey::AltArrowDown),
        "up" if keystroke.modifiers.alt => Some(ListKey::AltArrowUp),
        "down" => Some(ListKey::ArrowDown),
        "up" => Some(ListKey::ArrowUp),
        "home" => Some(ListKey::Home),
        "end" => Some(ListKey::End),
        "229" => Some(ListKey::Enter(EnterComposition::KeyCode229)),
        "ime" => Some(ListKey::Enter(EnterComposition::Composing)),
        "enter" | "space" => Some(ListKey::Enter(EnterComposition::Clear)),
        _ => None,
    }
}

fn menu_width_for_viewport(viewport: Size<Pixels>) -> Pixels {
    let available = f32::from(viewport.width) - MENU_VIEWPORT_INSET_X_PX;
    px(MENU_WIDTH_PX.min(available.max(0.0)))
}

fn menu_max_height_for_viewport(viewport: Size<Pixels>) -> Pixels {
    let available = f32::from(viewport.height) - MENU_VIEWPORT_INSET_Y_PX;
    px(MENU_MAX_HEIGHT_PX.min(available.max(0.0)))
}
