//! A typed value picker on the composer's dropdown recipe: a bordered
//! trigger, and a glass menu that fades in under it with the shared hover
//! pill. Opening, keyboard navigation and scrolling belong here.

use crate::AssetId;
use crate::asset_seam::asset_glyph;
use crate::hover_card::highlight_fill;
use crate::picker_motion::{HoverSurface, MenuEntrance, MenuMotion, animate_menu};
use crate::theme::{ArtisanTheme, Opacity, RadiusStep, RadiusTokens, TextStep, TypographyTokens};
use gpui::prelude::FluentBuilder;
use gpui::{
    App, BoxShadow, ClickEvent, Context, ElementId, FocusHandle, InteractiveElement, IntoElement,
    KeyDownEvent, MouseDownEvent, ParentElement, RenderOnce, Role, ScrollStrategy, SharedString,
    StatefulInteractiveElement, Styled, UniformListScrollHandle, Window, deferred, div, point, px,
    uniform_list,
};
use std::cell::RefCell;
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
    pub const fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

/// The menu's fade, as the composer's dropdowns run it.
const MENU_MOTION: Duration = Duration::from_millis(100);
/// One option's height: 20 px of text in 8 px above and below.
const OPTION_HEIGHT: f32 = 36.0;
/// The menu's narrowest.
const MENU_MIN_WIDTH: f32 = 160.0;

struct State<V> {
    open: bool,
    /// Painted: open, or playing its exit.
    shown: bool,
    motion: Rc<RefCell<MenuMotion>>,
    hover: HoverSurface,
    highlighted: Option<V>,
    /// The keys moved the highlight; it wears the highlight fill while the
    /// pointer's row wears the pill.
    keyed: bool,
    focus: FocusHandle,
    scroll: UniformListScrollHandle,
    query: String,
    typed_at: Instant,
    trigger_bounds: crate::measure::Measured,
}

