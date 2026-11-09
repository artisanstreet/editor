use crate::ui::theme::{ArtisanTheme, Oklch};
use aqua::{Aqua, Style, px};
use aqua::ShadowLayer;
use aqua_core::{DisplayItem, GpuiBackground, GpuiGradientColorSpace, GpuiGradientStop, Rect};
use std::sync::Arc;
use aqua_ui::element::Element;

pub const RADIUS: f32 = 18.4;
pub const INSET_RADIUS: f32 = 0.65 * 16.0;

const RIM_LIGHT: f32 = 0.04;
const RIM_SHADE: f32 = 0.25;
const HIGHLIGHT: f32 = 0.12;

pub struct Edge {
    pub rim_top: Oklch,
    pub rim_bottom: Oklch,
    pub highlight_top: Oklch,
    pub highlight_bottom: Oklch,
}

impl Edge {
    #[must_use]
    pub fn of(surface: Oklch, ink: Oklch) -> Self {
        Self {
            rim_top: surface.animate_to(ink, RIM_LIGHT),
            rim_bottom: surface.animate_to(Oklch::new(0.0, 0.0, 0.0), RIM_SHADE),
            highlight_top: surface.animate_to(ink, HIGHLIGHT),
            highlight_bottom: surface,
        }
    }
}

const WHITE: Oklch = Oklch::new(1.0, 0.0, 0.0);
const BLACK: Oklch = Oklch::new(0.0, 0.0, 0.0);
const GRAY_900: Oklch = Oklch::new(0.21, 0.034, 264.665);

pub struct Surfaces {
    pub bg: Oklch,
    pub inner: Oklch,
    pub ink: Oklch,
    pub ink_dim: Oklch,
}

impl Surfaces {
    #[must_use]
    pub fn of(theme: &ArtisanTheme) -> Self {
        let (bg, raised, ink, ink_dim) = match theme.colors.mode.is_dark() {
            true => (
                Oklch::new(0.1757, 0.0, 0.0),
                Oklch::new(0.21, 0.0, 0.0),
                WHITE,
                Oklch::new(0.709, 0.0, 0.0),
            ),
            false => (
                WHITE,
                Oklch::new(0.967, 0.003, 264.542),
                BLACK,
                Oklch::new(0.551, 0.027, 264.364),
            ),
        };
        Self {
            bg,
            inner: raised.animate_to(ink, 0.06),
            ink,
            ink_dim,
        }
    }
}

#[must_use]
pub fn inset(theme: &ArtisanTheme, children: Vec<Element>) -> Element {
    let surfaces = Surfaces::of(theme);
    outlined(surfaces.ink, surfaces.inner, INSET_RADIUS, rest_shadows(), children)
}

#[must_use]
pub fn double_rule(theme: &ArtisanTheme, bleed: f32) -> Element {
    let (shade, light) = match theme.colors.mode.is_dark() {
        true => (BLACK.with_alpha(0.22), WHITE.with_alpha(0.08)),
        false => (GRAY_900.with_alpha(0.08), WHITE.with_alpha(0.9)),
    };
    let line = |color: Oklch| {
        Aqua::Box()
            .styles([Style::new()
                .height(px(1.0))
                .gpui_background(GpuiBackground::Solid(color.to_reference_paint()))])
            .build()
    };
    Aqua::Box()
        .layout("column")
        .styles([Style::new()
            .flex_shrink(0.0)
            .margin_insets(aqua_core::Insets {
                left: -bleed,
                right: -bleed,
                top: 4.0,
                bottom: 4.0,
            })])
        .child(line(shade))
        .child(line(light))
        .build()
}

#[must_use]
pub fn card(theme: &ArtisanTheme, surface: Oklch, children: Vec<Element>) -> Element {
    outlined(theme.colors.text, surface, RADIUS, rest_shadows(), children)
}

#[must_use]
pub fn outlined(
    ink: Oklch,
    surface: Oklch,
    radius: f32,
    shadows: Vec<ShadowLayer>,
    children: Vec<Element>,
) -> Element {
    let edge = Edge::of(surface, ink);
    Aqua::Box()
        .layout("column")
        .styles([
            Aqua::Style()
                .flex_grow(1.0)
                .min_width(px(0.0))
                .min_height(px(0.0))
                .radius(px(radius)),
            Style::new()
                .gpui_background(vertical(edge.rim_top, edge.rim_bottom))
                .shadows(shadows),
        ])
        .child(layer(1.0, radius, vertical(edge.highlight_top, edge.highlight_bottom)))
        .child(layer(2.0, radius, GpuiBackground::Solid(surface.to_reference_paint())))
        .children(children)
        .build()
}

