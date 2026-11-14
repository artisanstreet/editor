use super::focus_ring;
use crate::ui::asset::source_asset_glyph;
use crate::ui::chrome::card::{POLAR_INK, Surfaces, key_face, polar_face};
use crate::ui::kit::tokens::{IconSize, Radius};
use crate::ui::motion::MotionPolicy;
use crate::ui::theme::{ArtisanTheme, Opacity};
use aqua_core::{CursorIcon, Role, Transform2D};
use aqua_ui::element::{Box as UiBox, Button, Canvas, Element, Key as NodeKey};
use aqua_ui::style::{Length, Style};
use aqua_ui::tree::ActionEvent;
use aqua_ui::{AlignItems, JustifyContent};
use editor_assets::AssetId;

pub const PRESS: f32 = 1.5;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum KeySize {
    Md,
    Lg,
}

impl KeySize {
    #[must_use]
    pub const fn edge(self) -> f32 {
        match self {
            Self::Md => 32.0,
            Self::Lg => 36.0,
        }
    }
}

pub enum KeyFace {
    Glyph(AssetId),
    Content(Element),
}

pub struct Key {
    key: NodeKey,
    theme: ArtisanTheme,
    label: String,
    size: KeySize,
    face: KeyFace,
    wide: bool,
    primary: bool,
    disabled: bool,
    motion: MotionPolicy,
    on_activate: Option<Box<dyn Fn(ActionEvent)>>,
}

impl Key {
    pub fn new(
        key: impl Into<NodeKey>,
        theme: ArtisanTheme,
        label: impl Into<String>,
        size: KeySize,
        face: KeyFace,
    ) -> Self {
        Self {
            key: key.into(),
            theme,
            label: label.into(),
            size,
            face,
            wide: false,
            primary: false,
            disabled: false,
            motion: MotionPolicy::Full,
            on_activate: None,
        }
    }

    #[must_use]
    pub const fn wide(mut self) -> Self {
        self.wide = true;
        self
    }

    #[must_use]
    pub const fn primary(mut self) -> Self {
        self.primary = true;
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
    pub fn build(self) -> Element {
        let theme = self.theme;
        let edge = self.size.edge();
        let radius = Radius::Field.px();
        let surfaces = Surfaces::of(&theme);
        let width = if self.wide {
            Length::Pct(100.0)
        } else {
            Length::Px(edge)
        };
        let ink = match self.primary {
            true => POLAR_INK,
            false => surfaces.ink,
        };
        let content = match self.face {
            KeyFace::Glyph(icon) => {
                source_asset_glyph(icon, IconSize::Md.px(), ink.to_reference_paint())
            }
            KeyFace::Content(element) => element,
        };
        let face = match self.primary {
            true => Canvas::new()
                .style(Style::new().width(Length::Pct(100.0)).height(Length::Pct(100.0)))
                .on_paint(move |bounds| vec![polar_face(bounds, 0.0, radius)])
                .build(),
            false => key_face(&surfaces, radius, edge),
        };
        let mut button = Button::new()
            .style(
                Style::new()
                    .width(width)
                    .height(Length::Px(edge))
                    .flex_shrink(0.0)
                    .align(AlignItems::CENTER)
                    .justify(JustifyContent::CENTER)
                    .radius(Length::Px(radius)),
            )
            .child(
                UiBox::new()
                    .style(Style::new().absolute_at(Some(0.0), Some(0.0), Some(0.0), Some(0.0)))
                    .child(face)
                    .build()
                    .exclude_from_hit_testing(),
            )
            .child(content);
        if self.disabled {
            button = button.opacity(Opacity::Disabled.alpha());
        } else if let Some(handler) = self.on_activate {
            button = button.on_activate_event(handler);
        }
        let element = button
            .build()
            .with_key(self.key)
            .role(Role::Button)
            .aria_label(self.label)
            .gpui_keyboard_activation()
            .disabled(self.disabled)
            .focusable(!self.disabled);
        if self.disabled {
            return element.aria_description("Unavailable");
        }
        let element = element
            .cursor(CursorIcon::Pointer)
            .focus_visible_style(focus_ring(&theme));
        match self.motion {
            MotionPolicy::Full => {
                element.pressed_style(Style::new().transform(Transform2D::translate(0.0, PRESS)))
            }
            MotionPolicy::Reduced => element,
        }
    }
}
