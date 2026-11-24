use aqua_core::{CursorIcon, Insets};
use aqua_ui::element::{Box as UiBox, Button as UiButton, Element, Key};
use aqua_ui::style::{Length, Style};
use aqua_ui::tree::ActionEvent;
use aqua_ui::{AlignItems, JustifyContent};

use super::face::Face;
use super::glyph::{Icon, glyph};
use super::{Carve, RADIUS_SM, Type, inner_shadow};
use crate::ui::kit::control::{Activation, activatable, pressable};
use crate::ui::kit::hover::{Hover, live};
use crate::ui::theme::{ArtisanTheme, Oklch};

pub const HEIGHT: f32 = 36.0;
const PAD_X: f32 = 14.0;
const QUIET_PAD_X: f32 = 12.0;
const PRESS: f32 = 1.5;
const LABEL: Type = Type::sans(14.0, 14.0, 500);

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Tone {
    Horizon,
    Secondary,
    Danger,
    Quiet,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Content {
    Label(String),
    IconLabel(Icon, String),
    Icon(Icon, String),
}

impl Content {
    fn name(&self) -> &str {
        match self {
            Self::Label(name) | Self::IconLabel(_, name) | Self::Icon(_, name) => name,
        }
    }
}

pub struct CarvedButton {
    key: Key,
    theme: ArtisanTheme,
    tone: Tone,
    content: Content,
    disabled: bool,
    width: Option<f32>,
    height: f32,
    radius: f32,
    lift: f32,
    on_activate: Option<Box<dyn Fn(ActionEvent)>>,
}

impl std::fmt::Debug for CarvedButton {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CarvedButton").finish_non_exhaustive()
    }
}

