use editor_aqua::ui::theme::{ArtisanTheme, Opacity, ThemeMode};
use editor_ui::theme::{
    ArtisanTheme as SourceTheme, Opacity as SourceOpacity, ThemeMode as SourceMode,
};

#[test]
fn reference_paint_matches_gpui_theme_and_scene_components_bit_for_bit() {
    for (mode, source_mode) in [
        (ThemeMode::Light, SourceMode::Light),
        (ThemeMode::Dark, SourceMode::Dark),
    ] {
        let colors = &ArtisanTheme::for_mode(mode).colors;
        let source = &SourceTheme::for_mode(source_mode).colors;
        let samples = [
            (colors.black, source.black),
            (colors.white, source.white),
            (colors.text, source.text),
            (colors.primary, source.primary),
            (colors.claude, source.claude),
            (colors.red.solid, source.red.solid),
            (colors.green.solid, source.green.solid),
            (
                colors.text.at(Opacity::Hairline),
                source.text.at(SourceOpacity::Hairline),
            ),
            (colors.primary.fade(0.0), source.primary.fade(0.0)),
            (colors.primary.fade(0.37), source.primary.fade(0.37)),
            (
                colors.primary.animate_to(colors.red.solid, 0.41),
                source.primary.animate_to(source.red.solid, 0.41),
            ),
            (
                colors.white.in_hue_of(colors.claude),
                source.white.in_hue_of(source.claude),
            ),
        ];
        for (token, source_token) in samples {
            let actual_srgb = token.to_srgb();
            let expected_srgb = source_token.to_srgb();
            assert_eq!(
                [actual_srgb.r, actual_srgb.g, actual_srgb.b, actual_srgb.a].map(f32::to_bits),
                [
                    expected_srgb.r,
                    expected_srgb.g,
                    expected_srgb.b,
                    expected_srgb.a
                ]
                .map(f32::to_bits)
            );
            let expected = source_token.to_paint();
            let source_color = token.to_reference_text_color();
            assert_eq!(
                source_color.hue_degrees.to_bits(),
                expected.hue.into_raw_degrees().to_bits()
            );
            assert_eq!(source_color.saturation.to_bits(), expected.saturation.to_bits());
            assert_eq!(source_color.lightness.to_bits(), expected.lightness.to_bits());
            assert_eq!(source_color.alpha.to_bits(), expected.alpha.to_bits());
            let actual = token.to_reference_paint();
            assert_eq!(
                actual.h.to_bits(),
                (expected.hue.into_positive_degrees() / 360.0).to_bits()
            );
            assert_eq!(actual.s.to_bits(), expected.saturation.to_bits());
            assert_eq!(actual.l.to_bits(), expected.lightness.to_bits());
            assert_eq!(actual.a.to_bits(), expected.alpha.to_bits());
        }
    }
}
