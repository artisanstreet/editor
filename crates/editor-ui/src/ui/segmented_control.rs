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

#[derive(Debug)]
pub struct Segment<T> {
    value: T,
    label: String,
    icon: Option<editor_assets::AssetId>,
    glyph: Option<super::carved::glyph::Icon>,
}
impl<T> Segment<T> {
    pub fn new(value: T, label: impl Into<String>) -> Self {
        Self {
            value,
            label: label.into(),
            icon: None,
            glyph: None,
        }
    }
    #[must_use]
    pub const fn glyph(mut self, glyph: super::carved::glyph::Icon) -> Self {
        self.glyph = Some(glyph);
        self
    }
    #[must_use]
    pub fn icon(mut self, icon: editor_assets::AssetId) -> Self {
        self.icon = Some(icon);
        self
    }
}

type Change<T> = Rc<dyn Fn(T)>;

pub struct SegmentedControl<T: Clone + Eq + 'static> {
    id: Key,
    theme: ArtisanTheme,
    label: String,
    selected: T,
    options: Vec<Segment<T>>,
    disabled: bool,
    carved: bool,
    change: Option<Change<T>>,
}

impl<T: Clone + Eq + 'static> std::fmt::Debug for SegmentedControl<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SegmentedControl")
            .field("label", &self.label)
            .finish_non_exhaustive()
    }
}

impl<T: Clone + Eq + 'static> SegmentedControl<T> {
    #[expect(
        clippy::large_types_passed_by_value,
        reason = "the segmented control stores this theme copy and paints from it when built"
    )]
    pub fn new(
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
            carved: false,
            change: None,
        }
    }
    #[must_use]
    pub fn options(mut self, options: impl IntoIterator<Item = Segment<T>>) -> Self {
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
    #[must_use]
    pub const fn carved(mut self) -> Self {
        self.carved = true;
        self
    }
    #[must_use]
    pub fn on_change(mut self, handler: impl Fn(T) + 'static) -> Self {
        self.change = Some(Rc::new(handler));
        self
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one segmented control build with its segments and keyboard semantics"
    )]
    pub fn build(self) -> Element {
        let theme = self.theme;
        let segment_height = SEGMENT
            .pad()
            .y
            .px()
            .mul_add(2.0, SEGMENT_TEXT.line_px(&theme));
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
        let carve = super::carved::Carve::of(&theme);
        let mut root = UiBox::new()
            .spaced(Space::S2)
            .style(Style::new().align(aqua_ui::AlignItems::CENTER));
        if self.carved {
            root = root.style(super::carved::tabs::well(&carve));
        }
        for (index, option) in self.options.into_iter().enumerate() {
            let chosen = index == selected;
            if self.carved {
                let change = (!self.disabled).then(|| self.change.clone()).flatten();
                let value = option.value.clone();
                let item = super::carved::tabs::tab(
                    &theme,
                    &carve,
                    super::carved::tabs::Tab {
                        index,
                        label: option.label,
                        glyph: option.glyph,
                        chosen,
                        disabled: self.disabled,
                    },
                    change
                        .map(|change| -> Box<dyn Fn()> { Box::new(move || change(value.clone())) }),
                );
                root = root.child(item);
                continue;
            }
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
            let text = option.icon.map_or(text, |icon| {
                super::kit::IconSize::Md.glyph(icon, foreground.to_reference_paint())
            });
            let style = Style::new()
                .align(aqua_ui::AlignItems::CENTER)
                .justify(aqua_ui::JustifyContent::CENTER)
                .padding(Length::Px(SEGMENT.pad().x.px()))
                .border_of(1.0, Oklch::TRANSPARENT)
                .radius(Length::Px(Radius::Field.px()));
            let mut button = Button::new()
                .height(segment_height)
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
