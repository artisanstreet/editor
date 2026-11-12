use super::kit::hover::{Hover, neutral_stops};
use super::theme::{ArtisanTheme, Oklch};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum HoverTone {
    #[default]
    Neutral,
    Destructive,
}

impl From<HoverTone> for Hover {
    fn from(tone: HoverTone) -> Self {
        match tone {
            HoverTone::Neutral => Self::Neutral,
            HoverTone::Destructive => Self::Destructive,
        }
    }
}

pub fn hover_fill_stops(theme: &ArtisanTheme, ink: Oklch) -> (Oklch, Oklch) {
    neutral_stops(theme, ink)
}