impl CarvedButton {
    #[expect(
        clippy::large_types_passed_by_value,
        reason = "the button stores this theme copy and paints from it when built"
    )]
    pub fn new(key: impl Into<Key>, theme: ArtisanTheme, tone: Tone, content: Content) -> Self {
        let width = matches!(content, Content::Icon(..)).then_some(HEIGHT);
        Self {
            key: key.into(),
            theme,
            tone,
            content,
            disabled: false,
            width,
            height: HEIGHT,
            radius: RADIUS_SM,
            lift: 0.0,
            on_activate: None,
        }
    }

    #[expect(
        clippy::large_types_passed_by_value,
        reason = "the button stores this theme copy and paints from it when built"
    )]
    pub fn label(
        key: impl Into<Key>,
        theme: ArtisanTheme,
        tone: Tone,
        label: impl Into<String>,
    ) -> Self {
        Self::new(key, theme, tone, Content::Label(label.into()))
    }

    #[must_use]
    pub const fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    #[must_use]
    pub const fn size(mut self, width: f32, height: f32) -> Self {
        self.width = Some(width);
        self.height = height;
        self
    }

    #[must_use]
    pub const fn lift(mut self, top: f32) -> Self {
        self.lift = top;
        self
    }

    #[must_use]
    pub const fn radius(mut self, radius: f32) -> Self {
        self.radius = radius;
        self
    }

    #[must_use]
    pub fn on_activate(mut self, handler: impl Fn(ActionEvent) + 'static) -> Self {
        self.on_activate = Some(Box::new(handler));
        self
    }

    fn ink(&self, carve: &Carve) -> Oklch {
        match (self.tone, &self.content) {
            (Tone::Horizon | Tone::Danger, _) => super::white(1.0),
            (Tone::Secondary, Content::Icon(..)) | (Tone::Quiet, _) => carve.ink_dim,
            (Tone::Secondary, _) if self.disabled => carve.ink_dim,
            (Tone::Secondary, _) => carve.ink,
        }
    }

    fn face(&self, carve: &Carve) -> Option<Face> {
        match self.tone {
            Tone::Horizon => Some(Face::horizon(self.radius)),
            Tone::Danger => Some(Face::ember(self.radius)),
            Tone::Secondary => Some(Face::secondary(carve, self.radius, false)),
            Tone::Quiet => None,
        }
    }

    fn shadows(&self, carve: &Carve) -> Vec<aqua::ShadowLayer> {
        match self.tone {
            Tone::Horizon | Tone::Danger => {
                let mut shadows = carve.rest();
                shadows.extend(carve.press());
                shadows
            }
            Tone::Secondary => carve.rest(),
            Tone::Quiet => Vec::new(),
        }
    }

    fn sheen(&self, carve: &Carve) -> Option<Element> {
        (self.tone == Tone::Secondary).then(|| {
            UiBox::new()
                .style(
                    Style::new()
                        .absolute_at(Some(1.0), Some(1.0), Some(1.0), Some(1.0))
                        .radius(Length::Px(self.radius - 1.0))
                        .shadows(vec![inner_shadow(
                            0.0,
                            2.0,
                            4.0,
                            0.0,
                            carve.tint.with_alpha(0.10),
                        )]),
                )
                .build()
                .exclude_from_hit_testing()
        })
    }

    fn children(&self, carve: &Carve) -> Vec<Element> {
        let ink = self.ink(carve);
        let words = |name: &str| {
            LABEL
                .text(&self.theme, ink, name.to_owned())
                .wrap(false)
                .build()
        };
        match &self.content {
            Content::Label(name) => vec![words(name)],
            Content::IconLabel(icon, name) => vec![glyph(*icon, 15.0, ink), words(name)],
            Content::Icon(icon, _) => vec![glyph(
                *icon,
                if self.height < 36.0 { 16.0 } else { 15.0 },
                ink,
            )],
        }
    }

    #[must_use]
    pub fn build(self) -> Element {
        let carve = Carve::of(&self.theme);
        let icon_only = matches!(self.content, Content::Icon(..));
        let pad = match (icon_only, self.tone) {
            (true, _) => 0.0,
            (false, Tone::Quiet) => QUIET_PAD_X,
            (false, _) => PAD_X,
        };
        let mut style = Style::new()
            .height(Length::Px(self.height))
            .align(AlignItems::CENTER)
            .justify(JustifyContent::CENTER)
            .flex_shrink(0.0)
            .padding_insets(Insets {
                left: pad,
                right: pad,
                top: self.lift,
                bottom: if icon_only { 0.0 } else { 1.0 },
            })
            .radius(Length::Px(self.radius))
            .shadows(self.shadows(&carve));
        if let Some(width) = self.width {
            style = style.width(Length::Px(width));
        }
        let mut root = UiButton::new().gap(8.0).style(style);
        if let Some(face) = self.face(&carve) {
            root = root.child(face.layer());
        }
        if let Some(sheen) = self.sheen(&carve) {
            root = root.child(sheen);
        }
        let fills = self.tone == Tone::Quiet && !self.disabled;
        if fills {
            root = root.child(live::layer(Hover::Neutral, &self.theme, self.radius, false));
        }
        for child in self.children(&carve) {
            root = root.child(child);
        }
        let theme = self.theme;
        let quiet_hover = Style::new().gpui_source_text_color(carve.ink.to_reference_text_color());
        let tone = self.tone;
        let activation = Activation {
            key: self.key,
            label: self.content.name().to_owned(),
            disabled: self.disabled,
            on_activate: self.on_activate,
            debug_selector: None,
        };
        activatable(root, activation, move |element| {
            let element = element
                .cursor(CursorIcon::Pointer)
                .focus_visible_style(focus(&theme));
            let element = match tone {
                Tone::Quiet => live::track(element.hover_style(quiet_hover)),
                Tone::Horizon | Tone::Danger => element.hover_style(Style::new().opacity(0.96)),
                Tone::Secondary => element,
            };
            pressable(element, &theme, PRESS)
        })
    }
}

#[must_use]
pub fn focus(theme: &ArtisanTheme) -> Style {
    let ring = Oklch::new(0.707, 0.165, 254.624);
    let ring = if theme.colors.mode.is_dark() {
        ring
    } else {
        Oklch::new(0.623, 0.214, 259.815)
    };
    Style::new().shadows(vec![
        super::drop_shadow(0.0, 0.0, 2.0, super::white(0.0)),
        super::drop_shadow(0.0, 0.0, 4.0, ring),
    ])
}
