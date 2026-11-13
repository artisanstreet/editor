use super::focus_ring;
use crate::ui::asset::source_asset_glyph;
use crate::ui::kit::hover::{Hover, live};
use crate::ui::kit::surface::Surface;
use crate::ui::kit::tokens::{IconSize, Radius, concentric};
use crate::ui::motion::MotionPolicy;
use crate::ui::theme::{ArtisanTheme, Opacity};
use aqua_core::{CursorIcon, Role, Transform2D};
use aqua_ui::element::{Button, Element, Key};
use aqua_ui::style::{Length, Style};
use aqua_ui::tree::ActionEvent;
use aqua_ui::{AlignItems, JustifyContent};
use editor_assets::AssetId;

pub const CORNER_INSET: f32 = 2.0;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum IconButtonSize {
    Sm,
    Md,
    Lg,
}

impl IconButtonSize {
    #[must_use]
    pub const fn edge(self) -> f32 {
        match self {
            Self::Sm => 24.0,
            Self::Md => 32.0,
            Self::Lg => 36.0,
        }
    }

    #[must_use]
    pub const fn glyph(self) -> IconSize {
        match self {
            Self::Sm => IconSize::Sm,
            Self::Md | Self::Lg => IconSize::Md,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Finish {
    Plain,
    Glass,
}

pub struct IconButton {
    key: Key,
    theme: ArtisanTheme,
    icon: AssetId,
    label: String,
    size: IconButtonSize,
    finish: Finish,
    radius: f32,
    muted: bool,
    selected: bool,
    hover: Hover,
    disabled: bool,
    motion: MotionPolicy,
    on_activate: Option<Box<dyn Fn(ActionEvent)>>,
}

impl IconButton {
    pub fn new(
        key: impl Into<Key>,
        theme: ArtisanTheme,
        icon: AssetId,
        label: impl Into<String>,
        size: IconButtonSize,
        finish: Finish,
    ) -> Self {
        Self {
            key: key.into(),
            theme,
            icon,
            label: label.into(),
            size,
            finish,
            radius: Radius::Field.px(),
            muted: false,
            selected: false,
            hover: Hover::Neutral,
            disabled: false,
            motion: MotionPolicy::Full,
            on_activate: None,
        }
    }

    pub fn corner_close(
        key: impl Into<Key>,
        theme: ArtisanTheme,
        label: impl Into<String>,
        surface_radius: f32,
    ) -> Self {
        Self::new(
            key,
            theme,
            AssetId::TABLER_X,
            label,
            IconButtonSize::Sm,
            Finish::Glass,
        )
        .muted()
        .nested(surface_radius, CORNER_INSET)
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
    pub const fn muted(mut self) -> Self {
        self.muted = true;
        self
    }

    #[must_use]
    pub const fn destructive(mut self) -> Self {
        self.hover = Hover::Destructive;
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
    pub const fn motion_policy(mut self, motion: MotionPolicy) -> Self {
        self.motion = motion;
        self
    }

    #[must_use]
    pub fn on_activate(mut self, handler: impl Fn(ActionEvent) + 'static) -> Self {
        self.on_activate = Some(Box::new(handler));
        self
    }

    fn glass(theme: &ArtisanTheme, radius: f32) -> Element {
        crate::ui::kit::surface::glass::paint(
            theme,
            crate::ui::kit::surface::glass::Shade::Clear,
            radius,
        )
    }

    #[must_use]
    pub fn build(self) -> Element {
        let theme = self.theme;
        let edge = self.size.edge();
        let rest = if self.muted {
            theme.colors.text_muted
        } else {
            theme.colors.text
        };
        let mut button = Button::new().style(
            Style::new()
                .width(Length::Px(edge))
                .height(Length::Px(edge))
                .flex_shrink(0.0)
                .align(AlignItems::CENTER)
                .justify(JustifyContent::CENTER)
                .radius(Length::Px(self.radius))
                .gpui_source_text_color(rest.to_reference_text_color()),
        );
        if self.finish == Finish::Glass {
            button = button.child(Self::glass(&theme, self.radius));
        }
        if !self.disabled {
            button = button.child(live::layer(self.hover, &theme, self.radius, self.selected));
        }
        button = button.child(source_asset_glyph(
            self.icon,
            self.size.glyph().px(),
            rest.to_reference_paint(),
        ));
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
        let hover =
            Style::new().gpui_source_text_color(theme.colors.text.to_reference_text_color());
        let element = live::track(
            element
                .cursor(CursorIcon::Pointer)
                .focus_visible_style(focus_ring(&theme))
                .hover_style(hover),
        );
        match self.motion {
            MotionPolicy::Full => {
                element.pressed_style(Style::new().transform(Transform2D::translate(0.0, 1.0)))
            }
            MotionPolicy::Reduced => element,
        }
    }
}
