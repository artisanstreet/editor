use crate::ui::theme::{ArtisanTheme, Oklch};
use aqua_core::geometry::CornerRadii;
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

#[must_use]
pub fn paint(theme: &ArtisanTheme, radius: f32) -> Element {
    let glass = Glass::of(theme);
    Canvas::new()
        .style(Style::new().absolute_at(Some(0.0), Some(0.0), Some(0.0), Some(0.0)))
        .scroll_stable()
        .on_paint(move |bounds| {
            let corners = CornerRadii::uniform(radius);
            let gradient = |angle, from: Oklch, to: Oklch, end| DisplayItem::GpuiBackground {
                bounds,
                corners,
                background: GpuiBackground::LinearGradient {
                    angle,
                    color_space: GpuiGradientColorSpace::Oklab,
                    stops: [stop(0.0, from), stop(end, to)],
                },
            };
            let rim = |dy: f32, color: Oklch| DisplayItem::GpuiShadow {
                bounds,
                corners,
                layer: GpuiShadowLayer {
                    dx: 0.0,
                    dy,
                    blur: 0.0,
                    spread: 0.0,
                    inset: true,
                    color: color.to_reference_paint(),
                },
            };
            vec![
                DisplayItem::BackdropBlurRounded {
                    bounds,
                    radius: BLUR,
                    corners,
                },
                DisplayItem::GpuiBackground {
                    bounds,
                    corners,
                    background: GpuiBackground::Solid(glass.base.to_reference_paint()),
                },
                gradient(145.0, glass.near, glass.far, 1.0),
                gradient(180.0, WHITE.with_alpha(0.08), WHITE.with_alpha(0.0), 0.44),
                rim(1.0, WHITE.with_alpha(0.1)),
                rim(-1.0, glass.rim_shade),
            ]
        })
        .build()
        .exclude_from_hit_testing()
}
