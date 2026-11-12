use super::{
    text::truncated::TruncatedText,
    kit::{Density, Hover, Radius, Row, Space},
    theme::{ArtisanTheme, Opacity},
};
use aqua_core::{CursorIcon, Role};
use aqua_ui::{
    AlignItems,
    element::{Box as UiBox, Button, Element, Key},
    style::{Length, Style},
    tree::ActionEvent,
};
pub struct ActionRow {
    id: Key,
    theme: ArtisanTheme,
    label: String,
    secondary: Option<String>,
    trailing: Option<Element>,
    selected: bool,
    disabled: bool,
    activate: Option<Box<dyn Fn(ActionEvent)>>,
}
impl ActionRow {
    pub fn row_height(theme: &ArtisanTheme) -> f32 {
        let pad = Row::ListItem(Density::Regular).pad().y.px();
        theme
            .density
            .control_sm
            .max(theme.typography.control_text * 1.25 + 2.0 * pad)
    }
    pub fn new(id: impl Into<Key>, theme: ArtisanTheme, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            theme,
            label: label.into(),
            secondary: None,
            trailing: None,
            selected: false,
            disabled: false,
            activate: None,
        }
    }
    pub fn secondary(mut self, value: impl Into<String>) -> Self {
        self.secondary = Some(value.into());
        self
    }
    pub fn trailing(mut self, value: Element) -> Self {
        self.trailing = Some(value);
        self
    }
    pub const fn selected(mut self, value: bool) -> Self {
        self.selected = value;
        self
    }
    pub const fn disabled(mut self, value: bool) -> Self {
        self.disabled = value;
        self
    }
    pub fn on_activate(mut self, callback: impl Fn(ActionEvent) + 'static) -> Self {
        self.activate = Some(Box::new(callback));
        self
    }
    pub fn build(self) -> Element {
        let t = self.theme;
        let mut text = UiBox::new()
            .layout("column")
            .style(Style::new().flex_grow(1.0).min_width(Length::Px(0.0)))
            .child(
                TruncatedText::new(
                    self.label.clone(),
                    t.typography.control_text,
                    t.colors.text.to_reference_text_color(),
                )
                .build(),
            );
        if let Some(secondary) = self.secondary {
            text = text.child(
                TruncatedText::new(
                    secondary,
                    12.0,
                    t.colors.text_muted.to_reference_text_color(),
                )
                .build(),
            );
        }
        let mut root = Button::new()
            .width(Length::Pct(100.0))
            .gap(Space::S8.px())
            .radius(Radius::Field.px())
            .border((1.0, aqua_core::Oklch::TRANSPARENT))
            .style(
                Style::new()
                    .align(AlignItems::CENTER)
                    .min_height(Length::Px(Self::row_height(&t)))
                    .padding(Length::Px(Row::ListItem(Density::Regular).pad().x.px())),
            )
            .child(text.build());
        if self.selected {
            root = root.style(Hover::Neutral.style(&t));
        }
        if let Some(trailing) = self.trailing {
            root = root.child(
                UiBox::new()
                    .style(Style::new().flex_shrink(0.0))
                    .child(trailing)
                    .build(),
            );
        }
        if self.disabled {
            root = root.opacity(Opacity::Disabled.alpha());
        } else if let Some(handler) = self.activate {
            root = root.on_activate_event(handler);
        }
        let element = root
            .build()
            .with_key(self.id)
            .role(Role::Button)
            .aria_label(self.label)
            .gpui_keyboard_activation()
            .aria_selected(self.selected)
            .disabled(self.disabled)
            .focusable(!self.disabled);
        if self.disabled {
            element.aria_description("Unavailable")
        } else {
            element
                .cursor(CursorIcon::Pointer)
                .focus_visible_style(Style::new().border_of(1.0, t.colors.ring.to_paint()))
                .hover_style(Hover::Neutral.style(&t))
        }
    }
}
