use aqua_ui::{
    element::{Box as UiBox, Element},
    style::Style,
};

use super::kit::surface::glass as kit_glass;
use super::kit::{Elevation as KitElevation, Inset, Radius};
use super::theme::{ArtisanTheme, Elevation, Oklch, Opacity, ShadowLayer, ThemeMode};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Gradient {
    pub angle_degrees: f32,
    pub start: Oklch,
    pub end: Oklch,
    pub end_position: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlassShadow {
    pub color: Oklch,
    pub offset_x: f32,
    pub offset_y: f32,
    pub css_blur: f32,
    pub spread: f32,
    pub inset: bool,
}

impl From<GlassShadow> for aqua::ShadowLayer {
    fn from(shadow: GlassShadow) -> Self {
        Self {
            dx: shadow.offset_x,
            dy: shadow.offset_y,
            blur: shadow.css_blur,
            spread: shadow.spread,
            inset: shadow.inset,
            color: shadow.color.to_paint(),
        }
    }
}

impl From<GlassShadow> for aqua_ui::style::GpuiShadowLayer {
    fn from(shadow: GlassShadow) -> Self {
        Self {
            dx: shadow.offset_x,
            dy: shadow.offset_y,
            blur: shadow.css_blur,
            spread: shadow.spread,
            inset: shadow.inset,
            color: shadow.color.to_reference_paint(),
        }
    }
}

pub fn glass_control_fill(theme: &ArtisanTheme) -> Gradient {
    let colors = &theme.colors;
    let (start, end) = match colors.mode {
        ThemeMode::Dark => (colors.hover, colors.background),
        ThemeMode::Light => (colors.background, colors.raised),
    };
    Gradient {
        angle_degrees: 180.0,
        start,
        end,
        end_position: 1.0,
    }
}

fn elevation_shadow(layer: ShadowLayer) -> GlassShadow {
    GlassShadow {
        color: layer.color,
        offset_x: layer.offset_x,
        offset_y: layer.offset_y,
        css_blur: layer.blur_radius,
        spread: layer.spread_radius,
        inset: false,
    }
}

pub fn card_shadows(theme: &ArtisanTheme) -> Vec<GlassShadow> {
    std::iter::once(theme.elevation.hairline)
        .chain(theme.elevation.drops(Elevation::Raised).iter().copied())
        .map(elevation_shadow)
        .collect()
}

pub fn glass_rims(theme: &ArtisanTheme) -> Vec<GlassShadow> {
    let colors = &theme.colors;
    let shade = match colors.mode {
        ThemeMode::Light => colors.black.with_alpha(0.05),
        ThemeMode::Dark => colors.black.at(Opacity::RimShade),
    };
    [
        (colors.white.at(Opacity::RimLight), -1.0, false),
        (colors.white.at(Opacity::RimInner), 1.0, true),
        (shade, -1.0, true),
    ]
    .into_iter()
    .map(|(color, offset_y, inset)| GlassShadow {
        color,
        offset_x: 0.0,
        offset_y,
        css_blur: 0.0,
        spread: 0.0,
        inset,
    })
    .collect()
}

pub fn glass_card_shadows(theme: &ArtisanTheme, elevation: Elevation) -> Vec<GlassShadow> {
    let mut shadows = glass_rims(theme);
    shadows.push(elevation_shadow(theme.elevation.hairline));
    shadows.extend(
        theme
            .elevation
            .drops(elevation)
            .iter()
            .copied()
            .map(elevation_shadow),
    );
    shadows
}

pub const KEY_RADIUS: f32 = Radius::Field.px();
pub const COMPOSER_PADDING: f32 = Inset::Frame.px();
pub const COMPOSER_RADIUS: f32 = Radius::Well.px();
pub const INSET_RADIUS: f32 =
    super::tab_card::CARD_RADIUS - super::tab_card::TAB_CARD_PADDING_PX;

fn inset(theme: &ArtisanTheme, content: Element) -> Element {
    use aqua_ui::style::{Axis, Length};
    let dark = theme.colors.mode.is_dark();
    let (shade, rim, depth) = match dark {
        true => (
            Oklch::new(0.0, 0.0, 0.0).with_alpha(0.05),
            Oklch::new(0.0, 0.0, 0.0).with_alpha(0.35),
            Oklch::new(0.0, 0.0, 0.0).with_alpha(0.4),
        ),
        false => (
            Oklch::new(0.21, 0.034, 264.665).with_alpha(0.04),
            Oklch::new(0.21, 0.034, 264.665).with_alpha(0.1),
            Oklch::new(0.21, 0.034, 264.665).with_alpha(0.12),
        ),
    };
    let inner = |dy: f32, blur: f32, spread: f32, color: Oklch| aqua_ui::style::GpuiShadowLayer {
        dx: 0.0,
        dy,
        blur,
        spread,
        inset: true,
        color: color.to_reference_paint(),
    };
    UiBox::new()
        .width(Length::Pct(100.0))
        .padding(COMPOSER_PADDING)
        .radius(INSET_RADIUS)
        .style(
            Style::new()
                .axis(Axis::Column)
                .gpui_background(aqua_core::GpuiBackground::Solid(shade.to_reference_paint()))
                .gpui_shadows(vec![inner(0.0, 0.0, 1.0, rim), inner(1.5, 4.0, 0.0, depth)]),
        )
        .child(content)
        .build()
}

pub struct ComposerFrame {
    theme: ArtisanTheme,
    content: Element,
    nested: bool,
}

impl ComposerFrame {
    pub fn new(theme: ArtisanTheme, content: Element) -> Self {
        Self {
            theme,
            content,
            nested: false,
        }
    }

    pub const fn nested(mut self, nested: bool) -> Self {
        self.nested = nested;
        self
    }

    pub fn build(self) -> Element {
        use aqua_ui::style::{Axis, Length};
        let radius = COMPOSER_RADIUS;
        match self.nested {
            true => inset(&self.theme, self.content),
            false => UiBox::new()
                .width(Length::Pct(100.0))
                .padding(COMPOSER_PADDING)
                .radius(radius)
                .style(
                    Style::new()
                        .axis(Axis::Column)
                        .shadows(KitElevation::Overlay.shadows(&self.theme)),
                )
                .child(kit_glass::paint(
                    &self.theme,
                    kit_glass::Shade::Clear,
                    radius,
                ))
                .child(self.content)
                .build(),
        }
    }
}
