//! GPUI render frame, interaction wiring, and debug selectors for `Select`.
//!
//! Split out of `select.rs`; the controlled state machine lives in `state`.

#![forbid(unsafe_code)]

use std::cell::RefCell;
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use artisan_assets::AssetId;
use gpui::{
    AnchoredPositionMode, App, ClickEvent, Div, ElementId, FocusHandle, InteractiveElement,
    IntoElement, KeyDownEvent, MouseDownEvent, ParentElement, Pixels, RenderOnce, ScrollHandle,
    SharedString, Stateful, StatefulInteractiveElement, Styled, Window, anchored, deferred, div,
    point, px, transparent_black,
};

use crate::icon::{IconSize, IconStyle, IconTint, icon};
use crate::theme::ArtisanTheme;

use super::state::{
    SelectAction, SelectEntry, SelectItem, SelectKey, SelectScrollState, SelectState, SelectValue,
    entry_enabled_flags,
};
use super::{DISABLED_OPACITY, Select, SelectChangeHandler, SelectOpenChangeHandler, SelectStyle};
/// Borrowed inputs for one rendered select row.
struct RenderItemArgs<'a, V: SelectValue> {
    theme: &'a ArtisanTheme,
    item: &'a SelectItem<V>,
    index: usize,
    content_id: &'a ElementId,
    selector_root: &'a str,
    selected: Option<&'a V>,
    highlighted: Option<usize>,
    style: &'a SelectStyle,
}

fn render_item<V: SelectValue>(args: &RenderItemArgs<'_, V>) -> Stateful<Div> {
    let &RenderItemArgs {
        theme,
        item,
        index,
        content_id,
        selector_root,
        selected,
        highlighted,
        style,
    } = args;
    let item_selected = selected.is_some_and(|selected| item.value() == selected);
    let item_highlighted = highlighted == Some(index);
    let item_selector = item_debug_selector(selector_root, item.value());
    let item_id =
        ElementId::NamedChild(Arc::new(content_id.clone()), format!("item-{index}").into());

    let mut row = div()
        .id(item_id)
        .relative()
        .flex()
        .flex_row()
        .items_center()
        .w_full()
        .min_w_0()
        .gap(style.item_gap)
        .px(style.item_horizontal_padding)
        .pr(px(32.0))
        .py(style.item_vertical_padding)
        .rounded(style.item_corner_radius)
        .bg(if item_highlighted {
            style.item_highlight_background
        } else {
            style.item_background
        })
        .text_color(if item_highlighted {
            style.item_highlight_foreground
        } else {
            style.item_foreground
        })
        .text_size(style.text_size)
        .debug_selector(move || item_selector.clone())
        .child(
            div()
                .min_w_0()
                .flex_1()
                .truncate()
                .whitespace_nowrap()
                .child(item.label.clone()),
        );

    if item_selected {
        row = row.child(
            div()
                .absolute()
                .right(px(8.0))
                .top(px(8.0))
                .size(px(14.0))
                .flex()
                .items_center()
                .justify_center()
                .child(icon(IconStyle::resolve(
                    *theme,
                    AssetId::TABLER_CHECK,
                    IconSize::Compact,
                    IconTint::Inherit,
                ))),
        );
    }

    if item.is_disabled() {
        row = row.opacity(style.disabled_opacity);
    } else {
        let hover_background = style.item_highlight_background;
        let hover_foreground = style.item_highlight_foreground;
        row = row.hover(move |hovered| hovered.bg(hover_background).text_color(hover_foreground));
    }

    row
}

fn scroll_button(
    theme: &ArtisanTheme,
    height: Pixels,
    asset: AssetId,
    selector: String,
    on_click: impl Fn(&mut Window) + 'static,
) -> Stateful<Div> {
    let debug_selector = selector.clone();

    div()
        .id(SharedString::from(selector))
        .flex()
        .items_center()
        .justify_center()
        .w_full()
        .h(height)
        .bg(transparent_black())
        .debug_selector(move || debug_selector.clone())
        .on_click(move |_, window, _| on_click(window))
        .child(icon(IconStyle::resolve(
            *theme,
            asset,
            IconSize::Default,
            IconTint::Muted,
        )))
}

