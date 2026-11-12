use super::kit::hover::{Hover, Material, neutral_stops};
use super::theme::{ArtisanTheme, Oklch};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HoverFill {
    Plain,
    Edged,
}
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum HoverTone {
    #[default]
    Neutral,
    Destructive,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HoverColors {
    top: Oklch,
    bottom: Oklch,
    ring: Oklch,
}

impl HoverColors {
    #[must_use]
    pub fn new(theme: &ArtisanTheme, tone: HoverTone) -> Self {
        match tone {
            HoverTone::Neutral => Self::of(Hover::Neutral.material(theme)),
            HoverTone::Destructive => Self::of(Hover::Destructive.material(theme)),
        }
    }
    const fn of(material: Material) -> Self {
        Self {
            top: material.top,
            bottom: material.bottom,
            ring: material.ring,
        }
    }
    #[must_use]
    pub fn animate_to(self, to: Self, progress: f32) -> Self {
        Self {
            top: self.top.animate_to(to.top, progress),
            bottom: self.bottom.animate_to(to.bottom, progress),
            ring: self.ring.animate_to(to.ring, progress),
        }
    }

    pub fn style(self, fill: HoverFill) -> aqua_ui::style::Style {
        let mut style = aqua_ui::style::Style::new().gpui_background(
            aqua_core::GpuiBackground::LinearGradient {
                angle: 180.0,
                color_space: aqua_core::GpuiGradientColorSpace::Oklab,
                stops: [
                    aqua_core::GpuiGradientStop {
                        percentage: 0.0,
                        color: self.top.to_reference_paint(),
                    },
                    aqua_core::GpuiGradientStop {
                        percentage: 1.0,
                        color: self.bottom.to_reference_paint(),
                    },
                ],
            },
        );
        if fill == HoverFill::Edged {
            style = style.gpui_shadows(vec![aqua_ui::style::GpuiShadowLayer {
                dx: 0.0,
                dy: 0.0,
                blur: 0.0,
                spread: 0.5,
                inset: true,
                color: self.ring.to_reference_paint(),
            }]);
        }
        style
    }
}

pub fn hover_fill_stops(theme: &ArtisanTheme, ink: Oklch) -> (Oklch, Oklch) {
    neutral_stops(theme, ink)
}
