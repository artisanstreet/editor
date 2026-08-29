//! Focused parity tests for the dependency-free native shell sizing policy.
//!
//! The production module is included directly so this harness does not need
//! frontend crate or build-file registration. Tests pin constants, clamp
//! endpoints, inclusive fit boundaries, and malformed geometry behavior.

#![allow(clippy::float_cmp)]

#[path = "../../modules/frontend/src/shell_layout.rs"]
mod shell_layout;

use shell_layout::{
    BALANCED_PROSE_WIDTH_PIXELS, INSPECTOR_MAX_WIDTH_PIXELS, INSPECTOR_MIN_WIDTH_PIXELS,
    LOOSE_PROSE_WIDTH_PIXELS, PROSE_GUTTER_PIXELS, ProseWidth, SHELL_CHROME_PIXELS,
    THREAD_RAIL_BAND_PIXELS, THREAD_RAIL_GAP_PIXELS, TIGHT_PROSE_WIDTH_PIXELS,
    inspector_column_pixels, prose_column_pixels, thread_inspector_fits_beside_rail,
};

#[test]
fn prose_widths_match_the_typescript_tokens() {
    let cases = [
        (ProseWidth::Tight, "tight", TIGHT_PROSE_WIDTH_PIXELS, 672.0),
        (
            ProseWidth::Balanced,
            "balanced",
            BALANCED_PROSE_WIDTH_PIXELS,
            768.0,
        ),
        (ProseWidth::Loose, "loose", LOOSE_PROSE_WIDTH_PIXELS, 896.0),
    ];

    assert_eq!(
        ProseWidth::ALL,
        [ProseWidth::Tight, ProseWidth::Balanced, ProseWidth::Loose,]
    );

    for (width, name, constant, expected_pixels) in cases {
        assert_eq!(width.as_str(), name);
        assert_eq!(constant, expected_pixels);
        assert_eq!(width.pixels(), expected_pixels);
        assert_eq!(prose_column_pixels(width), expected_pixels);
    }
}

#[test]
fn shell_geometry_constants_match_the_policy() {
    assert_eq!(THREAD_RAIL_BAND_PIXELS, 144.0);
    assert_eq!(THREAD_RAIL_GAP_PIXELS, 16.0);
    assert_eq!(PROSE_GUTTER_PIXELS, 32.0);
    assert_eq!(SHELL_CHROME_PIXELS, 80.0);
}

#[test]
fn inspector_clamp_has_the_expected_endpoints_and_transition() {
    let cases = [
        (-1.0, INSPECTOR_MIN_WIDTH_PIXELS),
        (0.0, INSPECTOR_MIN_WIDTH_PIXELS),
        (1023.0, INSPECTOR_MIN_WIDTH_PIXELS),
        (1024.0, INSPECTOR_MIN_WIDTH_PIXELS),
        (1025.0, 256.25),
        (1399.0, 349.75),
        (1400.0, INSPECTOR_MAX_WIDTH_PIXELS),
        (1401.0, INSPECTOR_MAX_WIDTH_PIXELS),
    ];

    for (viewport, expected) in cases {
        assert_eq!(
            inspector_column_pixels(viewport),
            expected,
            "viewport={viewport}"
        );
    }
}

#[test]
fn fit_is_inclusive_at_the_exact_boundary_and_changes_one_pixel_away() {
    // Tight and balanced cross the boundary while the inspector is in its
    // 25vw region. Loose crosses after the inspector reaches its 350px cap.
    let cases = [
        (ProseWidth::Tight, 1_258.666_666_666_666_7),
        (ProseWidth::Balanced, 1_386.666_666_666_666_7),
        (ProseWidth::Loose, 1518.0),
    ];

    for (prose_width, boundary) in cases {
        assert!(
            thread_inspector_fits_beside_rail(boundary, prose_width),
            "exact boundary should fit: prose={prose_width:?} viewport={boundary}"
        );
        assert!(
            !thread_inspector_fits_beside_rail(boundary - 1.0, prose_width),
            "one pixel below boundary should not fit: prose={prose_width:?} viewport={}",
            boundary - 1.0
        );
        assert!(
            thread_inspector_fits_beside_rail(boundary + 1.0, prose_width),
            "one pixel above boundary should fit: prose={prose_width:?} viewport={}",
            boundary + 1.0
        );
    }
}

#[test]
fn all_prose_widths_obey_the_same_inclusive_formula() {
    let viewports = [0.0, 1024.0, 1200.0, 1294.0, 1400.0, 1518.0, 2000.0];

    for viewport in viewports {
        for prose_width in ProseWidth::ALL {
            let inspector = inspector_column_pixels(viewport);
            let band = viewport
                - SHELL_CHROME_PIXELS
                - inspector
                - prose_column_pixels(prose_width)
                - PROSE_GUTTER_PIXELS
                - THREAD_RAIL_GAP_PIXELS;
            let expected = band >= THREAD_RAIL_BAND_PIXELS;

            assert_eq!(
                thread_inspector_fits_beside_rail(viewport, prose_width),
                expected,
                "viewport={viewport} prose={prose_width:?}"
            );
        }
    }
}

#[test]
fn small_viewports_cannot_fit_the_inspector_and_rail() {
    for viewport in [0.0, 1.0, 255.0, 256.0, 500.0, 1024.0] {
        for prose_width in ProseWidth::ALL {
            assert!(
                !thread_inspector_fits_beside_rail(viewport, prose_width),
                "small viewport unexpectedly fits: viewport={viewport} prose={prose_width:?}"
            );
        }
    }
}

#[test]
fn large_viewports_fit_every_prose_width_and_respect_the_maximum_clamp() {
    for viewport in [2000.0, 10_000.0, f64::MAX] {
        assert_eq!(
            inspector_column_pixels(viewport),
            INSPECTOR_MAX_WIDTH_PIXELS
        );
        for prose_width in ProseWidth::ALL {
            assert!(
                thread_inspector_fits_beside_rail(viewport, prose_width),
                "large viewport should fit: viewport={viewport} prose={prose_width:?}"
            );
        }
    }
}

#[test]
fn invalid_viewports_have_a_finite_inspector_fallback_and_never_fit() {
    for viewport in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -1.0, -10_000.0] {
        let inspector = inspector_column_pixels(viewport);
        assert_eq!(inspector, INSPECTOR_MIN_WIDTH_PIXELS, "viewport={viewport}");
        assert!(inspector.is_finite(), "viewport={viewport}");

        for prose_width in ProseWidth::ALL {
            assert!(
                !thread_inspector_fits_beside_rail(viewport, prose_width),
                "invalid viewport unexpectedly fits: viewport={viewport} prose={prose_width:?}"
            );
        }
    }
}