pub(super) fn selected_index<V: SelectValue>(
    entries: &[SelectEntry<V>],
    selected: Option<&V>,
) -> Option<usize> {
    selected.and_then(|selected| {
        entries
            .iter()
            .position(|entry| matches!(entry, SelectEntry::Item(item) if item.value() == selected))
    })
}

/// Borrowed delivery inputs for one applied select action.
struct ActionDelivery<'a, V: SelectValue> {
    action: SelectAction,
    state: &'a Rc<RefCell<SelectState>>,
    entries: &'a [SelectEntry<V>],
    on_change: Option<&'a SelectChangeHandler<V>>,
    on_open_change: Option<&'a SelectOpenChangeHandler>,
    focus: &'a FocusHandle,
    scroll_handle: &'a ScrollHandle,
    enabled: &'a [bool],
    was_open: bool,
    event: Option<&'a ClickEvent>,
    window: &'a mut Window,
    cx: &'a mut App,
}

fn notify_open_change(
    handler: Option<&SelectOpenChangeHandler>,
    open: bool,
    window: &mut Window,
    cx: &mut App,
) {
    if let Some(handler) = handler {
        handler(open, window, cx);
    }
}

fn apply_action<V: SelectValue>(delivery: ActionDelivery<'_, V>) {
    let ActionDelivery {
        action,
        state,
        entries,
        on_change,
        on_open_change,
        focus,
        scroll_handle,
        enabled,
        was_open,
        event,
        window,
        cx,
    } = delivery;
    match action {
        SelectAction::None => {}
        SelectAction::Open => {
            if let Some(index) = state.borrow().highlighted_index() {
                scroll_handle.scroll_to_item(index);
            }

            notify_open_change(on_open_change, true, window, cx);
        }
        SelectAction::Close => {
            window.focus(focus, cx);
            notify_open_change(on_open_change, false, window, cx);
        }
        SelectAction::Highlight(index) => {
            if enabled.get(index).copied().unwrap_or(false) {
                scroll_handle.scroll_to_item(index);
                window.refresh();
            }
        }
        SelectAction::Commit(index) => {
            if was_open {
                window.focus(focus, cx);
                notify_open_change(on_open_change, false, window, cx);
            }

            if let Some(SelectEntry::Item(item)) = entries.get(index)
                && enabled.get(index).copied().unwrap_or(false)
                && let Some(handler) = on_change
            {
                handler(item.value().clone(), event, window, cx);
            }
        }
    }
}
fn monotonic_now() -> Duration {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    EPOCH.get_or_init(Instant::now).elapsed()
}

struct SelectFnv1aHasher(u64);

impl Hasher for SelectFnv1aHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 ^= u64::from(*byte);
            self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
        }
    }
}

/// Owned render inputs shared by the select frame builders.
///
/// Cloning here is limited to `Rc` handles, small `Copy` values, and the
/// already-resolved style record; no caller data is duplicated by value.
struct SelectFrame<V: SelectValue> {
    id: ElementId,
    focus: FocusHandle,
    theme: ArtisanTheme,
    selected: Option<V>,
    placeholder: SharedString,
    entries: Rc<Vec<SelectEntry<V>>>,
    enabled: Rc<Vec<bool>>,
    disabled: bool,
    scroll_state: SelectScrollState,
    scroll_handle: ScrollHandle,
    on_change: Option<SelectChangeHandler<V>>,
    on_open_change: Option<SelectOpenChangeHandler>,
    debug_selector: Option<SharedString>,
    state: Rc<RefCell<SelectState>>,
    style: SelectStyle,
    selector_root: String,
    selected_idx: Option<usize>,
    highlighted: Option<usize>,
    content_id: ElementId,
}