impl<V: 'static> State<V> {
    /// Opens or closes the menu; it fades in and out unless motion is
    /// reduced.
    fn set_open(&mut self, open: bool, cx: &mut Context<Self>) {
        if self.open == open {
            return;
        }
        self.open = open;
        let generation = {
            let mut motion = self.motion.borrow_mut();
            if open {
                motion.begin_open()
            } else {
                motion.begin_close()
            }
        };
        if open {
            self.shown = true;
        } else {
            self.keyed = false;
            self.query.clear();
            self.hover.clear();
        }
        cx.notify();
        let settle = move |state: &mut Self| {
            let mut motion = state.motion.borrow_mut();
            let settled = if open {
                motion.finish_open(generation)
            } else {
                motion.finish_close(generation)
            };
            drop(motion);
            if settled && !open {
                state.shown = false;
            }
            settled
        };
        if cx.reduce_motion() {
            settle(self);
            return;
        }
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(MENU_MOTION).await;
            // A released view has nothing left to update.
            this.update(cx, |state, cx| {
                if settle(state) {
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Drops the menu without its exit.
    fn close_now(&mut self) {
        self.open = false;
        self.shown = false;
        self.keyed = false;
        self.motion.borrow_mut().hide();
        self.hover.clear();
    }
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
    pub const fn disabled(mut self, disabled: bool) -> Self {
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
    #[expect(
        clippy::too_many_lines,
        reason = "one element tree; splitting it would only thread its locals through helpers"
    )]
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = self.theme;
        let disabled = self.disabled || self.entries.iter().all(|entry| entry.disabled);
        let state = window.use_keyed_state(self.id.clone(), cx, |_, cx| State {
            open: false,
            shown: false,
            motion: Rc::default(),
            hover: HoverSurface::default(),
            highlighted: None,
            keyed: false,
            focus: cx.focus_handle(),
            scroll: UniformListScrollHandle::new(),
            query: String::new(),
            typed_at: Instant::now(),
            trigger_bounds: crate::measure::Measured::default(),
        });
        state.update(cx, |state, _| {
            reconcile(state, &self.entries, self.selected.as_ref());
            if disabled {
                state.close_now();
            }
        });
        let entries = Rc::new(self.entries);
        let (open, shown, focus, scroll, hover, motion, keyed) = {
            let state = state.read(cx);
            (
                state.open,
                state.shown,
                state.focus.clone(),
                state.scroll.clone(),
                state.hover.clone(),
                Rc::clone(&state.motion),
                state.keyed,
            )
        };
        let muted = theme.colors.text_muted.to_paint();
        let trigger_bounds = state.read(cx).trigger_bounds.clone();
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
            .relative()
            .w_full()
            .h(theme.density.control_default)
            .px(px(10.0))
            .rounded(RadiusTokens::value(RadiusStep::Md))
            .border_1()
            .border_color(theme.colors.text.at(Opacity::GlassBorder).to_paint())
            .bg(theme.colors.text.at(Opacity::GlassBase).to_paint())
            .text_color(theme.colors.text.to_paint())
            .text_size(TypographyTokens::size(TextStep::Sm))
            .flex()
            .items_center()
            .justify_between()
            .gap(px(8.0))
            .child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(display),
            )
            .child(
                asset_glyph(AssetId::TABLER_SELECTOR)
                    .size(px(14.0))
                    .flex_shrink_0()
                    .text_color(muted),
            )
            .when_some(self.selector.clone(), |el, selector| {
                el.debug_selector(move || format!("{selector}-trigger"))
            });
        if disabled {
            return div().id(self.id.clone()).w_full().child(
                trigger
                    .aria_description("Unavailable")
                    .opacity(crate::theme::Opacity::Disabled.alpha()),
            );
        }
        let clicked = state.clone();
        let click_entries = Rc::clone(&entries);
        let key_state = state.clone();
        let key_entries = Rc::clone(&entries);
        let change = self.on_change.clone();
        trigger = trigger
            .track_focus(&focus)
            .tab_index(0)
            .cursor_pointer()
            .hover(move |style| style.bg(theme.colors.text.at(Opacity::HoverBottom).to_paint()))
            .when(open, |trigger| {
                trigger
                    .bg(theme.colors.text.at(Opacity::HoverBottom).to_paint())
                    .border_color(theme.colors.text.at(Opacity::HoverRing).to_paint())
            })
            .focus_visible(move |style| {
                style
                    .border_color(theme.colors.ring.to_paint())
                    .shadow(vec![BoxShadow {
                        color: theme
                            .colors
                            .ring
                            .at(crate::theme::Opacity::FocusRing)
                            .to_paint(),
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
                    let open = !state.open;
                    state.set_open(open, cx);
                    if open {
                        reveal_highlighted(state, &click_entries);
                    }
                    window.focus(&state.focus, cx);
                });
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
                            let was_open = state.open;
                            state.set_open(false, cx);
                            return was_open && event.keystroke.key != "tab";
                        }
                        "enter" | "space" if state.open => {
                            commit.clone_from(&state.highlighted);
                            state.set_open(false, cx);
                            return true;
                        }
                        "enter" | "space" => {
                            state.set_open(true, cx);
                            state.keyed = true;
                            reveal_highlighted(state, &key_entries);
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
                    state.set_open(true, cx);
                    state.keyed = true;
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
        let mut root = div()
            .id(self.id.clone())
            .relative()
            .w_full()
            .child(trigger_bounds.track())
            .child(trigger);
        if shown {
            let popup_priority = crate::overlay::popup_priority(window, cx);
            let bounds = trigger_bounds.get();
            let list_state = state.clone();
            let list_entries = Rc::clone(&entries);
            let list_hover = hover.clone();
            let change = self.on_change;
            let selected = self.selected;
            let count = entries.len();
            let list = uniform_list("select-options", count, move |range, _, cx| {
                range
                    .map(|index| {
                        let entry = &list_entries[index];
                        let chosen = selected.as_ref() == Some(&entry.value);
                        let highlighted =
                            keyed && list_state.read(cx).highlighted.as_ref() == Some(&entry.value);
                        let hover_id = index.to_string();
                        let mut row = div()
                            .id(index)
                            .role(Role::ListBoxOption)
                            .aria_label(entry.label.clone())
                            .aria_selected(chosen)
                            .relative()
                            // The menu's width, so the pill and highlight
                            // span the row, not just its label.
                            .w_full()
                            .h(px(OPTION_HEIGHT))
                            .pl(px(12.0))
                            .pr(px(32.0))
                            .flex()
                            .items_center()
                            .rounded(RadiusTokens::value(RadiusStep::Xl))
                            .text_size(TypographyTokens::size(TextStep::Sm))
                            .line_height(px(20.0))
                            .child(
                                div()
                                    .min_w_0()
                                    .flex_1()
                                    .truncate()
                                    .whitespace_nowrap()
                                    .child(entry.label.clone()),
                            )
                            .when(chosen, |el| {
                                el.child(
                                    div()
                                        .absolute()
                                        .right(px(8.0))
                                        .top_0()
                                        .bottom_0()
                                        .w(px(14.0))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .child(
                                            asset_glyph(AssetId::TABLER_CHECK)
                                                .size(px(14.0))
                                                .text_color(muted),
                                        ),
                                )
                            })
                            .when(highlighted, |row| highlight_fill(row, &theme));
                        if entry.disabled {
                            return row.opacity(crate::theme::Opacity::Disabled.alpha());
                        }
                        let state = list_state.clone();
                        let value = entry.value.clone();
                        let change = change.clone();
                        let (hover, hover_state) = (list_hover.clone(), list_state.clone());
                        row = row
                            .cursor_pointer()
                            .child(list_hover.row_probe(hover_id.clone()))
                            .on_hover(move |hovered: &bool, _, cx| {
                                hover.hovered(&hover_id, *hovered);
                                hover_state.update(cx, |state, cx| {
                                    if *hovered {
                                        state.keyed = false;
                                    }
                                    cx.notify();
                                });
                            })
                            .on_click(move |event, window, cx| {
                                state.update(cx, |state, cx| {
                                    state.set_open(false, cx);
                                    window.focus(&state.focus, cx);
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
            .h((px(OPTION_HEIGHT) * crate::f32_of_count(count.max(1)))
                .min(theme.density.command_list_max_height))
            .track_scroll(&scroll);
            let outside = state.clone();
            let popup_bounds = crate::measure::Measured::default();
            let outside_bounds = popup_bounds.clone();
            let radius = RadiusTokens::value(RadiusStep::X2l);
            let panel = crate::popover::glass_menu("select-panel", &theme, radius)
                .role(Role::ListBox)
                .when_some(self.selector.clone(), |el, selector| {
                    el.debug_selector(move || format!("{selector}-popup"))
                })
                .relative()
                .w(bounds.map_or(px(MENU_MIN_WIDTH), |bounds| {
                    bounds.size.width.max(px(MENU_MIN_WIDTH))
                }))
                .p(px(4.0))
                .child(popup_bounds.record())
                .child(hover.surface_probe())
                .child(hover.pill(
                    "select-hover",
                    &theme,
                    RadiusTokens::value(RadiusStep::Xl),
                    cx.reduce_motion(),
                ))
                .child(list);
            if open {
                root = root.on_mouse_down_out(move |event: &MouseDownEvent, _, cx| {
                    if outside_bounds.contains(&event.position) {
                        return;
                    }
                    outside.update(cx, |state, cx| state.set_open(false, cx));
                });
            }
            // Keep the popup in the trigger's layout coordinate system, even
            // inside scrolling content. Deferred painting lets it overflow its
            // ancestors; the window clips it without moving or resizing it.
            root = root.child(
                div()
                    .absolute()
                    .top(theme.density.control_default + px(6.0))
                    .left_0()
                    .child(
                        deferred(div().id("select-listbox").child(animate_menu(
                            panel,
                            &motion,
                            "select",
                            MenuEntrance::Scale {
                                origin: gpui::Point::new(0.0, 0.0),
                            },
                            MENU_MOTION,
                            MENU_MOTION,
                        )))
                        .with_priority(popup_priority),
                    ),
            );
        }
        root
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Render, TestAppContext};

    struct PlacementGallery {
        top: f32,
        count: usize,
    }

    impl Render for PlacementGallery {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .id("placement-scroll")
                .size_full()
                .overflow_y_scroll()
                .child(
                    div().relative().w_full().h(px(1000.0)).child(
                        div()
                            .absolute()
                            .top(px(self.top))
                            .left(px(80.0))
                            .w(px(200.0))
                            .child(
                                Select::new(
                                    "placement-select",
                                    ArtisanTheme::resolved(),
                                    Some(0),
                                    (0..self.count)
                                        .map(|i| SelectEntry::item(i, format!("Option {i}")))
                                        .collect(),
                                )
                                .debug_selector("placement-select"),
                            ),
                    ),
                )
        }
    }

    #[gpui::test]
    fn popup_follows_trigger_width_and_opens_below(cx: &mut TestAppContext) {
        let (_, cx) = cx.add_window_view(|_, _| PlacementGallery {
            top: 80.0,
            count: 2,
        });
        cx.simulate_resize(gpui::size(px(480.0), px(420.0)));
        let trigger = cx
            .debug_bounds("placement-select-trigger")
            .expect("trigger");
        cx.simulate_click(trigger.center(), gpui::Modifiers::none());
        let popup = cx.debug_bounds("placement-select-popup").expect("popup");
        assert_eq!(popup.left(), trigger.left());
        assert_eq!(popup.top(), trigger.bottom() + px(6.0));
        assert_eq!(popup.size.width, trigger.size.width);
    }

    #[gpui::test]
    fn popup_stays_attached_when_its_page_scrolls(cx: &mut TestAppContext) {
        let (_, cx) = cx.add_window_view(|_, _| PlacementGallery {
            top: 140.0,
            count: 2,
        });
        cx.simulate_resize(gpui::size(px(480.0), px(420.0)));
        let before = cx
            .debug_bounds("placement-select-trigger")
            .expect("trigger");
        cx.simulate_click(before.center(), gpui::Modifiers::none());
        cx.update(|window, cx| {
            window.dispatch_event(
                gpui::PlatformInput::ScrollWheel(gpui::ScrollWheelEvent {
                    position: point(px(400.0), px(200.0)),
                    delta: gpui::ScrollDelta::Pixels(point(px(0.0), px(-60.0))),
                    ..Default::default()
                }),
                cx,
            );
        });
        cx.run_until_parked();
        let trigger = cx
            .debug_bounds("placement-select-trigger")
            .expect("scrolled trigger");
        let popup = cx
            .debug_bounds("placement-select-popup")
            .expect("scrolled popup");
        assert!(trigger.top() < before.top());
        assert_eq!(popup.left(), trigger.left());
        assert_eq!(popup.top(), trigger.bottom() + px(6.0));
        assert_eq!(popup.size.width, trigger.size.width);
    }

    #[gpui::test]
    fn tall_popup_overflows_the_window_without_moving_or_shrinking(cx: &mut TestAppContext) {
        let (_, cx) = cx.add_window_view(|_, _| PlacementGallery {
            top: 340.0,
            count: 30,
        });
        cx.simulate_resize(gpui::size(px(480.0), px(420.0)));
        let trigger = cx
            .debug_bounds("placement-select-trigger")
            .expect("trigger");
        cx.simulate_click(trigger.center(), gpui::Modifiers::none());
        let popup = cx.debug_bounds("placement-select-popup").expect("popup");
        assert_eq!(popup.left(), trigger.left());
        assert_eq!(popup.top(), trigger.bottom() + px(6.0));
        assert!(popup.bottom() > px(420.0));
        assert_eq!(
            popup.size.height,
            ArtisanTheme::resolved().density.command_list_max_height + px(8.0)
        );
    }

    #[gpui::test]
    #[expect(
        clippy::needless_pass_by_ref_mut,
        reason = "gpui::test hands every test its context as &mut"
    )]
    fn committed_values_and_reordered_options_keep_semantic_selection(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            let mut state = State {
                open: false,
                shown: false,
                motion: Rc::default(),
                hover: HoverSurface::default(),
                highlighted: Some(1),
                keyed: false,
                focus: cx.focus_handle(),
                scroll: UniformListScrollHandle::new(),
                query: String::new(),
                typed_at: Instant::now(),
                trigger_bounds: crate::measure::Measured::default(),
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
