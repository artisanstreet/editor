use super::theme::{ArtisanTheme, BannerColors, Oklch};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ChipTone {
    Success,
    Info,
    Warning,
    Danger,
    Muted,
}
pub const fn tone_color(theme: &ArtisanTheme, tone: ChipTone) -> Oklch {
    tone_colors(theme, tone).text
}
const fn tone_colors(theme: &ArtisanTheme, tone: ChipTone) -> BannerColors {
    let c = &theme.colors;
    match tone {
        ChipTone::Success => c.banner_success,
        ChipTone::Info => c.banner_info,
        ChipTone::Warning => c.banner_warning,
        ChipTone::Danger => c.banner_error,
        ChipTone::Muted => BannerColors {
            bg: c.raised,
            border: c.line,
            text: c.text_muted,
        },
    }
}
