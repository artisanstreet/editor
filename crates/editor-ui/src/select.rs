//! A typed value picker. Opening, keyboard navigation and scrolling belong here.

use crate::theme::{ArtisanTheme, RadiusStep, RadiusTokens, SurfaceStep};
use crate::{
    AssetId,
    icon::{IconSize, IconStyle, IconTint, icon},
};
use gpui::prelude::FluentBuilder;
use gpui::{
    AnchoredPositionMode, App, BoxShadow, ClickEvent, ElementId, FocusHandle, InteractiveElement,
    IntoElement, KeyDownEvent, MouseDownEvent, ParentElement, RenderOnce, Role, ScrollStrategy,
    SharedString, StatefulInteractiveElement, Styled, UniformListScrollHandle, Window, anchored,
    deferred, div, point, px, uniform_list,
};
use std::{
    rc::Rc,
    time::{Duration, Instant},
};

#[derive(Clone)]
pub struct SelectEntry<V> {
    value: V,
    label: SharedString,
    disabled: bool,
}
impl<V> SelectEntry<V> {
    pub fn item(value: V, label: impl Into<SharedString>) -> Self {
        Self {
            value,
            label: label.into(),
            disabled: false,
        }
    }
    #[must_use]
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

struct State<V> {
    open: bool,
    highlighted: Option<V>,
    focus: FocusHandle,
    scroll: UniformListScrollHandle,
    query: String,
    typed_at: Instant,
}

type Changed<V> = Rc<dyn Fn(V, Option<&ClickEvent>, &mut Window, &mut App)>;

#[derive(IntoElement)]
pub struct Select<V: Clone + Eq + 'static> {
    id: ElementId,
    theme: ArtisanTheme,
    selected: Option<V>,
    entries: Vec<SelectEntry<V>>,
    label: SharedString,
    disabled: bool,
    on_change: Option<Changed<V>>,
    selector: Option<SharedString>,
}

impl<V: Clone + Eq + 'static> Select<V> {
    pub fn new(
        id: impl Into<ElementId>,
        theme: ArtisanTheme,
        selected: Option<V>,
        entries: Vec<SelectEntry<V>>,
    ) -> Self {
        Self {
            id: id.into(),
            theme,
            selected,
            entries,
            label: "Choose".into(),
            disabled: false,
            on_change: None,
            selector: None,
        }
    }
    #[must_use]
    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = label.into();
        self
    }
    #[must_use]
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
    #[must_use]
    pub fn debug_selector(mut self, selector: impl Into<SharedString>) -> Self {
        self.selector = Some(selector.into());
        self
    }
    #[must_use]
    pub fn on_change(
        mut self,
        handler: impl Fn(V, Option<&ClickEvent>, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_change = Some(Rc::new(handler));
        self
    }
}

fn reconcile<V: Clone + Eq>(
    state: &mut State<V>,
    entries: &[SelectEntry<V>],
    selected: Option<&V>,
) {
    if !state.open
        || !state.highlighted.as_ref().is_some_and(|value| {
            entries
                .iter()
                .any(|entry| &entry.value == value && !entry.disabled)
        })
    {
        state.highlighted = selected
            .and_then(|value| {
                entries
                    .iter()
                    .find(|entry| &entry.value == value && !entry.disabled)
            })
            .or_else(|| entries.iter().find(|entry| !entry.disabled))
            .map(|entry| entry.value.clone());
    }
}

fn reveal_highlighted<V: Eq>(state: &State<V>, entries: &[SelectEntry<V>]) {
    if let Some(index) = entries
        .iter()
        .position(|entry| Some(&entry.value) == state.highlighted.as_ref())
    {
        state.scroll.scroll_to_item(index, ScrollStrategy::Nearest);
    }
}

