use editor_ui::chrome::card::Surfaces;
use editor_ui::theme::{ArtisanTheme, Contrast, ThemeMode};

#[test]
fn chrome_materials_follow_the_high_contrast_palette() {
    for mode in [ThemeMode::Light, ThemeMode::Dark] {
        let standard = ArtisanTheme::for_mode_and_contrast(mode, Contrast::Standard);
        let high = ArtisanTheme::for_mode_and_contrast(mode, Contrast::High);
        assert_ne!(Surfaces::of(standard).ink_dim, Surfaces::of(high).ink_dim);
        assert_eq!(Surfaces::of(high).ink_dim, high.colors.text);
        assert_eq!(Surfaces::of(high).ink, high.colors.text);
    }
}
