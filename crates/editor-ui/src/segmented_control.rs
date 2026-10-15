//! Single-choice groups with shared geometry, semantics and roving focus.
use crate::theme::{ArtisanTheme, RadiusStep, RadiusTokens, TextStep, TypographyTokens};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, ElementId, Entity, FocusHandle, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement as _, RenderOnce, SharedString, StatefulInteractiveElement as _, Styled as _,
    Window, div, px,
};
use std::rc::Rc;

/// A typed choice; values are the stable identities within a group.
pub struct Segment<T> {
    value: T,
    label: SharedString,
}
impl<T> Segment<T> {
    pub fn new(value: T, label: impl Into<SharedString>) -> Self {
        Self {
            value,
            label: label.into(),
        }
    }
}
struct FocusState<T> {
    handles: Vec<(T, FocusHandle, usize)>,
    next_id: usize,
}
type Change<T> = Rc<dyn Fn(T, &mut Window, &mut App)>;

#[derive(IntoElement)]
pub struct SegmentedControl<T: Clone + Eq + 'static> {
    id: ElementId,
    theme: ArtisanTheme,
    label: SharedString,
    selected: T,
    options: Vec<Segment<T>>,
    disabled: bool,
    outline: bool,
    change: Option<Change<T>>,
    debug_selector: Option<SharedString>,
}
impl<T: Clone + Eq + 'static> SegmentedControl<T> {
    /// The first option makes an empty group impossible. An absent selection
    /// resolves to that first option when feature data changes.
    pub fn new(
        id: impl Into<ElementId>,
        theme: ArtisanTheme,
        label: impl Into<SharedString>,
        selected: T,
        first: Segment<T>,
    ) -> Self {
        Self {
            id: id.into(),
            theme,
            label: label.into(),
            selected,
            options: vec![first],
            disabled: false,
            outline: false,
            change: None,
            debug_selector: None,
        }
    }
    #[must_use]
    pub fn options(mut self, options: impl IntoIterator<Item = Segment<T>>) -> Self {
        for option in options {
            if !self.options.iter().any(|other| other.value == option.value) {
                self.options.push(option);
            }
        }
        self
    }
    #[must_use]
    pub fn debug_selector(mut self, selector: impl Into<SharedString>) -> Self {
        self.debug_selector = Some(selector.into());
        self
    }
    #[must_use]
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
    /// Joined outlined segments used by settings choices.
    #[must_use]
    pub fn outline(mut self) -> Self {
        self.outline = true;
        self
    }
    #[must_use]
    pub fn on_change(mut self, change: impl Fn(T, &mut Window, &mut App) + 'static) -> Self {
        self.change = Some(Rc::new(change));
        self
    }
}
impl<T: Clone + Eq + 'static> RenderOnce for SegmentedControl<T> {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = self.theme;
        let selected = self
            .options
            .iter()
            .position(|option| option.value == self.selected)
            .unwrap_or(0);
        let focus: Entity<FocusState<T>> =
            window.use_keyed_state(self.id.clone(), cx, |_, _| FocusState {
                handles: Vec::new(),
                next_id: 0,
            });
        focus.update(cx, |focus, cx| {
            focus
                .handles
                .retain(|(value, _, _)| self.options.iter().any(|option| &option.value == value));
            for option in &self.options {
                if !focus
                    .handles
                    .iter()
                    .any(|(value, _, _)| value == &option.value)
                {
                    focus
                        .handles
                        .push((option.value.clone(), cx.focus_handle(), focus.next_id));
                    focus.next_id += 1;
                }
            }
        });
        let targets: Rc<Vec<(T, FocusHandle, usize)>> = Rc::new(
            self.options
                .iter()
                .map(|option| {
                    let retained = focus.read(cx);
                    let (_, handle, id) = retained
                        .handles
                        .iter()
                        .find(|(value, _, _)| value == &option.value)
                        .expect("each retained option has a focus handle");
                    (option.value.clone(), handle.clone(), *id)
                })
                .collect(),
        );
        let option_count = self.options.len();
        let outline = self.outline;
        let selector = self.debug_selector;
        let disabled = self.disabled;
        let change = self.change;
        div()
            .id(self.id)
            .role(gpui::Role::RadioGroup)
            .aria_label(self.label)
            .flex()
            .items_center()
            .gap(if outline { px(0.0) } else { px(2.0) })
            .children(self.options.into_iter().enumerate().map(|(index, option)| {
                let chosen = index == selected;
                let targets = targets.clone();
                let key_change = change.clone();
                let click_change = change.clone();
                let target = targets[index].1.clone();
                let value = option.value;
                div()
                    .id(targets[index].2)
                    .when_some(selector.clone(), |el, selector| {
                        let selector = format!("{selector}-{index}");
                        el.debug_selector(move || selector.clone())
                    })
                    .role(gpui::Role::RadioButton)
                    .aria_label(option.label.clone())
                    .aria_toggled(if chosen {
                        gpui::Toggled::True
                    } else {
                        gpui::Toggled::False
                    })
                    .flex()
                    .items_center()
                    .justify_center()
                    .px(px(8.0))
                    .h(px(24.0))
                    .rounded(RadiusTokens::value(RadiusStep::Sm))
                    .border_1()
                    .border_color(gpui::transparent_black())
                    .text_size(TypographyTokens::size(TextStep::Xs))
                    .text_color(if chosen {
                        theme.colors.foreground.to_paint()
                    } else {
                        theme.colors.muted_foreground.to_paint()
                    })
                    .when(chosen, |el| {
                        el.font_weight(FontWeight::SEMIBOLD)
                            .bg(theme.colors.muted.to_paint())
                    })
                    .when(outline, |el| {
                        el.h(theme.density.control_default)
                            .min_w(theme.density.control_default)
                            .px(px(12.0))
                            .rounded_none()
                            .when(index == 0, |el| {
                                el.rounded_l(RadiusTokens::value(RadiusStep::X3l))
                            })
                            .when(index + 1 == option_count, |el| {
                                el.rounded_r(RadiusTokens::value(RadiusStep::X3l))
                            })
                            .border_color(theme.colors.input.to_paint())
                            .when(index > 0, |el| el.border_l_0())
                            .text_size(theme.typography.control_text)
                            .text_color(theme.colors.foreground.to_paint())
                            .font_weight(FontWeight::MEDIUM)
                    })
                    .when(disabled, |el| {
                        el.opacity(0.5).aria_description("Unavailable")
                    })
                    .when(!disabled, |el| {
                        el.track_focus(&target)
                            .tab_index(0)
                            .tab_stop(chosen)
                            .cursor_pointer()
                            .focus_visible(move |style| {
                                style.border_color(theme.colors.ring.to_paint())
                            })
                            .hover(move |style| {
                                crate::hover_card::hover_fill(
                                    style.rounded(RadiusTokens::value(RadiusStep::Sm)),
                                    &theme,
                                    crate::hover_card::HoverFill::Edged,
                                )
                            })
                    })
                    .on_click(move |_, window, cx| {
                        if !disabled {
                            if let Some(change) = &click_change {
                                change(value.clone(), window, cx);
                            }
                        }
                    })
                    .on_key_down(move |event, window, cx| {
                        if disabled {
                            return;
                        }
                        let Some(next) =
                            selection_index(event.keystroke.key.as_str(), index, targets.len())
                        else {
                            return;
                        };
                        if let Some(change) = &key_change {
                            change(targets[next].0.clone(), window, cx);
                        }
                        window.focus(&targets[next].1, cx);
                        cx.stop_propagation();
                    })
                    .child(option.label)
            }))
    }
}
fn selection_index(key: &str, index: usize, len: usize) -> Option<usize> {
    match key {
        "left" | "up" => Some(if index == 0 { len - 1 } else { index - 1 }),
        "right" | "down" => Some((index + 1) % len),
        "home" => Some(0),
        "end" => Some(len - 1),
        _ => None,
    }
}
