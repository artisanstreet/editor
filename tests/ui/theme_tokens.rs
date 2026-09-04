//! External coverage for the packet 7.01 typed theme foundation.
//!
//! These tests exercise only the public `artisan_ui::theme` API and pin:
//! the complete 41-step OKLCH surface ramp verbatim from `theme.css:49–89`,
//! every light/dark semantic token, the `color-mix` foreground derivation,
//! the OKLCH→sRGB→GPUI-paint conversion against independently computed
//! anchors, and the radius/spacing/typography/density/elevation/interaction
//! values cited in INVENTORY §2 and §5.

// Every float comparison below pins a transcribed legacy constant or an
// exactly-representable derived value; strict equality is the point, since
// any drift is precisely the regression these tests exist to catch.
#![allow(clippy::float_cmp)]

use artisan_ui::theme::{
    ArtisanTheme, ControlSize, Oklch, RadiusStep, RadiusTokens, SurfaceStep, ThemeMode,
};

/// Tolerance for normalized sRGB channels compared against double-precision
/// reference anchors: comfortably above float32 noise (~1e-7) and below one
/// eighth-bit step (1/255 ≈ 0.0039).
const SRGB_TOLERANCE: f32 = 1.0 / 255.0 / 4.0;

fn assert_channel_close(label: &str, actual: f32, expected: f32) {
    assert!(
        (actual - expected).abs() <= SRGB_TOLERANCE,
        "{label}: actual {actual} vs expected {expected}"
    );
}

fn assert_srgb_close(label: &str, actual: artisan_ui::theme::SrgbComponents, expected: [f32; 4]) {
    assert_channel_close(&format!("{label} r"), actual.r, expected[0]);
    assert_channel_close(&format!("{label} g"), actual.g, expected[1]);
    assert_channel_close(&format!("{label} b"), actual.b, expected[2]);
    assert_channel_close(&format!("{label} a"), actual.a, expected[3]);
}

/// The full source ramp as `(step, l, c, h)` triples transcribed from
/// `theme.css:49–89`.
const RAMP_SOURCE: [(SurfaceStep, f32, f32, f32); 41] = [
    (SurfaceStep::S0, 1.0, 0.0, 0.0),
    (SurfaceStep::S25, 0.9925, 0.0, 0.0),
    (SurfaceStep::S50, 0.985, 0.0, 0.0),
    (SurfaceStep::S75, 0.976, 0.0005, 286.375),
    (SurfaceStep::S100, 0.967, 0.001, 286.375),
    (SurfaceStep::S125, 0.9552, 0.0018, 286.361),
    (SurfaceStep::S150, 0.9435, 0.0025, 286.347),
    (SurfaceStep::S175, 0.9318, 0.0033, 286.334),
    (SurfaceStep::S200, 0.92, 0.004, 286.32),
    (SurfaceStep::S225, 0.9078, 0.0045, 286.312),
    (SurfaceStep::S250, 0.8955, 0.005, 286.303),
    (SurfaceStep::S275, 0.8832, 0.0055, 286.294),
    (SurfaceStep::S300, 0.871, 0.006, 286.286),
    (SurfaceStep::S325, 0.8295, 0.0083, 286.231),
    (SurfaceStep::S350, 0.788, 0.0105, 286.177),
    (SurfaceStep::S375, 0.7465, 0.0127, 286.122),
    (SurfaceStep::S400, 0.705, 0.015, 286.067),
    (SurfaceStep::S425, 0.6667, 0.0152, 286.035),
    (SurfaceStep::S450, 0.6285, 0.0155, 286.002),
    (SurfaceStep::S475, 0.5903, 0.0158, 285.97),
    (SurfaceStep::S500, 0.552, 0.016, 285.938),
    (SurfaceStep::S525, 0.5245, 0.0163, 285.9),
    (SurfaceStep::S550, 0.497, 0.0165, 285.862),
    (SurfaceStep::S575, 0.4695, 0.0168, 285.824),
    (SurfaceStep::S600, 0.442, 0.017, 285.786),
    (SurfaceStep::S625, 0.424, 0.016, 285.791),
    (SurfaceStep::S650, 0.406, 0.015, 285.796),
    (SurfaceStep::S675, 0.388, 0.014, 285.8),
    (SurfaceStep::S700, 0.37, 0.013, 285.805),
    (SurfaceStep::S725, 0.346, 0.0112, 285.862),
    (SurfaceStep::S750, 0.322, 0.0095, 285.919),
    (SurfaceStep::S775, 0.298, 0.0077, 285.976),
    (SurfaceStep::S800, 0.274, 0.006, 286.033),
    (SurfaceStep::S825, 0.258, 0.006, 285.996),
    (SurfaceStep::S850, 0.242, 0.006, 285.959),
    (SurfaceStep::S875, 0.226, 0.006, 285.922),
    (SurfaceStep::S900, 0.21, 0.006, 285.885),
    (SurfaceStep::S925, 0.1755, 0.0055, 285.854),
    (SurfaceStep::S950, 0.141, 0.005, 285.823),
    (SurfaceStep::S975, 0.0705, 0.0025, 285.823),
    (SurfaceStep::S1000, 0.0, 0.0, 0.0),
];

