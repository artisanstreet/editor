use crate::ui::chrome::card::{key_shadows, rest_shadows, soft};
use crate::ui::glass::glass_card_shadows;
use crate::ui::theme::{ArtisanTheme, Cast, Elevation as Tier};
use aqua::ShadowLayer;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Elevation {
    Flat,
    Key,
    Rest,
    Float,
    Overlay,
}

impl Elevation {
    #[must_use]
    pub fn shadows(self, theme: &ArtisanTheme) -> Vec<ShadowLayer> {
        match self {
            Self::Flat => Vec::new(),
            Self::Key => key_shadows(theme),
            Self::Rest => rest_shadows(theme),
            Self::Float => glass_card_shadows(theme, Tier::Floating)
                .into_iter()
                .map(ShadowLayer::from)
                .collect(),
            Self::Overlay => soft(theme, Cast::OVERLAY),
        }
    }
}
