use crate::ui::kit::hover::{Highlight, Hover};
use crate::ui::kit::surface::Surface;
use crate::ui::kit::tokens::{IconSize, Space, TextRole, concentric};
use crate::ui::theme::{ArtisanTheme, Opacity};
use aqua_core::{CursorIcon, Insets, Role};
use aqua_text::overflow::OverflowMode;
use aqua_ui::AlignItems;
use aqua_ui::element::{Box as UiBox, Button, Element, Key, Text};
use aqua_ui::style::{Length, Style};
use std::time::Instant;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Density {
    Regular,
    Dense,
    Tall,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Row {
    MenuItem,
    ListItem(Density),
    Heading,
    FormRow,
    KeyRow,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Pad {
    pub x: Space,
    pub y: Space,
}

impl Row {
    #[must_use]
    pub const fn pad(self) -> Pad {
        let (x, y) = match self {
            Self::MenuItem | Self::KeyRow => (Space::S12, Space::S8),
            Self::ListItem(Density::Regular | Density::Tall) => (Space::S8, Space::S8),
            Self::ListItem(Density::Dense) => (Space::S4, Space::S4),
            Self::Heading => (Space::S12, Space::S6),
            Self::FormRow => (Space::S12, Space::S12),
        };
        Pad { x, y }
    }

    #[must_use]
    pub const fn title(self) -> TextRole {
        match self {
            Self::Heading => TextRole::Caption,
            _ => TextRole::Label,
        }
    }

    #[must_use]
    pub const fn lines(self) -> f32 {
        match self {
            Self::ListItem(Density::Tall) => {
                TextRole::Label.spec().line + TextRole::Caption.spec().line
            }
            _ => self.title().spec().line,
        }
    }

    #[must_use]
    pub const fn height(self) -> Option<f32> {
        match self {
            Self::FormRow => None,
            _ => Some(self.lines() + 2.0 * self.pad().y.px()),
        }
    }

    #[must_use]
    pub fn radius_in(self, surface: Surface) -> f32 {
        surface.child_radius().map_or_else(
            || concentric(surface.radius_px(), surface.padding_px()),
            |radius| radius.px(),
        )
    }

    #[must_use]
    pub fn text_inset(self, surface: Surface) -> f32 {
        surface.padding_px() + self.pad().x.px()
    }
}

pub struct RowItem {
    key: Key,
    theme: ArtisanTheme,
    row: Row,
    radius: f32,
    hover: Hover,
    leading: Option<Element>,
    title: String,
    detail: Option<String>,
    trailing: Option<Element>,
    lit: Option<(Highlight, Instant)>,
    selected: bool,
    disabled: bool,
    role: Role,
    on_activate: Option<Box<dyn Fn()>>,
    on_hover: Option<Box<dyn Fn(bool)>>,
}

impl RowItem {
    pub fn new(
        key: impl Into<Key>,
        theme: ArtisanTheme,
        row: Row,
        surface: Surface,
        title: impl Into<String>,
    ) -> Self {
        Self {
            key: key.into(),
            theme,
            row,
            radius: row.radius_in(surface),
            hover: Hover::Neutral,
            leading: None,
            title: title.into(),
            detail: None,
            trailing: None,
            lit: None,
            selected: false,
            disabled: false,
            role: match row {
                Row::MenuItem => Role::MenuItem,
                _ => Role::Button,
            },
            on_activate: None,
            on_hover: None,
        }
    }

    #[must_use]
    pub fn nested(mut self, outer_radius: f32, inset: f32) -> Self {
        self.radius = concentric(outer_radius, inset);
        self
    }

    #[must_use]
    pub const fn destructive(mut self) -> Self {
        self.hover = Hover::Destructive;
        self
    }

    #[must_use]
    pub fn leading(mut self, element: Element) -> Self {
        self.leading = Some(element);
        self
    }

    #[must_use]
    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    #[must_use]
    pub fn trailing(mut self, element: Element) -> Self {
        self.trailing = Some(element);
        self
    }

    #[must_use]
    pub const fn lit(mut self, highlight: Highlight, now: Instant) -> Self {
        self.lit = Some((highlight, now));
        self
    }

    #[must_use]
    pub const fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    #[must_use]
    pub const fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    #[must_use]
    pub const fn role(mut self, role: Role) -> Self {
        self.role = role;
        self
    }

    #[must_use]
    pub fn on_activate(mut self, handler: impl Fn() + 'static) -> Self {
        self.on_activate = Some(Box::new(handler));
        self
    }

    #[must_use]
    pub fn on_hover(mut self, handler: impl Fn(bool) + 'static) -> Self {
        self.on_hover = Some(Box::new(handler));
        self
    }

    fn text(theme: &ArtisanTheme, words: String, role: TextRole, muted: bool) -> Element {
        let color = if muted {
            theme.colors.text_muted
        } else {
            theme.colors.text
        };
        Text::new(words)
            .text_style(role.style(theme))
            .overflow(OverflowMode::EndEllipsis)
            .style(Style::new().min_width(Length::Px(0.0)).text_wrap(false))
            .gpui_source_color(color.to_reference_text_color())
            .build()
    }

    #[must_use]
    pub fn build(self) -> Element {
        let pad = self.row.pad();
        let height = self.row.height();
        let mut style = Style::new()
            .width(Length::Pct(100.0))
            .align(AlignItems::CENTER)
            .padding_insets(Insets {
                left: pad.x.px(),
                right: pad.x.px(),
                top: pad.y.px(),
                bottom: pad.y.px(),
            })
            .radius(Length::Px(self.radius));
        if let Some(height) = height {
            style = style.height(Length::Px(height));
        }
        let mut button = Button::new().gap(Space::S8.px()).style(style);
        let interactive = !matches!(self.row, Row::Heading | Row::FormRow);
        if interactive {
            button = match (self.lit, self.selected) {
                (Some((lit, now)), _) => button.child(self.hover.layer(
                    &self.theme,
                    lit,
                    now,
                    height.unwrap_or_default(),
                    self.radius,
                )),
                (None, true) => button.child(
                    UiBox::new()
                        .style(self.hover.style(&self.theme))
                        .style(
                            Style::new()
                                .absolute_at(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
                                .radius(Length::Px(self.radius)),
                        )
                        .build()
                        .exclude_from_hit_testing(),
                ),
                (None, false) => button,
            };
        }
        if let Some(leading) = self.leading {
            let edge = IconSize::Md.px();
            button = button.child(
                UiBox::new()
                    .width(edge)
                    .height(edge)
                    .style(Style::new().flex_shrink(0.0).align(AlignItems::CENTER))
                    .child(leading)
                    .build(),
            );
        }
        let title = Self::text(
            &self.theme,
            self.title.clone(),
            self.row.title(),
            self.row == Row::Heading,
        );
        let label = match self.detail.clone() {
            Some(detail) => UiBox::new()
                .layout("column")
                .style(Style::new().flex_grow(1.0).min_width(Length::Px(0.0)))
                .child(title)
                .child(Self::text(&self.theme, detail, TextRole::Caption, true))
                .build(),
            None => UiBox::new()
                .style(Style::new().flex_grow(1.0).min_width(Length::Px(0.0)))
                .child(title)
                .build(),
        };
        button = button.child(label);
        if let Some(trailing) = self.trailing {
            button = button.child(trailing);
        }
        if self.disabled {
            button = button.opacity(Opacity::Disabled.alpha());
        } else if let Some(handler) = self.on_activate {
            button = button.on_activate(handler);
        }
        let element = button
            .build()
            .with_key(self.key)
            .aria_label(self.title)
            .disabled(self.disabled);
        if !interactive {
            return element.exclude_from_hit_testing();
        }
        let element = element
            .role(self.role)
            .aria_selected(self.selected)
            .focusable(!self.disabled);
        let element = match (self.lit.is_some(), self.disabled) {
            (_, true) => element,
            (true, false) => element.cursor(CursorIcon::Pointer),
            (false, false) => element
                .cursor(CursorIcon::Pointer)
                .hover_style(self.hover.style(&self.theme)),
        };
        match self.on_hover {
            Some(handler) => element.on_hover_change(handler),
            None => element,
        }
    }
}