#[test]
fn ramp_carries_all_41_source_values_verbatim() {
    assert_eq!(SurfaceStep::ALL.len(), 41);
    for (index, (step, l, c, h)) in RAMP_SOURCE.iter().enumerate() {
        assert_eq!((index, &SurfaceStep::ALL[index]), (index, step));
        let value = step.oklch();
        assert_eq!(value.l, *l, "lightness of {step:?}");
        assert_eq!(value.c, *c, "chroma of {step:?}");
        assert_eq!(value.h, *h, "hue of {step:?}");
        assert_eq!(value.a, 1.0, "ramp steps are opaque");
    }
}

#[test]
fn ramp_lightness_is_monotonic_with_exact_endpoints() {
    // The ramp runs white (S0) down to black (S1000), so perceptual
    // lightness must be non-increasing across source order.
    let mut previous = 2.0_f32;
    for step in SurfaceStep::ALL {
        let value = step.oklch();
        assert!(
            value.l <= previous,
            "ramp must not brighten upward at {step:?}"
        );
        previous = value.l;
    }
    // Endpoints are exact CSS white and black (`oklch(1 0 0)`, `oklch(0 0 0)`).
    let white = SurfaceStep::S0.oklch().to_srgb();
    assert_srgb_close("surface-0", white, [1.0, 1.0, 1.0, 1.0]);
    let black = SurfaceStep::S1000.oklch().to_srgb();
    assert_srgb_close("surface-1000", black, [0.0, 0.0, 0.0, 1.0]);
}

#[test]
fn theme_mode_defaults_to_dark_and_selects_explicitly() {
    assert_eq!(ThemeMode::default(), ThemeMode::Dark);

    let dark = ArtisanTheme::for_mode(ThemeMode::Dark);
    let light = ArtisanTheme::for_mode(ThemeMode::Light);
    assert_eq!(dark.mode, ThemeMode::Dark);
    assert_eq!(
        dark.colors.background,
        SurfaceStep::S950.oklch(),
        ".dark background = --surface-950"
    );
    assert_eq!(
        light.colors.background,
        SurfaceStep::S0.oklch(),
        ":root background = --surface-0"
    );
    assert_ne!(
        dark.colors.background, light.colors.background,
        "modes must resolve different surfaces"
    );
}

