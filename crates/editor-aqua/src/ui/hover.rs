use super::theme::{ArtisanTheme, Oklch, Opacity};

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
    pub const fn new(theme: &ArtisanTheme, tone: HoverTone) -> Self {
        match tone {
            HoverTone::Neutral => {
                let ink = theme.colors.text;
                let (top, bottom) = hover_fill_stops(theme, ink);
                Self {
                    top,
                    bottom,
                    ring: ink.at(Opacity::HoverRing),
                }
            }
            HoverTone::Destructive => {
                let ink = theme.colors.destructive_hover;
                let strong = ink.at(Opacity::DestructiveHoverStrong);
                let soft = ink.at(Opacity::DestructiveHoverSoft);
                let (top, bottom) = if theme.colors.mode.is_dark() {
                    (strong, soft)
                } else {
                    (soft, strong)
                };
                Self {
                    top,
                    bottom,
                    ring: strong,
                }
            }
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

pub const fn hover_fill_stops(theme: &ArtisanTheme, ink: Oklch) -> (Oklch, Oklch) {
    let (strong, soft) = (ink.at(Opacity::HoverTop), ink.at(Opacity::HoverBottom));
    if theme.colors.mode.is_dark() {
        (strong, soft)
    } else {
        (soft, strong)
    }
}