fn layer(inset: f32, radius: f32, background: GpuiBackground) -> Element {
    Aqua::Box()
        .styles([Style::new()
            .absolute_at(Some(inset), Some(inset), Some(inset), Some(inset))
            .radius(px(radius - inset))
            .gpui_background(background)])
        .build()
        .exclude_from_hit_testing()
}

pub(super) fn vertical(top: Oklch, bottom: Oklch) -> GpuiBackground {
    GpuiBackground::LinearGradient {
        angle: 180.0,
        color_space: GpuiGradientColorSpace::Srgb,
        stops: [
            GpuiGradientStop {
                percentage: 0.0,
                color: top.to_reference_paint(),
            },
            GpuiGradientStop {
                percentage: 1.0,
                color: bottom.to_reference_paint(),
            },
        ],
    }
}

#[must_use]
pub fn rest_shadows() -> Vec<ShadowLayer> {
    vec![
        ShadowLayer {
            dx: 0.0,
            dy: 5.0,
            blur: 10.0,
            spread: 0.0,
            inset: false,
            color: aqua_core::Oklch::BLACK.with_alpha(0.06),
        },
        ShadowLayer {
            dx: 0.0,
            dy: 1.0,
            blur: 8.0,
            spread: -5.0,
            inset: false,
            color: aqua_core::Oklch::BLACK,
        },
    ]
}

#[must_use]
pub fn key_shadows() -> Vec<ShadowLayer> {
    vec![ShadowLayer {
        dx: 0.0,
        dy: 3.0,
        blur: 4.0,
        spread: -3.0,
        inset: false,
        color: aqua_core::Oklch::BLACK.with_alpha(0.4),
    }]
}

#[must_use]
pub fn hex(color: Oklch) -> String {
    let srgb = color.to_srgb();
    let channel = |value: f32| (value * 255.0).round().clamp(0.0, 255.0) as u8;
    format!(
        "#{:02x}{:02x}{:02x}",
        channel(srgb.r),
        channel(srgb.g),
        channel(srgb.b)
    )
}

#[must_use]
pub fn radial_glow(bounds: Rect, color: Oklch, alpha: f32) -> DisplayItem {
    let (w, h) = (bounds.w.max(1.0), bounds.h.max(1.0));
    let tint = hex(color);
    let source = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w} {h}"><radialGradient id="g" gradientUnits="userSpaceOnUse" cx="{cx}" cy="{cy}" r="{rx}" gradientTransform="translate({cx} {cy}) scale(1 {ratio}) translate(-{cx} -{cy})"><stop offset="0" stop-color="{tint}" stop-opacity="{alpha}"/><stop offset="0.7" stop-color="{tint}" stop-opacity="0"/></radialGradient><rect width="{w}" height="{h}" fill="url(#g)"/></svg>"#,
        cx = w * 0.5,
        cy = h * 1.15,
        rx = w * 0.7,
        ratio = (h * 1.1) / (w * 0.7),
    );
    DisplayItem::GpuiSvg {
        bounds,
        source: Arc::<str>::from(source),
        tint: None,
        opacity: 1.0,
    }
}

#[must_use]
pub fn key_face(surfaces: &Surfaces, radius: f32, height: f32) -> Element {
    outlined(
        surfaces.ink,
        surfaces.bg,
        radius,
        key_shadows(),
        vec![
            Aqua::Box()
                .styles([Style::new()
                    .absolute_at(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
                    .radius(px(radius))
                    .clip(true)])
                .child(
                    Aqua::Canvas()
                        .styles([Style::new().absolute_at(Some(0.0), Some(0.0), Some(0.0), Some(0.0))])
                        .on_paint(|bounds| vec![radial_glow(bounds, WHITE, 0.07)])
                        .build()
                        .exclude_from_hit_testing(),
                )
                .build()
                .exclude_from_hit_testing(),
            Aqua::Box().styles([Aqua::Style().height(px(height))]).build(),
        ],
    )
}