#[test]
fn light_semantic_tokens_match_theme_css_root_block() {
    let theme = ArtisanTheme::for_mode(ThemeMode::Light);
    let c = &theme.colors;
    assert_eq!(c.foreground_base, SurfaceStep::S950.oklch());
    assert_eq!(c.foreground_extra, SurfaceStep::S950.oklch());
    assert_eq!(c.highlight, SurfaceStep::S950.oklch());
    assert_eq!(
        (c.card, c.card_foreground),
        (SurfaceStep::S0.oklch(), SurfaceStep::S950.oklch())
    );
    assert_eq!(
        (c.popover, c.popover_foreground),
        (SurfaceStep::S0.oklch(), SurfaceStep::S950.oklch())
    );
    assert_eq!(
        (c.primary, c.primary_foreground),
        (SurfaceStep::S900.oklch(), SurfaceStep::S50.oklch())
    );
    assert_eq!(
        (c.secondary, c.secondary_foreground),
        (SurfaceStep::S100.oklch(), SurfaceStep::S900.oklch())
    );
    assert_eq!(
        (c.muted, c.muted_foreground),
        (SurfaceStep::S100.oklch(), SurfaceStep::S500.oklch())
    );
    assert_eq!(
        (c.accent, c.accent_foreground),
        (SurfaceStep::S100.oklch(), SurfaceStep::S900.oklch())
    );
    assert_eq!(c.destructive, Oklch::new(0.577, 0.245, 27.325));
    assert_eq!(c.banner_info, Oklch::new(0.623, 0.214, 259.815));
    assert_eq!(c.banner_error, Oklch::new(0.577, 0.245, 27.325));
    assert_eq!(c.banner_warning, Oklch::new(0.681, 0.162, 75.834));
    assert_eq!(c.banner_success, Oklch::new(0.527, 0.154, 150.069));
    assert_eq!(c.favorite, Oklch::new(0.706, 0.153, 78.5));
    assert_eq!(c.unread, Oklch::new(0.685, 0.145, 230.318));
    assert_eq!(c.question_from, Oklch::new(0.558, 0.288, 302.321));
    assert_eq!(c.question_to, Oklch::new(0.714, 0.203, 305.504));
    assert_eq!(
        (c.border, c.input),
        (SurfaceStep::S200.oklch(), SurfaceStep::S200.oklch())
    );
    assert_eq!(c.ring, SurfaceStep::S400.oklch());
    assert_eq!(
        c.charts,
        [
            SurfaceStep::S300.oklch(),
            SurfaceStep::S500.oklch(),
            SurfaceStep::S600.oklch(),
            SurfaceStep::S700.oklch(),
            SurfaceStep::S800.oklch(),
        ]
    );
}

#[test]
fn dark_semantic_tokens_match_the_dark_blocks() {
    let theme = ArtisanTheme::for_mode(ThemeMode::Dark);
    let c = &theme.colors;
    assert_eq!(c.foreground_base, SurfaceStep::S50.oklch());
    assert_eq!(c.highlight, SurfaceStep::S50.oklch());
    assert_eq!(
        (c.card, c.card_foreground),
        (SurfaceStep::S900.oklch(), SurfaceStep::S50.oklch())
    );
    assert_eq!(
        (c.primary, c.primary_foreground),
        (SurfaceStep::S200.oklch(), SurfaceStep::S900.oklch())
    );
    assert_eq!(
        (c.muted, c.muted_foreground),
        (SurfaceStep::S800.oklch(), SurfaceStep::S400.oklch())
    );
    assert_eq!(c.destructive, Oklch::new(0.704, 0.191, 22.216));
    assert_eq!(c.banner_info, Oklch::new(0.707, 0.165, 254.624));
    assert_eq!(c.banner_warning, Oklch::new(0.795, 0.184, 86.047));
    assert_eq!(c.banner_success, Oklch::new(0.723, 0.219, 149.579));
    assert_eq!(c.favorite, Oklch::new(0.823, 0.158, 82.5));
    assert_eq!(c.unread, Oklch::new(0.828, 0.111, 230.318));
    // Dark hairlines are white at fixed alphas (`oklch(1 0 0 / 10%)` / `15%`).
    assert_eq!(c.border, Oklch::new(1.0, 0.0, 0.0).with_alpha(0.10));
    assert_eq!(c.input, Oklch::new(1.0, 0.0, 0.0).with_alpha(0.15));
    assert_eq!(c.ring, SurfaceStep::S500.oklch());
    // Sidebar: dark primary is an off-ramp blue literal.
    assert_eq!(theme.sidebar.primary, Oklch::new(0.488, 0.243, 264.376));
    assert_eq!(
        theme.sidebar.border,
        Oklch::new(1.0, 0.0, 0.0).with_alpha(0.10)
    );
}

