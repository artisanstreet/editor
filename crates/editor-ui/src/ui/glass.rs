use aqua_core::Insets;
use aqua_ui::{
    element::{Box as UiBox, Element},
    style::Style,
};

use super::theme::{ArtisanTheme, Elevation, Oklch, Opacity, ShadowLayer, ThemeMode};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GlassStrength {
    Quiet,
    Strong,
}

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

pub const fn glass_blur_radius(strength: GlassStrength) -> f32 {
    match strength {
        GlassStrength::Quiet => 12.0,
        GlassStrength::Strong => 20.0,
    }
}

pub fn glass_material(theme: &ArtisanTheme, strength: GlassStrength) -> Gradient {
    let (from, to) = match strength {
        GlassStrength::Quiet => (Opacity::GlassFaceQuiet, Opacity::GlassFaceQuietFar),
        GlassStrength::Strong => (Opacity::GlassFaceStrong, Opacity::GlassFaceStrongFar),
    };
    Gradient {
        angle_degrees: 145.0,
        start: theme.colors.gray.step(7).at(from),
        end: theme.colors.gray.step(4).at(to),
        end_position: 1.0,
    }
}

pub fn glass_highlight(theme: &ArtisanTheme, strength: GlassStrength) -> Gradient {
    let (opacity, end_position) = match strength {
        GlassStrength::Quiet => (Opacity::Sheen, 0.42),
        GlassStrength::Strong => (Opacity::SheenStrong, 0.44),
    };
    Gradient {
        angle_degrees: 180.0,
        start: theme.colors.white.at(opacity),
        end: theme.colors.white.at(Opacity::Clear),
        end_position,
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

pub fn glass_navigator_card(theme: &ArtisanTheme, content: Element) -> Element {
    let radius = 14.0;
    UiBox::new()
        .layout("column")
        .width(aqua_ui::style::Length::Pct(100.0))
        .radius(radius)
        .style(
            Style::new()
                .min_width(aqua_ui::style::Length::Px(0.0))
                .gpui_border(
                    aqua_core::Insets::uniform(1.0),
                    theme
                        .colors
                        .text
                        .at(Opacity::GlassBorder)
                        .to_reference_paint(),
                    false,
                )
                .gpui_source_text_color(theme.colors.text_muted.to_reference_text_color())
                .gpui_shadows(
                    glass_card_shadows(theme, Elevation::Overlay)
                        .into_iter()
                        .map(aqua_ui::style::GpuiShadowLayer::from)
                        .collect(),
                ),
        )
        .child(glass_layers(theme, GlassStrength::Strong, radius))
        .child(content)
        .build()
}

pub fn glass_handle(theme: &ArtisanTheme, content: Element) -> Element {
    let radius = 16.0;
    UiBox::new()
        .width(32.0)
        .height(32.0)
        .radius(radius)
        .style(
            Style::new()
                .gpui_border(
                    Insets::uniform(1.0),
                    theme
                        .colors
                        .text
                        .at(Opacity::GlassBorder)
                        .to_reference_paint(),
                    false,
                )
                .gpui_source_text_color(theme.colors.text.to_reference_text_color())
                .gpui_shadows(
                    glass_card_shadows(theme, Elevation::Overlay)
                        .into_iter()
                        .map(aqua_ui::style::GpuiShadowLayer::from)
                        .collect(),
                ),
        )
        .child(glass_layers(theme, GlassStrength::Quiet, radius))
        .child(
            UiBox::new()
                .width(aqua_ui::style::Length::Pct(100.0))
                .height(aqua_ui::style::Length::Pct(100.0))
                .child(content)
                .build(),
        )
        .build()
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

pub fn tinted_card_shadows(theme: &ArtisanTheme, tone: Oklch) -> Vec<GlassShadow> {
    let mut shadows = vec![
        GlassShadow {
            color: tone.with_alpha(0.35),
            offset_x: 0.0,
            offset_y: 0.0,
            css_blur: 0.0,
            spread: 0.5,
            inset: false,
        },
        GlassShadow {
            color: tone.with_alpha(0.18),
            offset_x: 0.0,
            offset_y: 1.0,
            css_blur: 6.0,
            spread: 0.0,
            inset: false,
        },
    ];
    shadows.extend(
        theme
            .elevation
            .drops(Elevation::Raised)
            .iter()
            .copied()
            .map(elevation_shadow),
    );
    shadows
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

pub fn glass_layers(theme: &ArtisanTheme, strength: GlassStrength, radius: f32) -> Element {
    use aqua_core::{
        CornerRadii, DisplayItem, GpuiBackground, GpuiGradientColorSpace, GpuiGradientStop,
    };
    use aqua_ui::element::Canvas;

    let material = glass_material(theme, strength);
    let highlight = glass_highlight(theme, strength);
    let base = theme
        .colors
        .text
        .at(Opacity::GlassBase)
        .to_reference_paint();
    Canvas::new()
        .radius(radius)
        .clip(true)
        .style(Style::new().absolute_at(Some(0.0), Some(0.0), Some(0.0), Some(0.0)))
        .scroll_stable()
        .on_paint(move |bounds| {
            let layer = |gradient: Gradient| DisplayItem::GpuiBackground {
                bounds,
                corners: CornerRadii::uniform(radius),
                background: GpuiBackground::LinearGradient {
                    angle: gradient.angle_degrees,
                    color_space: GpuiGradientColorSpace::Oklab,
                    stops: [
                        GpuiGradientStop {
                            percentage: 0.0,
                            color: gradient.start.to_reference_paint(),
                        },
                        GpuiGradientStop {
                            percentage: gradient.end_position,
                            color: gradient.end.to_reference_paint(),
                        },
                    ],
                },
            };
            vec![
                DisplayItem::BackdropBlurRounded {
                    bounds,
                    radius: glass_blur_radius(strength),
                    corners: CornerRadii::uniform(radius),
                },
                DisplayItem::GpuiBackground {
                    bounds,
                    corners: CornerRadii::uniform(radius),
                    background: GpuiBackground::Solid(base),
                },
                layer(material),
                layer(highlight),
            ]
        })
        .build()
}

pub const KEY_RADIUS: f32 = 6.0;
pub const COMPOSER_PADDING: f32 = 8.0;
pub const COMPOSER_RADIUS: f32 = KEY_RADIUS + COMPOSER_PADDING;

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
        let shadows = if self.nested {
            glass_rims(&self.theme)
        } else {
            glass_card_shadows(&self.theme, Elevation::Floating)
        };
        UiBox::new()
            .width(Length::Pct(100.0))
            .padding(COMPOSER_PADDING)
            .radius(radius)
            .style(
                Style::new()
                    .axis(Axis::Column)
                    .gpui_shadows(
                        shadows
                            .into_iter()
                            .map(aqua_ui::style::GpuiShadowLayer::from)
                            .collect(),
                    ),
            )
            .child(glass_layers(&self.theme, GlassStrength::Quiet, radius))
            .child(self.content)
            .build()
    }
}