impl<V: Clone + Eq + 'static> RenderOnce for Select<V> {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = self.theme;
        let disabled = self.disabled || self.entries.iter().all(|entry| entry.disabled);
        let state = window.use_keyed_state(self.id.clone(), cx, |_, cx| State {
            open: false,
            highlighted: None,
            focus: cx.focus_handle(),
            scroll: UniformListScrollHandle::new(),
            query: String::new(),
            typed_at: Instant::now(),
        });
        state.update(cx, |state, _| {
            reconcile(state, &self.entries, self.selected.as_ref());
            if disabled {
                state.open = false;
            }
        });
        let entries = Rc::new(self.entries);
        let (open, focus, scroll) = {
            let state = state.read(cx);
            (state.open, state.focus.clone(), state.scroll.clone())
        };
        let display = entries
            .iter()
            .find(|entry| self.selected.as_ref() == Some(&entry.value))
            .map_or_else(|| self.label.clone(), |entry| entry.label.clone());
        let mut trigger = div()
            .id(self.id.clone())
            .role(Role::ComboBox)
            .aria_label(self.label)
            .aria_value(display.clone())
            .aria_expanded(open)
            .w_full()
            .h(theme.density.control_default)
            .px(px(12.0))
            .rounded(RadiusTokens::value(RadiusStep::X4l))
            .border_1()
            .border_color(theme.colors.input.to_paint())
            .bg(SurfaceStep::S900.oklch().to_paint())
            .text_color(theme.colors.foreground.to_paint())
            .text_size(theme.typography.control_text)
            .flex()
            .items_center()
            .justify_between()
            .gap(px(6.0))
            .child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(display),
            )
            .child(icon(IconStyle::resolve(
                theme,
                AssetId::TABLER_CHEVRON_DOWN,
                IconSize::Compact,
                IconTint::Muted,
            )))
            .when_some(self.selector.clone(), |el, selector| {
                el.debug_selector(move || format!("{selector}-trigger"))
            });
        if disabled {
            return div()
                .id(self.id.clone())
                .w_full()
                .child(trigger.aria_description("Unavailable").opacity(0.5));
        }
        let clicked = state.clone();
        let click_entries = entries.clone();
        let key_state = state.clone();
        let key_entries = entries.clone();
        let change = self.on_change.clone();
        trigger = trigger
            .track_focus(&focus)
            .tab_index(0)
            .cursor_pointer()
            .focus_visible(move |style| {
                style
                    .border_color(theme.colors.ring.to_paint())
                    .shadow(vec![BoxShadow {
                        color: theme.interaction.focus_ring_color.to_paint(),
                        offset: point(px(0.0), px(0.0)),
                        blur_radius: px(0.0),
                        spread_radius: theme.interaction.focus_ring_width,
                        inset: false,
                    }])
            })
            .on_click(move |event, window, cx| {
                // Enter/Space are handled on key down below. GPUI synthesizes
                // a keyboard click on key release; handling both reopens a
                // committed popup. Accessibility Click uses native pointer
                // dispatch and still reaches this handler.
                if event.is_keyboard() {
                    return;
                }
                clicked.update(cx, |state, cx| {
                    state.open = !state.open;
                    state.query.clear();
                    if state.open {
                        reveal_highlighted(state, &click_entries);
                    }
                    window.focus(&state.focus, cx);
                    cx.notify();
                })
            })
            .on_key_down(move |event: &KeyDownEvent, window, cx| {
                let mut commit = None;
                let handled = key_state.update(cx, |state, cx| {
                    let enabled: Vec<usize> = key_entries
                        .iter()
                        .enumerate()
                        .filter_map(|(i, e)| (!e.disabled).then_some(i))
                        .collect();
                    if enabled.is_empty() {
                        return false;
                    }
                    let current = enabled
                        .iter()
                        .position(|index| {
                            state.highlighted.as_ref() == Some(&key_entries[*index].value)
                        })
                        .unwrap_or(0);
                    let target = match event.keystroke.key.as_str() {
                        "escape" | "tab" => {
                            state.open = false;
                            state.query.clear();
                            cx.notify();
                            return event.keystroke.key != "tab";
                        }
                        "enter" | "space" if state.open => {
                            commit = state.highlighted.clone();
                            state.open = false;
                            cx.notify();
                            return true;
                        }
                        "enter" | "space" => {
                            state.open = true;
                            reveal_highlighted(state, &key_entries);
                            cx.notify();
                            return true;
                        }
                        "down" => {
                            if state.open {
                                (current + 1).min(enabled.len() - 1)
                            } else {
                                current
                            }
                        }
                        "up" => {
                            if state.open {
                                current.saturating_sub(1)
                            } else {
                                current
                            }
                        }
                        "home" => 0,
                        "end" => enabled.len() - 1,
                        key if key.chars().count() == 1
                            && !event.keystroke.modifiers.control
                            && !event.keystroke.modifiers.platform
                            && !event.keystroke.modifiers.alt =>
                        {
                            let now = Instant::now();
                            if now.duration_since(state.typed_at) > Duration::from_secs(1) {
                                state.query.clear();
                            }
                            state.typed_at = now;
                            state.query.push_str(&key.to_lowercase());
                            enabled
                                .iter()
                                .position(|index| {
                                    key_entries[*index]
                                        .label
                                        .to_lowercase()
                                        .starts_with(&state.query)
                                })
                                .unwrap_or(current)
                        }
                        _ => return false,
                    };
                    state.open = true;
                    let index = enabled[target];
                    state.highlighted = Some(key_entries[index].value.clone());
                    state.scroll.scroll_to_item(index, ScrollStrategy::Nearest);
                    cx.notify();
                    true
                });
                if let Some(value) = commit
                    && let Some(change) = &change
                {
                    change(value, None, window, cx);
                }
                if handled {
                    window.prevent_default();
                    cx.stop_propagation();
                }
            });
        let mut root = div().id(self.id.clone()).relative().w_full().child(trigger);
        if open {
            let popup_priority = crate::overlay::popup_priority(window, cx);
            let popup_height = (window.viewport_size().height - px(16.0)).max(px(1.0));
            let list_state = state.clone();
            let list_entries = entries.clone();
            let change = self.on_change;
            let selected = self.selected;
            let count = entries.len();
            let row_height = window.line_height() + px(16.0);
            let list = uniform_list("select-options", count, move |range, _, cx| {
                range
                    .map(|index| {
                        let entry = &list_entries[index];
                        let hovered =
                            list_state.read(cx).highlighted.as_ref() == Some(&entry.value);
                        let mut row = div()
                            .id(index)
                            .role(Role::ListBoxOption)
                            .aria_label(entry.label.clone())
                            .aria_selected(selected.as_ref() == Some(&entry.value))
                            .relative()
                            .py(px(8.0))
                            .px(px(12.0))
                            .pr(px(32.0))
                            .flex()
                            .items_center()
                            .rounded(RadiusTokens::value(RadiusStep::Xl))
                            .text_size(theme.typography.control_text)
                            .text_color(theme.colors.popover_foreground.to_paint())
                            .child(
                                div()
                                    .min_w_0()
                                    .flex_1()
                                    .truncate()
                                    .whitespace_nowrap()
                                    .child(entry.label.clone()),
                            )
                            .when(selected.as_ref() == Some(&entry.value), |el| {
                                el.child(
                                    div()
                                        .absolute()
                                        .right(px(8.0))
                                        .top(px(8.0))
                                        .size(px(14.0))
                                        .child(icon(IconStyle::resolve(
                                            theme,
                                            AssetId::TABLER_CHECK,
                                            IconSize::Compact,
                                            IconTint::Inherit,
                                        ))),
                                )
                            })
                            .when(hovered, |el| {
                                el.bg(theme.colors.accent.to_paint())
                                    .text_color(theme.colors.accent_foreground.to_paint())
                            });
                        if entry.disabled {
                            return row.opacity(0.5);
                        }
                        let state = list_state.clone();
                        let value = entry.value.clone();
                        let change = change.clone();
                        row = row.cursor_pointer().on_click(move |event, window, cx| {
                            state.update(cx, |state, cx| {
                                state.open = false;
                                window.focus(&state.focus, cx);
                                cx.notify();
                            });
                            if let Some(change) = &change {
                                change(value.clone(), Some(event), window, cx);
                            }
                            cx.stop_propagation();
                        });
                        row
                    })
                    .collect()
            })
            .h((row_height * count.max(1) as f32)
                .min(theme.density.command_list_max_height)
                .min(popup_height))
            .track_scroll(&scroll);
            let outside = state.clone();
            let popup_bounds = crate::measure::Measured::default();
            let outside_bounds = popup_bounds.clone();
            root = root
                .on_mouse_down_out(move |event: &MouseDownEvent, _, cx| {
                    if outside_bounds.contains(&event.position) {
                        return;
                    }
                    outside.update(cx, |state, cx| {
                        state.open = false;
                        cx.notify();
                    });
                })
                .child(
                    deferred(
                        anchored()
                            .position_mode(AnchoredPositionMode::Local)
                            .snap_to_window_with_margin(px(8.0))
                            .position(point(px(0.0), theme.density.control_default + px(4.0)))
                            .child(
                                div()
                                    .id("select-listbox")
                                    .role(Role::ListBox)
                                    .when_some(self.selector.clone(), |el, selector| {
                                        el.debug_selector(move || format!("{selector}-popup"))
                                    })
                                    .relative()
                                    .w(px(144.0).min(
                                        (window.viewport_size().width - px(16.0)).max(px(1.0)),
                                    ))
                                    .child(popup_bounds.record())
                                    .child(
                                        crate::popover::PopoverPanel::new("panel", theme, list)
                                            .select_menu(),
                                    ),
                            ),
                    )
                    .with_priority(popup_priority),
                );
        }
        root
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[gpui::test]
    fn committed_values_and_reordered_options_keep_semantic_selection(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            let mut state = State {
                open: false,
                highlighted: Some(1),
                focus: cx.focus_handle(),
                scroll: UniformListScrollHandle::new(),
                query: String::new(),
                typed_at: Instant::now(),
            };
            let entries = vec![SelectEntry::item(1, "One"), SelectEntry::item(2, "Two")];
            reconcile(&mut state, &entries, Some(&2));
            assert_eq!(state.highlighted, Some(2));
            state.open = true;
            let reversed = vec![SelectEntry::item(2, "Two"), SelectEntry::item(1, "One")];
            reconcile(&mut state, &reversed, Some(&1));
            assert_eq!(state.highlighted, Some(2));
            let removed = vec![SelectEntry::item(1, "One")];
            reconcile(&mut state, &removed, Some(&1));
            assert_eq!(state.highlighted, Some(1));
        });
    }
}