#[test]
fn foreground_resolves_the_legacy_oklch_color_mix() {
    // Light: 0.9·(0.141, 0.005, 285.823) + 0.1·(1, 0, powerless) = (0.2269, 0.0045).
    let light = ArtisanTheme::for_mode(ThemeMode::Light);
    let mixed_l = 0.9 * SurfaceStep::S950.oklch().l + 0.1 * SurfaceStep::S0.oklch().l;
    let mixed_c = 0.9 * SurfaceStep::S950.oklch().c + 0.1 * SurfaceStep::S0.oklch().c;
    assert!((light.colors.foreground.l - mixed_l).abs() <= f32::EPSILON * 8.0);
    assert!((light.colors.foreground.c - mixed_c).abs() <= f32::EPSILON * 8.0);
    assert_eq!(light.colors.foreground.h, SurfaceStep::S950.oklch().h);

    // Dark: 0.9·(0.985, 0) + 0.1·(0.141, 0.005) = (0.9006, 0.0005).
    let dark = ArtisanTheme::for_mode(ThemeMode::Dark);
    let dark_l = 0.9 * SurfaceStep::S50.oklch().l + 0.1 * SurfaceStep::S950.oklch().l;
    let dark_c = 0.9 * SurfaceStep::S50.oklch().c + 0.1 * SurfaceStep::S950.oklch().c;
    assert!((dark.colors.foreground.l - dark_l).abs() <= f32::EPSILON * 8.0);
    assert!((dark.colors.foreground.c - dark_c).abs() <= f32::EPSILON * 8.0);
}

/// Anchors computed independently of the Rust implementation with a
/// double-precision transcription of the CSS Color 4 pipeline; values are the
/// normalized sRGB outputs `(r, g, b)`.
#[test]
fn oklch_conversion_matches_independent_reference_anchors() {
    let cases: [(&str, Oklch, [f32; 3]); 8] = [
        (
            "surface-950",
            Oklch::new(0.141, 0.005, 285.823),
            [0.035_374, 0.035_359, 0.044_318],
        ),
        (
            "surface-500",
            Oklch::new(0.552, 0.016, 285.938),
            [0.442_995, 0.442_929, 0.483_804],
        ),
        (
            "destructive-light",
            Oklch::new(0.577, 0.245, 27.325),
            [0.906_458, 0.0, 0.042_215],
        ),
        (
            "banner-warning-light",
            Oklch::new(0.681, 0.162, 75.834),
            [0.817_554, 0.529_636, 0.0],
        ),
        (
            "favorite-light",
            Oklch::new(0.706, 0.153, 78.5),
            [0.827_605, 0.573_131, 0.0],
        ),
        (
            "selection-bg",
            Oklch::new(0.48, 0.13, 250.0),
            [0.048_999, 0.375_587, 0.639_760],
        ),
        (
            "foreground-light-mix",
            Oklch::new(0.2269, 0.0045, 285.823),
            [0.109_289, 0.109_331, 0.118_552],
        ),
        (
            "surface-400",
            Oklch::new(0.705, 0.015, 286.067),
            [0.622_613, 0.622_562, 0.663_360],
        ),
    ];
    for (label, source, expected) in cases {
        let srgb = source.to_srgb();
        assert_srgb_close(label, srgb, [expected[0], expected[1], expected[2], 1.0]);
    }
}

#[test]
fn paint_color_round_trips_through_gpui_without_drift() {
    for source in [
        SurfaceStep::S500.oklch(),
        Oklch::new(0.577, 0.245, 27.325),
        ArtisanTheme::for_mode(ThemeMode::Dark).colors.border,
    ] {
        let direct = source.to_srgb();
        let through_gpui = gpui::hsla_to_rgba(source.to_paint());
        assert_channel_close("round-trip r", through_gpui.color.red, direct.r);
        assert_channel_close("round-trip g", through_gpui.color.green, direct.g);
        assert_channel_close("round-trip b", through_gpui.color.blue, direct.b);
        assert_channel_close("round-trip a", through_gpui.alpha, direct.a);
    }
}

