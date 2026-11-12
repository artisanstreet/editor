use crate::ui::kit::hover::{Highlight, Hover};
use crate::ui::kit::surface::Surface;
use crate::ui::kit::tokens::{IconSize, Space, TextRole, concentric};
use crate::ui::theme::{ArtisanTheme, Oklch, Opacity};
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
    pub const fn interactive(self) -> bool {
        !matches!(self, Self::Heading | Self::FormRow)
    }

    #[must_use]
    pub const fn wraps(self) -> bool {
        matches!(self, Self::FormRow)
    }

    #[must_use]
    pub const fn gap(self) -> Space {
        match self {
            Self::FormRow => Space::S16,
            _ => Space::S8,
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
    body: Option<Element>,
    trailing: Option<Element>,
    lit: Option<(Highlight, Instant)>,
    underlay: Option<Option<Element>>,
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
            body: None,
            trailing: None,
            lit: None,
            underlay: None,
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
    pub fn body(mut self, element: Element) -> Self {
        self.body = Some(element);
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
    pub fn underlay(mut self, layer: Option<Element>) -> Self {
        self.underlay = Some(layer);
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

    fn text(theme: &ArtisanTheme, words: String, role: TextRole, color: Oklch, wraps: bool) -> Element {
        let text = Text::new(words)
            .text_style(role.style(theme))
            .gpui_source_color(color.to_reference_text_color());
        match wraps {
            true => text.wrap(true).build(),
            false => text
                .overflow(OverflowMode::EndEllipsis)
                .style(Style::new().min_width(Length::Px(0.0)).text_wrap(false))
                .build(),
        }
    }

    fn style(&self) -> Style {
        let pad = self.row.pad();
        let style = Style::new()
            .width(Length::Pct(100.0))
            .align(AlignItems::CENTER)
            .padding_insets(Insets {
                left: pad.x.px(),
                right: pad.x.px(),
                top: pad.y.px(),
                bottom: pad.y.px(),
            })
            .radius(Length::Px(self.radius));
        match self.row.height() {
            Some(height) => style.height(Length::Px(height)),
            None => style,
        }
    }

    fn leading_slot(leading: Element) -> Element {
        let edge = IconSize::Md.px();
        UiBox::new()
            .width(edge)
            .height(edge)
            .style(Style::new().flex_shrink(0.0).align(AlignItems::CENTER))
            .child(leading)
            .build()
    }

    fn ink(&self) -> Oklch {
        match (self.row, self.hover) {
            (Row::Heading, _) => self.theme.colors.text_muted,
            (_, Hover::Destructive) => self.theme.colors.destructive_text,
            (_, Hover::Neutral) => self.theme.colors.text,
        }
    }

    fn label(&mut self) -> Element {
        let grow = Style::new().flex_grow(1.0).min_width(Length::Px(0.0));
        if let Some(body) = self.body.take() {
            return UiBox::new().style(grow).child(body).build();
        }
        let wraps = self.row.wraps();
        let title = Self::text(
            &self.theme,
            self.title.clone(),
            self.row.title(),
            self.ink(),
            wraps,
        );
        match self.detail.clone() {
            Some(detail) => UiBox::new()
                .layout("column")
                .style(grow)
                .child(title)
                .child(Self::text(
                    &self.theme,
                    detail,
                    TextRole::Caption,
                    self.theme.colors.text_muted,
                    wraps,
                ))
                .build(),
            None => UiBox::new().style(grow).child(title).build(),
        }
    }

    fn build_static(mut self) -> Element {
        let gap = self.row.gap().px();
        let mut row = UiBox::new().gap(gap).style(self.style());
        if let Some(leading) = self.leading.take() {
            row = row.child(Self::leading_slot(leading));
        }
        row = row.child(self.label());
        if self.disabled {
            row = row.opacity(Opacity::Disabled.alpha());
        }
        if let Some(trailing) = self.trailing {
            row = row.child(
                UiBox::new()
                    .style(Style::new().flex_shrink(0.0))
                    .child(trailing)
                    .build(),
            );
        }
        let element = row.build().with_key(self.key).aria_label(self.title);
        element.disabled(self.disabled)
    }

    #[must_use]
    pub fn build(mut self) -> Element {
        if !self.row.interactive() {
            return self.build_static();
        }
        let height = self.row.height();
        let mut button = Button::new().gap(self.row.gap().px()).style(self.style());
        let external = self.underlay.take();
        let driven = external.is_some();
        if let Some(Some(layer)) = external {
            button = button.child(layer);
        }
        let lit = self.lit.filter(|_| !driven);
        button = match (lit, self.selected) {
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
        if let Some(leading) = self.leading.take() {
            button = button.child(Self::leading_slot(leading));
        }
        button = button.child(self.label());
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
        let element = element
            .role(self.role)
            .aria_selected(self.selected)
            .focusable(!self.disabled);
        let element = match (self.lit.is_some() || driven, self.disabled) {
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
