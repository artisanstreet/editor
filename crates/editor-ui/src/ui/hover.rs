use super::kit::hover::neutral_stops;
use super::theme::{ArtisanTheme, Oklch};

pub fn hover_fill_stops(theme: &ArtisanTheme, ink: Oklch) -> (Oklch, Oklch) {
    neutral_stops(theme, ink)
}