#[test]
fn out_of_gamut_values_clamp_at_the_final_boundary_only() {
    // A deliberately over-saturated synthetic color must stay finite and
    // inside [0, 1]; nothing upstream clamps or relabels it.
    let wild = Oklch::new(0.9, 0.4, 30.0).to_srgb();
    for (label, channel) in [("r", wild.r), ("g", wild.g), ("b", wild.b)] {
        assert!(channel.is_finite(), "{label} must stay finite");
        assert!(
            (0.0..=1.0).contains(&channel),
            "{label} must clamp into gamut"
        );
    }
    // Alpha survives conversion untouched, including fractional legacy alphas.
    let selection = Oklch::new(0.48, 0.13, 250.0).with_alpha(0.42);
    let painted = selection.to_paint();
    assert_channel_close("alpha preserved", painted.alpha, 0.42);
}

#[test]
fn radius_ramp_and_nested_arithmetic_match_the_legacy_comment() {
    let ramp = [
        RadiusTokens::value(RadiusStep::Xs),
        RadiusTokens::value(RadiusStep::Sm),
        RadiusTokens::value(RadiusStep::Md),
        RadiusTokens::value(RadiusStep::Lg),
        RadiusTokens::value(RadiusStep::Xl),
        RadiusTokens::value(RadiusStep::X2l),
        RadiusTokens::value(RadiusStep::X3l),
        RadiusTokens::value(RadiusStep::X4l),
    ];
    let expected = [
        gpui::px(4.0),
        gpui::px(6.0),
        gpui::px(8.0),
        gpui::px(10.0),
        gpui::px(14.0),
        gpui::px(18.0),
        gpui::px(22.0),
        gpui::px(26.0),
    ];
    assert_eq!(
        ramp.iter().map(f32::from).collect::<Vec<_>>(),
        expected.iter().map(|p| f32::from(*p)).collect::<Vec<_>>()
    );
    assert_eq!(
        RadiusTokens::BASE_PX,
        10.0,
        "--radius is 0.625rem at a 16px root"
    );
    // Composer nested corner: radius-2xl surface minus spacing(2) gap -> 10 px.
    let theme = ArtisanTheme::for_mode(ThemeMode::Dark);
    let nested = RadiusTokens::nested(
        RadiusTokens::value(RadiusStep::X2l),
        theme.spacing.steps(2.0),
    );
    assert_eq!(f32::from(nested), 10.0);
}

#[test]
fn typography_spacing_density_and_interaction_pin_their_sources() {
    use artisan_ui::theme::WeightRange;

    let theme = ArtisanTheme::for_mode(ThemeMode::Light);
    let t = &theme.typography;
    assert_eq!(t.sans.family, "Artisan Neo");
    assert_eq!(t.sans.weights, WeightRange { min: 100, max: 900 });
    assert_eq!(t.mono.family, "JetBrains Mono");
    assert_eq!(t.mono.weights, WeightRange { min: 100, max: 800 });
    assert_eq!(t.logo.family, "Cal Sans");
    assert_eq!(
        t.logo.weights,
        WeightRange {
            min: 100,
            max: 1000
        }
    );
    assert_eq!(t.wordmark.family, "Sigurd Variable");
    assert_eq!(t.wordmark.weights, WeightRange { min: 300, max: 900 });
    assert_eq!(
        (
            f32::from(t.control_text),
            f32::from(t.editor_text_base),
            f32::from(t.editor_text_desktop),
            f32::from(t.label_text),
        ),
        (14.0, 16.0, 14.0, 12.0)
    );
    assert_eq!(t.dialog_title_weight, 500);

    assert_eq!(f32::from(theme.spacing.steps(1.0)), 4.0);
    assert_eq!(f32::from(theme.spacing.steps(6.0)), 24.0);
    assert_eq!(f32::from(theme.spacing.steps(1.5)), 6.0);

    let d = &theme.density;
    assert_eq!(
        [
            f32::from(d.control_default),
            f32::from(d.control_xs),
            f32::from(d.control_sm),
            f32::from(d.control_lg),
        ],
        [36.0, 24.0, 32.0, 40.0]
    );
    assert_eq!(d.control_height(ControlSize::Default), d.control_default);
    assert_eq!(d.control_height(ControlSize::Sm), d.control_sm);
    assert_eq!(d.control_height(ControlSize::Xs), d.control_xs);
    assert_eq!(d.control_height(ControlSize::Lg), d.control_lg);
    assert_eq!(
        (
            f32::from(d.switch_default.0),
            f32::from(d.switch_default.1),
            f32::from(d.switch_sm.0),
            f32::from(d.switch_sm.1),
        ),
        (32.0, 18.4, 24.0, 14.0)
    );
    assert_eq!(f32::from(d.badge_height), 20.0);
    assert_eq!(
        (
            f32::from(d.tabs_list_height),
            f32::from(d.tabs_list_padding)
        ),
        (36.0, 3.0)
    );
    assert_eq!(f32::from(d.command_list_max_height), 288.0);
    assert_eq!(
        (f32::from(d.card_padding), f32::from(d.card_padding_compact)),
        (24.0, 16.0)
    );

    let i = &theme.interaction;
    assert_eq!(f32::from(i.focus_ring_width), 3.0);
    assert_eq!(i.focus_ring_color, theme.colors.ring.with_alpha(0.5));
    assert_eq!(
        i.invalid_ring_color,
        theme.colors.destructive.with_alpha(0.2)
    );
    assert_channel_close("selection alpha", i.selection_background.a, 0.42);
    assert_eq!(i.selection_foreground, theme.colors.foreground);
    assert_eq!(
        (i.hover_fill_top, i.hover_fill_bottom),
        (
            theme.colors.foreground.with_alpha(0.16),
            theme.colors.foreground.with_alpha(0.07)
        )
    );
}

