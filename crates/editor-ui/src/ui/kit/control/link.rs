use super::focus_ring;
use crate::ui::kit::tokens::TextRole;
use crate::ui::theme::ArtisanTheme;
use aqua_core::{CursorIcon, Role};
use aqua_text::overflow::OverflowMode;
use aqua_ui::element::{Button, Element, Key, Text};
use aqua_ui::style::{Length, Style};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum LinkTone {
    Link,
    Crumb,
}

pub struct Link {
    key: Key,
    theme: ArtisanTheme,
    tone: LinkTone,
    text: String,
    current: bool,
    on_activate: Option<Box<dyn Fn()>>,
}

impl Link {
    pub fn new(
        key: impl Into<Key>,
        theme: ArtisanTheme,
        tone: LinkTone,
        text: impl Into<String>,
    ) -> Self {
        Self {
            key: key.into(),
            theme,
            tone,
            text: text.into(),
            current: false,
            on_activate: None,
        }
    }

    #[must_use]
    pub const fn current(mut self, current: bool) -> Self {
        self.current = current;
        self
    }

    #[must_use]
    pub fn on_activate(mut self, handler: impl Fn() + 'static) -> Self {
        self.on_activate = Some(Box::new(handler));
        self
    }

    #[must_use]
    pub fn build(self) -> Element {
        let theme = self.theme;
        let (rest, lit) = match (self.tone, self.current) {
            (LinkTone::Link, _) => (theme.colors.link, theme.colors.text),
            (LinkTone::Crumb, true) => (theme.colors.text, theme.colors.text),
            (LinkTone::Crumb, false) => (theme.colors.text_muted, theme.colors.text),
        };
        let mut button = Button::new()
            .style(
                Style::new()
                    .min_width(Length::Px(0.0))
                    .flex_shrink(1.0)
                    .gpui_source_text_color(rest.to_reference_text_color()),
            )
            .child(
                Text::new(self.text.clone())
                    .text_style(TextRole::Label.style(&theme))
                    .overflow(OverflowMode::EndEllipsis)
                    .style(Style::new().min_width(Length::Px(0.0)).text_wrap(false))
                    .build(),
            );
        if let Some(handler) = self.on_activate {
            button = button.on_activate(handler);
        }
        button
            .build()
            .with_key(self.key)
            .role(Role::Button)
            .aria_label(self.text)
            .gpui_keyboard_activation()
            .focusable(true)
            .cursor(CursorIcon::Pointer)
            .focus_visible_style(focus_ring(&theme))
            .hover_style(Style::new().gpui_source_text_color(lit.to_reference_text_color()))
    }
}
