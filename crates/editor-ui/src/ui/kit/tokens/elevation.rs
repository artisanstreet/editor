use crate::ui::chrome::card::{key_shadows, rest_shadows};
use crate::ui::glass::glass_card_shadows;
use crate::ui::theme::{ArtisanTheme, Elevation as Tier, Oklch};
use aqua::ShadowLayer;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Elevation {
    Flat,
    Key,
    Rest,
    Float,
    Overlay,
}

const WHITE: Oklch = Oklch::new(1.0, 0.0, 0.0);
const BLACK: Oklch = Oklch::new(0.0, 0.0, 0.0);
const GRAY_900: Oklch = Oklch::new(0.21, 0.034, 264.665);

impl Elevation {
    #[must_use]
    pub fn shadows(self, theme: &ArtisanTheme) -> Vec<ShadowLayer> {
        match self {
            Self::Flat => Vec::new(),
            Self::Key => key_shadows(),
            Self::Rest => rest_shadows(),
            Self::Float => glass_card_shadows(theme, Tier::Floating)
                .into_iter()
                .map(ShadowLayer::from)
                .collect(),
            Self::Overlay => overlay(theme),
        }
    }
}

fn overlay(theme: &ArtisanTheme) -> Vec<ShadowLayer> {
    let (ink, alphas) = if theme.colors.mode.is_dark() {
        (BLACK, [0.25, 0.3, 0.35, 0.35])
    } else {
        (GRAY_900, [0.03, 0.04, 0.04, 0.05])
    };
    let rim = ShadowLayer {
        dx: 0.0,
        dy: -1.0,
        blur: 0.0,
        spread: 0.0,
        inset: false,
        color: WHITE.with_alpha(0.14).to_paint(),
    };
    std::iter::once(rim)
        .chain(
            [(1.0, 2.0, 0.0), (4.0, 8.0, -2.0), (12.0, 24.0, -6.0), (24.0, 48.0, -12.0)]
                .into_iter()
                .zip(alphas)
                .map(|((dy, blur, spread), alpha)| drop(dy, blur, spread, ink.with_alpha(alpha))),
        )
        .collect()
}

fn drop(dy: f32, blur: f32, spread: f32, color: Oklch) -> ShadowLayer {
    ShadowLayer {
        dx: 0.0,
        dy,
        blur,
        spread,
        inset: false,
        color: color.to_paint(),
    }
}