#[test]
fn elevation_preserves_card_and_menu_stacks_and_records_insets() {
    let theme = ArtisanTheme::for_mode(ThemeMode::Dark);
    // `@utility card` geometry per utilities.css:31-37.
    let geometry: Vec<(f32, f32, f32, f32)> = theme
        .elevation
        .card_shadow
        .iter()
        .map(|layer| {
            (
                f32::from(layer.offset_x),
                f32::from(layer.offset_y),
                f32::from(layer.blur_radius),
                f32::from(layer.spread_radius),
            )
        })
        .collect();
    assert_eq!(
        geometry,
        vec![
            (0.0, -0.5, 0.0, 0.0),
            (0.0, 4.0, 8.0, 0.0),
            (0.0, 0.0, 0.0, 0.5),
            (0.0, 1.0, 6.0, -4.0),
        ]
    );
    assert_channel_close(
        "card layer-1 white alpha",
        theme.elevation.card_shadow[0].color.a,
        0.08,
    );
    assert_channel_close(
        "card layer-2 black alpha",
        theme.elevation.card_shadow[1].color.a,
        0.06,
    );

    // The floating-menu shadow is Tailwind's shadow-2xl recipe.
    let menu = theme.elevation.menu_shadow[0];
    assert_eq!(
        (
            f32::from(menu.offset_y),
            f32::from(menu.blur_radius),
            f32::from(menu.spread_radius)
        ),
        (25.0, 50.0, -12.0)
    );
    assert_channel_close("menu shadow alpha", menu.color.a, 0.25);

    // GPUI mapping lands each field where the pinned struct expects it.
    let mapped = theme.elevation.card_shadow[1].to_box_shadow();
    assert_channel_close("mapped offset y", f32::from(mapped.offset.y), 4.0);
    assert_channel_close("mapped blur", f32::from(mapped.blur_radius), 8.0);

    // Inset stacks are recorded verbatim but intentionally expose no GPUI
    // conversion; pin their recorded shape.
    assert_eq!(theme.elevation.inset.len(), 5);
    assert_eq!(theme.elevation.inset_artwork.len(), 5);
    let artwork_first = theme.elevation.inset_artwork[0];
    assert_eq!(
        (
            f32::from(artwork_first.offset_y),
            f32::from(artwork_first.blur_radius)
        ),
        (1.0, 1.0)
    );
    assert_channel_close("artwork inset alpha", artwork_first.color.a, 0.30);
}
