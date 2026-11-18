pub mod live;

use crate::ui::chrome::glide::{CubicBezier, Glide, Transition};
use crate::ui::theme::{ArtisanTheme, Oklch, Opacity};
use aqua_core::{GpuiBackground, GpuiGradientColorSpace, GpuiGradientStop};
use aqua_ui::style::{GpuiShadowLayer, Style};
use std::time::{Duration, Instant};

const SMOOTH_OUT: CubicBezier = CubicBezier::new(0.22, 1.0, 0.36, 1.0);
const QUICK: Transition = Transition {
    duration: Duration::from_millis(150),
    easing: SMOOTH_OUT,
};
const FAST: Transition = Transition {
    duration: Duration::from_millis(250),
    easing: SMOOTH_OUT,
};
pub const REST_SCALE: f32 = 0.4;
const RING_WIDTH: f32 = 0.5;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Hover {
    Neutral,
    Destructive,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Material {
    top: Oklch,
    bottom: Oklch,
    ring: Oklch,
}

impl Material {
    #[must_use]
    fn fill(self) -> GpuiBackground {
        GpuiBackground::LinearGradient {
            angle: 180.0,
            color_space: GpuiGradientColorSpace::Oklab,
            stops: [
                GpuiGradientStop {
                    percentage: 0.0,
                    color: self.top.to_reference_paint(),
                },
                GpuiGradientStop {
                    percentage: 1.0,
                    color: self.bottom.to_reference_paint(),
                },
            ],
        }
    }

    #[must_use]
    fn ring(self) -> GpuiShadowLayer {
        GpuiShadowLayer {
            dx: 0.0,
            dy: 0.0,
            blur: 0.0,
            spread: RING_WIDTH,
            inset: true,
            color: self.ring.to_reference_paint(),
        }
    }

    #[must_use]
    fn style(self) -> Style {
        Style::new()
            .gpui_background(self.fill())
            .gpui_shadows(vec![self.ring()])
    }
}

#[must_use]
pub(crate) fn neutral_stops(theme: &ArtisanTheme, ink: Oklch) -> (Oklch, Oklch) {
    match theme.colors.mode.is_dark() {
        true => (ink.with_alpha(0.09), ink.with_alpha(0.03)),
        false => (ink.with_alpha(0.02), ink.with_alpha(0.06)),
    }
}

impl Hover {
    #[must_use]
    fn material(self, theme: &ArtisanTheme) -> Material {
        let dark = theme.colors.mode.is_dark();
        match self {
            Self::Neutral => {
                let ink = theme.colors.text;
                let (top, bottom) = neutral_stops(theme, ink);
                Material {
                    top,
                    bottom,
                    ring: ink.with_alpha(if dark { 0.07 } else { 0.05 }),
                }
            }
            Self::Destructive => {
                let ink = theme.colors.destructive_hover;
                let strong = ink.at(Opacity::DestructiveHoverStrong);
                let soft = ink.at(Opacity::DestructiveHoverSoft);
                let (top, bottom) = if dark { (strong, soft) } else { (soft, strong) };
                Material {
                    top,
                    bottom,
                    ring: strong,
                }
            }
        }
    }

    #[must_use]
    pub fn style(self, theme: &ArtisanTheme) -> Style {
        self.material(theme).style()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Highlight {
    opacity: Glide,
    scale: Glide,
}

impl Default for Highlight {
    fn default() -> Self {
        Self {
            opacity: Glide::Resting(0.0),
            scale: Glide::Resting(REST_SCALE),
        }
    }
}

impl Highlight {
    #[must_use]
    pub fn opacity(&self, now: Instant) -> f32 {
        self.opacity.value(now, QUICK)
    }

    #[must_use]
    pub fn scale(&self, now: Instant) -> f32 {
        self.scale.value(now, QUICK)
    }

    #[must_use]
    pub fn lit(&self) -> bool {
        self.opacity.target() > 0.5
    }

    #[must_use]
    pub fn light(self, on: bool, now: Instant, reduced: bool) -> Self {
        if self.lit() == on {
            return self;
        }
        let (opacity, scale, grow) = if on {
            (1.0, 1.0, FAST)
        } else {
            (0.0, REST_SCALE, QUICK)
        };
        Self {
            opacity: self.opacity.retarget(opacity, now, QUICK, reduced),
            scale: self.scale.retarget(scale, now, grow, reduced),
        }
    }

    #[must_use]
    pub fn settle(self, now: Instant) -> Self {
        Self {
            opacity: self.opacity.settle(now),
            scale: self.scale.settle(now),
        }
    }

    #[must_use]
    pub const fn running(&self) -> bool {
        self.opacity.running() || self.scale.running()
    }
}
