pub mod special;

use crate::ui::asset::source_asset_glyph;
use crate::ui::chrome::card::{POLAR_INK, polar_face, soft};
use crate::ui::kit::hover::{Hover, live};
use crate::ui::kit::surface::Surface;
use crate::ui::kit::tokens::{Elevation, IconSize, Radius, Space, TextRole, concentric};
use crate::ui::motion::MotionPolicy;
use crate::ui::theme::{ArtisanTheme, Cast, Oklch, Opacity};
use aqua::ShadowLayer;
use aqua_core::{CursorIcon, DisplayItem, Insets, Role, Transform2D};
use aqua_text::overflow::OverflowMode;
use aqua_ui::element::{Button as UiButton, Canvas, Element, Key, Text};
use aqua_ui::style::{Length, Style};
use aqua_ui::tree::ActionEvent;
use aqua_ui::{AlignItems, JustifyContent};
use editor_assets::AssetId;
use std::sync::Arc;

const PRESS: f32 = 1.5;
const WHITE: Oklch = Oklch::new(1.0, 0.0, 0.0);

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ButtonTone {
    Primary,
    Special,
    Neutral,
    Quiet,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ButtonSize {
    Md,
    Sm,
}

impl ButtonSize {
    #[must_use]
    pub const fn height(self) -> f32 {
        match self {
            Self::Md => 32.0,
            Self::Sm => 28.0,
        }
    }

    #[must_use]
    pub const fn pad(self) -> Space {
        match self {
            Self::Md => Space::S12,
            Self::Sm => Space::S8,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Ellipsis {
    Start,
    End,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Label {
    Text(String),
    IconText(AssetId, String),
}

impl Label {
    fn words(&self) -> &str {
        match self {
            Self::Text(words) | Self::IconText(_, words) => words,
        }
    }
}

pub struct Button {
    key: Key,
    theme: ArtisanTheme,
    tone: ButtonTone,
    size: ButtonSize,
    label: Label,
    radius: f32,
    ellipsis: Option<Ellipsis>,
    disabled: bool,
    hover: Hover,
    motion: MotionPolicy,
    on_activate: Option<Box<dyn Fn(ActionEvent)>>,
    debug_selector: Option<String>,
}

impl Button {
    pub fn new(
        key: impl Into<Key>,
        theme: ArtisanTheme,
        tone: ButtonTone,
        size: ButtonSize,
        label: Label,
    ) -> Self {
        Self {
            key: key.into(),
            theme,
            tone,
            size,
            label,
            radius: Radius::Field.px(),
            ellipsis: None,
            disabled: false,
            hover: Hover::Neutral,
            motion: MotionPolicy::Full,
            on_activate: None,
            debug_selector: None,
        }
    }

    pub fn text(
        key: impl Into<Key>,
        theme: ArtisanTheme,
        tone: ButtonTone,
        label: impl Into<String>,
    ) -> Self {
        Self::new(key, theme, tone, ButtonSize::Md, Label::Text(label.into()))
    }

    #[must_use]
    pub const fn destructive(mut self) -> Self {
        self.hover = Hover::Destructive;
        self
    }

    #[must_use]
    pub const fn size(mut self, size: ButtonSize) -> Self {
        self.size = size;
        self
    }

    #[must_use]
    pub fn within(mut self, surface: Surface) -> Self {
        self.radius = surface.child_radius().map_or_else(
            || concentric(surface.radius_px(), surface.padding_px()),
            |radius| radius.px(),
        );
        self
    }

    #[must_use]
    pub fn nested(mut self, outer_radius: f32, inset: f32) -> Self {
        self.radius = concentric(outer_radius, inset);
        self
    }

    #[must_use]
    pub const fn ellipsis(mut self, ellipsis: Ellipsis) -> Self {
        self.ellipsis = Some(ellipsis);
        self
    }

    #[must_use]
    pub const fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    #[must_use]
    pub const fn motion_policy(mut self, motion: MotionPolicy) -> Self {
        self.motion = motion;
        self
    }

    #[must_use]
    pub fn on_activate(mut self, handler: impl Fn(ActionEvent) + 'static) -> Self {
        self.on_activate = Some(Box::new(handler));
        self
    }

    #[must_use]
    pub fn debug_selector(mut self, selector: impl Into<String>) -> Self {
        self.debug_selector = Some(selector.into());
        self
    }

    fn ink(&self) -> Oklch {
        match self.tone {
            ButtonTone::Primary => POLAR_INK,
            ButtonTone::Special => WHITE,
            ButtonTone::Neutral | ButtonTone::Quiet => self.theme.colors.text,
        }
    }

    fn press(&self) -> Vec<ShadowLayer> {
        soft(&self.theme, Cast::PRESS)
    }

    fn face(&self) -> Option<Element> {
        let radius = self.radius;
        let paint = match self.tone {
            ButtonTone::Primary => {
                Canvas::new().on_paint(move |bounds| vec![polar_face(bounds, 0.0, radius)])
            }
            ButtonTone::Special => Canvas::new().on_paint(move |bounds| {
                vec![DisplayItem::GpuiSvg {
                    bounds,
                    source: Arc::<str>::from(special::svg(bounds.w, bounds.h, radius, 0.0)),
                    tint: None,
                    opacity: 1.0,
                }]
            }),
            ButtonTone::Neutral | ButtonTone::Quiet => return None,
        };
        Some(
            paint
                .style(Style::new().absolute_at(Some(0.0), Some(0.0), Some(0.0), Some(0.0)))
                .scroll_stable()
                .build()
                .exclude_from_hit_testing(),
        )
    }

    fn content(&self) -> Vec<Element> {
        let ink = self.ink();
        let mut text = Text::new(self.label.words().to_owned())
            .text_style(TextRole::LabelStrong.style(&self.theme))
            .gpui_source_color(ink.to_reference_text_color());
        if let Some(ellipsis) = self.ellipsis {
            text = text
                .overflow(match ellipsis {
                    Ellipsis::Start => OverflowMode::StartEllipsis,
                    Ellipsis::End => OverflowMode::EndEllipsis,
                })
                .style(Style::new().flex_shrink(1.0).min_width(Length::Px(0.0)));
        }
        let text = text.build();
        match &self.label {
            Label::Text(_) => vec![text],
            Label::IconText(icon, _) => vec![
                source_asset_glyph(*icon, IconSize::Md.px(), ink.to_reference_paint())
                    .style(Style::new().flex_shrink(0.0)),
                text,
            ],
        }
    }

    #[must_use]
    pub fn build(self) -> Element {
        let theme = self.theme;
        let height = self.size.height();
        let pad = self.size.pad().px();
        let mut style = Style::new()
            .height(Length::Px(height))
            .align(AlignItems::CENTER)
            .justify(JustifyContent::CENTER)
            .flex_shrink(if self.ellipsis.is_some() { 1.0 } else { 0.0 })
            .padding_insets(Insets {
                left: pad,
                right: pad,
                top: 0.0,
                bottom: 0.0,
            })
            .radius(Length::Px(self.radius));
        if self.ellipsis.is_some() {
            style = style
                .min_width(Length::Px(0.0))
                .max_width(Length::Pct(100.0));
        }
        let shadows = match self.tone {
            ButtonTone::Primary => self.press(),
            ButtonTone::Special => Elevation::Rest.shadows(&theme),
            ButtonTone::Neutral | ButtonTone::Quiet => Vec::new(),
        };
        let mut root = UiButton::new()
            .gap(Space::S6.px())
            .style(style.shadows(shadows));
        if self.tone == ButtonTone::Neutral {
            root = root
                .fill(theme.colors.surface.to_paint())
                .border((1.0, theme.colors.line.to_paint()));
        }
        if let Some(face) = self.face() {
            root = root.child(face);
        }
        let fills = matches!(self.tone, ButtonTone::Neutral | ButtonTone::Quiet) && !self.disabled;
        if fills {
            root = root.child(live::layer(self.hover, &theme, self.radius, false));
        }
        for child in self.content() {
            root = root.child(child);
        }
        let label = self.label.words().to_owned();
        let tone = self.tone;
        let lift = soft(&theme, Cast::LIFT);
        if self.disabled {
            root = root.opacity(Opacity::Disabled.alpha());
        } else if let Some(handler) = self.on_activate {
            root = root.on_activate_event(handler);
        }
        let element = root
            .build()
            .with_key(self.key)
            .role(Role::Button)
            .aria_label(label)
            .gpui_keyboard_activation()
            .disabled(self.disabled)
            .focusable(!self.disabled);
        let element = match self.debug_selector {
            Some(selector) => element.debug_selector(selector),
            None => element,
        };
        if self.disabled {
            return element.aria_description("Unavailable");
        }
        let focus = Style::new()
            .border_of(1.0, theme.colors.ring.to_paint())
            .shadows(vec![ShadowLayer {
                dx: 0.0,
                dy: 0.0,
                blur: 0.0,
                spread: theme.interaction.focus_ring_width,
                inset: false,
                color: theme.colors.ring.at(Opacity::FocusRing).to_paint(),
            }]);
        let hover = match tone {
            ButtonTone::Neutral | ButtonTone::Quiet => Style::new(),
            ButtonTone::Primary => Style::new().shadows(lift),
            ButtonTone::Special => Style::new().opacity(0.94),
        };
        let element = element
            .cursor(CursorIcon::Pointer)
            .focus_visible_style(focus)
            .hover_style(hover);
        let element = match fills {
            true => live::track(element),
            false => element,
        };
        match self.motion {
            MotionPolicy::Full => {
                element.pressed_style(Style::new().transform(Transform2D::translate(0.0, PRESS)))
            }
            MotionPolicy::Reduced => element,
        }
    }
}