impl<V: SelectValue> SelectFrame<V> {
    fn trigger_base(&self, label: SharedString, selector: String) -> Stateful<Div> {
        let style = &self.style;
        div()
            .id(ElementId::NamedChild(
                Arc::new(self.id.clone()),
                "trigger".into(),
            ))
            .track_focus(&self.focus)
            .tab_index(0)
            .flex()
            .flex_row()
            .items_center()
            .w_full()
            .h(style.trigger_height)
            .px(style.trigger_horizontal_padding)
            .gap(style.trigger_gap)
            .rounded(style.trigger_corner_radius)
            .border(px(1.0))
            .border_color(style.trigger_border)
            .bg(style.trigger_background)
            .text_color(if self.selected_idx.is_none() {
                style.placeholder_foreground
            } else {
                style.trigger_foreground
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .whitespace_nowrap()
                    .child(label),
            )
            .child(icon(IconStyle::resolve(
                self.theme,
                AssetId::TABLER_CHEVRON_DOWN,
                IconSize::Compact,
                IconTint::Muted,
            )))
            .debug_selector(move || selector.clone())
    }

    fn trigger(&self) -> Stateful<Div> {
        let selected_idx = self.selected_idx;
        let entries = &self.entries;
        let placeholder = &self.placeholder;
        let selector_root = &self.selector_root;
        let state = &self.state;
        let on_change = &self.on_change;
        let on_open_change = &self.on_open_change;
        let focus = &self.focus;
        let scroll_handle = &self.scroll_handle;
        let disabled = self.disabled;
        // Trigger row: committed label or placeholder plus selector glyph.
        let trigger_label = selected_idx
            .and_then(|index| entries.get(index))
            .and_then(|entry| entry.as_item())
            .map_or_else(|| placeholder.clone(), |item| item.label.clone());
        let trigger_selector = format!("{selector_root}-trigger");
        let trigger_state = Rc::clone(state);
        let trigger_entries = Rc::clone(entries);
        let trigger_enabled = Rc::clone(&self.enabled);
        let trigger_open = on_open_change.clone();
        let trigger_change = on_change.clone();
        let trigger_focus = focus.clone();
        let trigger_scroll = scroll_handle.clone();
        let trigger_escape_state = Rc::clone(state);
        let trigger_escape_enabled = Rc::clone(&self.enabled);
        let trigger_escape_open = on_open_change.clone();
        let trigger_escape_focus = focus.clone();
        let state_for_keys = Rc::clone(state);
        let entries_for_keys = Rc::clone(entries);
        let enabled_for_keys = Rc::clone(&self.enabled);
        let on_change_for_keys = on_change.clone();
        let on_open_change_for_keys = on_open_change.clone();
        let focus_for_keys = focus.clone();
        let scroll_for_keys = scroll_handle.clone();
        let mut trigger = self.trigger_base(trigger_label, trigger_selector);
        if disabled {
            trigger = trigger.opacity(DISABLED_OPACITY);
        } else {
            trigger = trigger
                .on_click(
                    move |event: &ClickEvent, window: &mut Window, cx: &mut App| {
                        let was_open = trigger_state.borrow().is_open();
                        let action = trigger_state.borrow_mut().toggle(&trigger_enabled);
                        apply_action(ActionDelivery {
                            action,
                            state: &trigger_state,
                            entries: &trigger_entries,
                            on_change: trigger_change.as_ref(),
                            on_open_change: trigger_open.as_ref(),
                            focus: &trigger_focus,
                            scroll_handle: &trigger_scroll,
                            enabled: &trigger_enabled,
                            was_open,
                            event: Some(event),
                            window,
                            cx,
                        });
                        window.refresh();
                    },
                )
                .on_key_down(
                    move |event: &KeyDownEvent, window: &mut Window, cx: &mut App| {
                        if event.keystroke.modifiers.modified() {
                            return;
                        }
                        // Escape always announces the closed state, even when
                        // the menu is already closed: the owner may hold a
                        // stale open value that only this announcement heals.
                        if event.keystroke.key.as_str() == "escape" {
                            let _ = trigger_escape_state
                                .borrow_mut()
                                .close(&trigger_escape_enabled);
                            window.focus(&trigger_escape_focus, cx);
                            if let Some(handler) = trigger_escape_open.as_ref() {
                                handler(false, window, cx);
                            }
                            window.prevent_default();
                            cx.stop_propagation();
                            window.refresh();
                            return;
                        }
                        handle_select_key(
                            &SelectKeyContext {
                                state: Rc::clone(&state_for_keys),
                                entries: Rc::clone(&entries_for_keys),
                                enabled: Rc::clone(&enabled_for_keys),
                                on_change: on_change_for_keys.clone(),
                                on_open_change: on_open_change_for_keys.clone(),
                                focus: focus_for_keys.clone(),
                                scroll_handle: scroll_for_keys.clone(),
                            },
                            event.keystroke.key.as_str(),
                            window,
                            cx,
                        );
                    },
                );
        }
        trigger
    }

    fn viewport(&self) -> Stateful<Div> {
        let entries = &self.entries;
        let content_id = &self.content_id;
        let selector_root = &self.selector_root;
        let selected = self.selected.as_ref();
        let highlighted = self.highlighted;
        let style = &self.style;
        let theme = &self.theme;
        let disabled = self.disabled;
        let state = &self.state;
        let on_change = &self.on_change;
        let on_open_change = &self.on_open_change;
        let focus = &self.focus;
        let scroll_handle = &self.scroll_handle;
        let viewport_id = ElementId::NamedChild(Arc::new(content_id.clone()), "viewport".into());
        let viewport_selector = format!("{selector_root}-viewport");
        let mut viewport = div()
            .id(viewport_id)
            .w_full()
            .max_h(style.content_max_height)
            .overflow_y_scroll()
            .track_scroll(scroll_handle)
            .debug_selector(move || viewport_selector.clone());
        for (index, entry) in entries.iter().enumerate() {
            match entry {
                SelectEntry::Group(heading) => {
                    viewport = viewport.child(
                        div()
                            .px(style.item_horizontal_padding)
                            .py(style.item_vertical_padding)
                            .text_color(style.group_label_foreground)
                            .child(heading.clone()),
                    );
                }
                SelectEntry::Separator => {
                    viewport = viewport.child(div().h(px(1.0)).bg(style.separator_color));
                }
                SelectEntry::Item(item) => {
                    let row = render_item(&RenderItemArgs {
                        theme,
                        item,
                        index,
                        content_id,
                        selector_root,
                        selected,
                        highlighted,
                        style,
                    });
                    if item.is_disabled() || disabled {
                        viewport = viewport.child(row);
                    } else {
                        let row_state = Rc::clone(state);
                        let row_entries = Rc::clone(entries);
                        let row_enabled = Rc::clone(&self.enabled);
                        let row_change = on_change.clone();
                        let row_open = on_open_change.clone();
                        let row_focus = focus.clone();
                        let row_scroll = scroll_handle.clone();
                        viewport = viewport.child(row.on_click(
                            move |event: &ClickEvent, window: &mut Window, cx: &mut App| {
                                let was_open = row_state.borrow().is_open();
                                let action = row_state.borrow_mut().commit(index, &row_enabled);
                                apply_action(ActionDelivery {
                                    action,
                                    state: &row_state,
                                    entries: &row_entries,
                                    on_change: row_change.as_ref(),
                                    on_open_change: row_open.as_ref(),
                                    focus: &row_focus,
                                    scroll_handle: &row_scroll,
                                    enabled: &row_enabled,
                                    was_open,
                                    event: Some(event),
                                    window,
                                    cx,
                                });
                                window.refresh();
                            },
                        ));
                    }
                }
            }
        }
        viewport
    }

    fn content(&self, viewport: Stateful<Div>) -> Stateful<Div> {
        let style = &self.style;
        let theme = &self.theme;
        let content_id = &self.content_id;
        let selector_root = &self.selector_root;
        let scroll_handle = &self.scroll_handle;
        let content_selector = format!("{selector_root}-content");
        let mut content = div()
            .id(content_id.clone())
            .w(style.content_min_width)
            .rounded(style.content_corner_radius)
            .bg(style.content_background)
            .text_color(style.content_foreground)
            .shadow(style.content_shadow.to_vec())
            .overflow_hidden()
            .debug_selector(move || content_selector.clone());
        if self.scroll_state.can_scroll_up() {
            let handle = scroll_handle.clone();
            content = content.child(scroll_button(
                theme,
                style.scroll_button_height,
                AssetId::TABLER_CHEVRON_UP,
                format!("{selector_root}-scroll-up"),
                move |window| {
                    handle.scroll_to_top_of_item(0);
                    window.refresh();
                },
            ));
        }
        content = content.child(viewport);
        if self.scroll_state.can_scroll_down() {
            let handle = scroll_handle.clone();
            content = content.child(scroll_button(
                theme,
                style.scroll_button_height,
                AssetId::TABLER_CHEVRON_DOWN,
                format!("{selector_root}-scroll-down"),
                move |window| {
                    handle.scroll_to_bottom();
                    window.refresh();
                },
            ));
        }
        content
    }

    fn root(&self, trigger: Stateful<Div>, content: Stateful<Div>) -> Div {
        let state = &self.state;
        let focus = &self.focus;
        let scroll_handle = &self.scroll_handle;
        let enabled = &self.enabled;
        let on_open_change = &self.on_open_change;
        let debug_selector = &self.debug_selector;
        let mut root = div().relative().w_full().child(trigger);
        if state.borrow().is_open() {
            let outside_state = Rc::clone(state);
            let outside_focus = focus.clone();
            let outside_scroll = scroll_handle.clone();
            let outside_enabled = Rc::clone(enabled);
            let outside_open = on_open_change.clone();
            root = root
                .on_mouse_down_out(move |event: &MouseDownEvent, window, cx| {
                    if outside_scroll.bounds().contains(&event.position) {
                        return;
                    }
                    let action = outside_state.borrow_mut().close(&outside_enabled);
                    if !action.is_effective() {
                        return;
                    }
                    window.focus(&outside_focus, cx);
                    if let Some(handler) = outside_open.as_ref() {
                        handler(false, window, cx);
                    }
                    cx.stop_propagation();
                })
                .child(
                    deferred(
                        anchored()
                            .position_mode(AnchoredPositionMode::Local)
                            .position(point(px(0.0), self.style.trigger_height + px(4.0)))
                            .child(content),
                    )
                    .with_priority(20),
                );
        }

        if let Some(selector) = debug_selector {
            let selector = selector.to_string();
            root = root.debug_selector(move || selector.clone());
        }
        root
    }
}

impl<V: SelectValue> RenderOnce for Select<V> {
    fn render(self, window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let Select {
            id,
            focus,
            theme,
            selected,
            open,
            placeholder,
            entries,
            size,
            disabled,
            scroll_state,
            scroll_handle,
            on_change,
            on_open_change,
            debug_selector,
            interaction_state,
        } = self;
        let mut style = SelectStyle::resolve(theme, size);
        style.content_max_height = style.content_max_height.min(
            (window.viewport_size().height - px(16.0) - style.scroll_button_height * 2.0)
                .max(px(1.0)),
        );
        style.content_min_width = style
            .content_min_width
            .min((window.viewport_size().width - px(16.0)).max(px(1.0)));
        let enabled = entry_enabled_flags(&entries);
        let selected_idx = selected_index(&entries, selected.as_ref());
        let state = interaction_state.unwrap_or_else(|| {
            Rc::new(RefCell::new(SelectState::new(open, selected_idx, &enabled)))
        });
        // Reconcile only when the controlled props actually changed since the
        // last reconciliation. View-initiated transitions (toggle, activation)
        // mutate the shared state optimistically; reconciling those away on
        // the next render would revert them before the owner applies them.
        state
            .borrow_mut()
            .reconcile_controlled(open, selected_idx, &enabled);

        let entries = Rc::new(entries);
        let enabled = Rc::new(enabled);
        let selector_root = debug_selector
            .as_ref()
            .map_or_else(|| "artisan-select".to_owned(), ToString::to_string);
        let highlighted = state.borrow().highlighted_index();
        let content_id = ElementId::NamedChild(Arc::new(id.clone()), "content".into());
        let frame = SelectFrame {
            id,
            focus,
            theme,
            selected,
            placeholder,
            entries,
            enabled,
            disabled,
            scroll_state,
            scroll_handle,
            on_change,
            on_open_change,
            debug_selector,
            state,
            style,
            selector_root,
            selected_idx,
            highlighted,
            content_id,
        };
        let trigger = frame.trigger();
        let viewport = frame.viewport();
        let content = frame.content(viewport);
        frame.root(trigger, content)
    }
}

/// Shared delivery context for trigger keystrokes.
struct SelectKeyContext<V: SelectValue> {
    state: Rc<RefCell<SelectState>>,
    entries: Rc<Vec<SelectEntry<V>>>,
    enabled: Rc<Vec<bool>>,
    on_change: Option<SelectChangeHandler<V>>,
    on_open_change: Option<SelectOpenChangeHandler>,
    focus: FocusHandle,
    scroll_handle: ScrollHandle,
}

/// Handles one trigger keystroke through the shared engine and applies the
/// resulting action with keyboard (event-less) delivery.
fn handle_select_key<V: SelectValue>(
    context: &SelectKeyContext<V>,
    key_name: &str,
    window: &mut Window,
    cx: &mut App,
) {
    let mapped = SelectKey::from_key_name(key_name);
    let key = if let Some(key) = mapped {
        key
    } else {
        let mut chars = key_name.chars();
        match (chars.next(), chars.next()) {
            (Some(character), None) if !character.is_control() => SelectKey::Character(character),
            _ => return,
        }
    };
    let was_open = context.state.borrow().is_open();
    let action = context
        .state
        .borrow_mut()
        .handle_key_at(key, &context.entries, monotonic_now());
    if action.is_effective() {
        apply_action(ActionDelivery {
            action,
            state: &context.state,
            entries: &context.entries,
            on_change: context.on_change.as_ref(),
            on_open_change: context.on_open_change.as_ref(),
            focus: &context.focus,
            scroll_handle: &context.scroll_handle,
            enabled: &context.enabled,
            was_open,
            event: None,
            window,
            cx,
        });
        window.refresh();
    }
}

/// Returns a stable hexadecimal hash suffix for one caller-owned value.
///
/// Hashing keeps arbitrary caller data out of selector text while remaining
/// deterministic within one process.
#[must_use]
pub fn stable_value_selector_suffix<V: Hash + ?Sized>(value: &V) -> String {
    let mut hasher = SelectFnv1aHasher(0xcbf2_9ce4_8422_2325);
    value.hash(&mut hasher);
    format!("{:x}", hasher.finish())
}

/// Returns the stable debug selector for one item value under a root.
#[must_use]
pub fn item_debug_selector<V: Hash + ?Sized>(root_selector: &str, value: &V) -> String {
    format!(
        "{root_selector}-item-{}",
        stable_value_selector_suffix(value)
    )
}
