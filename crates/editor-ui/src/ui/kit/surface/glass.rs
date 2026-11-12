use crate::ui::theme::{ArtisanTheme, Oklch};
use aqua_core::geometry::{CornerRadii, Rect};
use aqua_core::scene::GpuiShadowLayer;
use aqua_core::{DisplayItem, GpuiBackground, GpuiGradientColorSpace, GpuiGradientStop};
use aqua_ui::element::{Canvas, Element};
use aqua_ui::style::Style;

pub const BLUR: f32 = 20.0;
const WHITE: Oklch = Oklch::new(1.0, 0.0, 0.0);
const BLACK: Oklch = Oklch::new(0.0, 0.0, 0.0);

struct Glass {
    base: Oklch,
    near: Oklch,
    far: Oklch,
    rim_shade: Oklch,
}

impl Glass {
    fn of(theme: &ArtisanTheme) -> Self {
        let ink = theme.colors.text;
        match theme.colors.mode.is_dark() {
            true => Self {
                base: ink.with_alpha(0.05),
                near: Oklch::new(0.2801, 0.0, 0.0).with_alpha(0.22),
                far: Oklch::new(0.2484, 0.0, 0.0).with_alpha(0.16),
                rim_shade: BLACK.with_alpha(0.22),
            },
            false => Self {
                base: ink.with_alpha(0.05),
                near: Oklch::new(0.872, 0.01, 258.338).with_alpha(0.22),
                far: Oklch::new(0.928, 0.006, 264.531).with_alpha(0.16),
                rim_shade: BLACK.with_alpha(0.05),
            },
        }
    }
}

fn stop(percentage: f32, color: Oklch) -> GpuiGradientStop {
    GpuiGradientStop {
        percentage,
        color: color.to_reference_paint(),
    }
}

fn gradient(angle: f32, from: Oklch, to: Oklch, end: f32) -> GpuiBackground {
    GpuiBackground::LinearGradient {
        angle,
        color_space: GpuiGradientColorSpace::Oklab,
        stops: [stop(0.0, from), stop(end, to)],
    }
}

#[must_use]
pub fn fills(theme: &ArtisanTheme) -> [GpuiBackground; 3] {
    let glass = Glass::of(theme);
    [
        GpuiBackground::Solid(glass.base.to_reference_paint()),
        gradient(145.0, glass.near, glass.far, 1.0),
        gradient(180.0, WHITE.with_alpha(0.08), WHITE.with_alpha(0.0), 0.44),
    ]
}

#[must_use]
pub fn rims(theme: &ArtisanTheme) -> [(f32, Oklch); 2] {
    [(1.0, WHITE.with_alpha(0.1)), (-1.0, Glass::of(theme).rim_shade)]
}

fn rim(bounds: Rect, corners: CornerRadii, dy: f32, color: Oklch, inset: bool) -> DisplayItem {
    DisplayItem::GpuiShadow {
        bounds,
        corners,
        layer: GpuiShadowLayer {
            dx: 0.0,
            dy,
            blur: 0.0,
            spread: 0.0,
            inset,
            color: color.to_reference_paint(),
        },
    }
}

#[must_use]
pub fn paint(theme: &ArtisanTheme, radius: f32) -> Element {
    let fills = fills(theme);
    let rims = rims(theme);
    Canvas::new()
        .style(Style::new().absolute_at(Some(0.0), Some(0.0), Some(0.0), Some(0.0)))
        .scroll_stable()
        .on_paint(move |bounds| {
            let corners = CornerRadii::uniform(radius);
            std::iter::once(DisplayItem::BackdropBlurRounded {
                bounds,
                radius: BLUR,
                corners,
            })
            .chain(fills.iter().map(|background| DisplayItem::GpuiBackground {
                bounds,
                corners,
                background: *background,
            }))
            .chain(rims.iter().map(|(dy, color)| rim(bounds, corners, *dy, *color, true)))
            .chain(std::iter::once(rim(
                bounds,
                corners,
                -1.0,
                WHITE.with_alpha(0.14),
                false,
            )))
            .collect()
        })
        .build()
        .exclude_from_hit_testing()
}
