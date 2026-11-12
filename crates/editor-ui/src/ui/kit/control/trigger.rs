use super::focus_ring;
use crate::ui::asset::source_asset_glyph;
use crate::ui::kit::hover::Hover;
use crate::ui::kit::tokens::{IconSize, Radius, Space, TextRole};
use crate::ui::theme::{ArtisanTheme, Opacity};
use aqua_core::{CursorIcon, Insets, Role};
use aqua_text::overflow::OverflowMode;
use aqua_ui::AlignItems;
use aqua_ui::element::{Box as UiBox, Button, Element, Key, Text};
use aqua_ui::style::{Length, Style};
use editor_assets::AssetId;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TriggerSize {
    Md,
    Sm,
}

impl TriggerSize {
    #[must_use]
    pub const fn height(self) -> f32 {
        match self {
            Self::Md => 32.0,
            Self::Sm => 28.0,
        }
    }
}

pub enum Lead {
    Glyph(AssetId),
    Avatar(Element),
}

pub struct Trigger {
    key: Key,
    theme: ArtisanTheme,
    size: TriggerSize,
    label: String,
    detail: Option<String>,
    lead: Option<Lead>,
    open: bool,
    disabled: bool,
    on_activate: Option<Box<dyn Fn()>>,
}

impl Trigger {
    pub fn new(
        key: impl Into<Key>,
        theme: ArtisanTheme,
        size: TriggerSize,
        label: impl Into<String>,
    ) -> Self {
        Self {
            key: key.into(),
            theme,
            size,
            label: label.into(),
            detail: None,
            lead: None,
            open: false,
            disabled: false,
            on_activate: None,
        }
    }

    #[must_use]
    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    #[must_use]
    pub fn lead(mut self, lead: Lead) -> Self {
        self.lead = Some(lead);
        self
    }

    #[must_use]
    pub const fn open(mut self, open: bool) -> Self {
        self.open = open;
        self
    }

    #[must_use]
    pub const fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    #[must_use]
    pub fn on_activate(mut self, handler: impl Fn() + 'static) -> Self {
        self.on_activate = Some(Box::new(handler));
        self
    }

    fn text(theme: &ArtisanTheme, words: String, muted: bool) -> Element {
        let color = if muted {
            theme.colors.text_muted
        } else {
            theme.colors.text
        };
        Text::new(words)
            .text_style(TextRole::Label.style(theme))
            .overflow(OverflowMode::EndEllipsis)
            .style(Style::new().min_width(Length::Px(0.0)).text_wrap(false))
            .gpui_source_color(color.to_reference_text_color())
            .build()
    }

    #[must_use]
    pub fn build(self) -> Element {
        let theme = self.theme;
        let pad = Space::S8.px();
        let mut button = Button::new().gap(Space::S8.px()).style(
            Style::new()
                .height(Length::Px(self.size.height()))
                .min_width(Length::Px(0.0))
                .align(AlignItems::CENTER)
                .padding_insets(Insets {
                    left: pad,
                    right: pad,
                    top: 0.0,
                    bottom: 0.0,
                })
                .radius(Length::Px(Radius::Field.px())),
        );
        if self.open {
            button = button.style(Hover::Neutral.style(&theme));
        }
        match self.lead {
            Some(Lead::Glyph(icon)) => {
                button = button.child(source_asset_glyph(
                    icon,
                    IconSize::Md.px(),
                    theme.colors.text.to_reference_paint(),
                ));
            }
            Some(Lead::Avatar(avatar)) => button = button.child(avatar),
            None => {}
        }
        let mut words = UiBox::new()
            .gap(Space::S6.px())
            .style(Style::new().min_width(Length::Px(0.0)).flex_shrink(1.0).align(AlignItems::CENTER))
            .child(Self::text(&theme, self.label.clone(), false));
        if let Some(detail) = self.detail {
            words = words.child(Self::text(&theme, detail, true));
        }
        button = button.child(words.build()).child(source_asset_glyph(
            AssetId::TABLER_SELECTOR,
            IconSize::Sm.px(),
            theme.colors.text_muted.to_reference_paint(),
        ));
        if self.disabled {
            button = button.opacity(Opacity::Disabled.alpha());
        } else if let Some(handler) = self.on_activate {
            button = button.on_activate(handler);
        }
        let element = button
            .build()
            .with_key(self.key)
            .role(Role::ComboBox)
            .aria_label(self.label)
            .aria_expanded(self.open)
            .gpui_keyboard_activation()
            .disabled(self.disabled)
            .focusable(!self.disabled);
        if self.disabled {
            return element;
        }
        element
            .cursor(CursorIcon::Pointer)
            .focus_visible_style(focus_ring(&theme))
            .hover_style(Hover::Neutral.style(&theme))
    }
}
