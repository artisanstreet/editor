use std::cell::Cell;
use std::rc::Rc;

use aqua_core::{InputEvent, Key as InputKey, NamedKey, Oklch, Role};
use aqua_text::FontWeight;
use aqua_ui::{
    element::{Box as UiBox, Button, Element, Key},
    input::{EventPhase, InputKind, InputResponse, RoutedInput},
    style::{Length, Style},
};

use super::kit::{Density, Hover, Radius, Row, Space, Spaced, TextRole, control::focus_ring, live};
use super::theme::{ArtisanTheme, Opacity};

const SEGMENT: Row = Row::ListItem(Density::Dense);
const SEGMENT_TEXT: TextRole = TextRole::Caption;
const SEGMENT_HEIGHT: f32 = SEGMENT_TEXT.spec().line + 2.0 * SEGMENT.pad().y.px();

pub(crate) struct Segment<T> {
    value: T,
    label: String,
}
impl<T> Segment<T> {
    pub(crate) fn new(value: T, label: impl Into<String>) -> Self {
        Self {
            value,
            label: label.into(),
        }
    }
}

type Change<T> = Rc<dyn Fn(T)>;

pub(crate) struct SegmentedControl<T: Clone + Eq + 'static> {
    id: Key,
    theme: ArtisanTheme,
    label: String,
    selected: T,
    options: Vec<Segment<T>>,
    disabled: bool,
    change: Option<Change<T>>,
}

impl<T: Clone + Eq + 'static> SegmentedControl<T> {
    pub(crate) fn new(
        id: impl Into<Key>,
        theme: ArtisanTheme,
        label: impl Into<String>,
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
            change: None,
        }
    }
    pub(crate) fn options(mut self, options: impl IntoIterator<Item = Segment<T>>) -> Self {
        for option in options {
            if !self
                .options
                .iter()
                .any(|existing| existing.value == option.value)
            {
                self.options.push(option);
            }
        }
        self
    }
    pub(crate) fn on_change(mut self, handler: impl Fn(T) + 'static) -> Self {
        self.change = Some(Rc::new(handler));
        self
    }

    pub(crate) fn build(self) -> Element {
        let theme = self.theme;
        let selected = self
            .options
            .iter()
            .position(|option| option.value == self.selected)
            .unwrap_or(0);
        let count = self.options.len();
        let values = Rc::new(
            self.options
                .iter()
                .map(|option| option.value.clone())
                .collect::<Vec<_>>(),
        );
        let mut root = UiBox::new()
            .spaced(Space::S2)
            .style(Style::new().align(aqua_ui::AlignItems::CENTER));
        for (index, option) in self.options.into_iter().enumerate() {
            let chosen = index == selected;
            let foreground = if chosen {
                theme.colors.text
            } else {
                theme.colors.text_muted
            };
            let mut font = SEGMENT_TEXT.style(&theme);
            if chosen {
                font.weight = FontWeight(600);
            }
            let text = SEGMENT_TEXT
                .text(&theme, option.label.clone())
                .text_style(font)
                .gpui_source_color(foreground.to_reference_text_color())
                .style(Style::new().text_wrap(false))
                .build();
            let style = Style::new()
                .align(aqua_ui::AlignItems::CENTER)
                .justify(aqua_ui::JustifyContent::CENTER)
                .padding(Length::Px(SEGMENT.pad().x.px()))
                .border_of(1.0, Oklch::TRANSPARENT)
                .radius(Length::Px(Radius::Field.px()));
            let mut button = Button::new()
                .height(SEGMENT_HEIGHT)
                .style(style)
                .child(live::layer(
                    Hover::Neutral,
                    &theme,
                    Radius::Field.px(),
                    chosen,
                ))
                .child(text);
            if self.disabled {
                button = button.opacity(Opacity::Disabled.alpha());
            }
            if !self.disabled
                && let Some(change) = self.change.clone()
            {
                let value = option.value.clone();
                button = button.on_activate(move || change(value.clone()));
            }
            let mut item = button
                .build()
                .with_key(index)
                .role(Role::RadioButton)
                .aria_label(option.label)
                .aria_checked(chosen)
                .focusable(false)
                .disabled(self.disabled);
            if chosen {
                item = item.aria_active_descendant();
            }
            if !self.disabled {
                item = live::track(item).cursor(aqua_core::CursorIcon::Pointer);
            }
            root = root.child(item);
        }
        let group = root
            .build()
            .with_key(self.id)
            .role(Role::RadioGroup)
            .aria_label(self.label)
            .focusable(!self.disabled);
        if self.disabled {
            return group;
        }
        let current = Rc::new(Cell::new(selected));
        let change = self.change;
        group
            .tab_index(0)
            .focus_visible_style(focus_ring(&theme))
            .on_input(InputKind::KeyDown, EventPhase::Bubble, move |event| {
                let RoutedInput::Native(InputEvent::KeyDown(key)) = &event.input else {
                    return InputResponse::default();
                };
                let at = current.get();
                let next = match key.key {
                    InputKey::Named(NamedKey::Left | NamedKey::Up) => (at + count - 1) % count,
                    InputKey::Named(NamedKey::Right | NamedKey::Down) => (at + 1) % count,
                    InputKey::Named(NamedKey::Home) => 0,
                    InputKey::Named(NamedKey::End) => count - 1,
                    InputKey::Char(_) | InputKey::Named(_) => return InputResponse::default(),
                };
                current.set(next);
                if let Some(change) = &change {
                    change(values[next].clone());
                }
                InputResponse {
                    stop_propagation: true,
                    ..InputResponse::handled()
                }
            })
    }
}

#[cfg(test)]
#[path = "../../tests/ui/segmented_control.rs"]
mod tests;
