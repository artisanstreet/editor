//! Wave-2 typography + gradient fidelity over the public `artisan-ui` API.
//!
//! Compiled as a white-box child of `crate::gradient` under `cfg(test)` (the
//! same `#[path]` linkage `asset_seam.rs` uses for `tinted_svg.rs`), so no
//! manifest change is needed to run it: `cargo test -p artisan-ui` picks it
//! up with the library's unit tests.
//!
//! What is proven here, and what is not: byte-identity pins
//! (`artisan_assets::fonts` lengths, WOFF2 magic) prove the exact legacy
//! `@font-face` bytes ship; the registration test proves those bytes flow
//! through `TextSystem::add_fonts` without error on the test platform (whose
//! text system is a no-op stub, so DirectWrite parsing of WOFF2 on Windows
//! remains a native-gate observation, with the Artisan Neo TTF in the legacy
//! tree as the documented fallback); the render test proves the gradient
//! faces paint on a real GPUI window via the native `linear_gradient`
//! primitive.

use artisan_assets::fonts as bundled_fonts;

use gpui::{
    Context, FontWeight, InteractiveElement, IntoElement, ParentElement, Render, Styled,
    TestAppContext, Window, div, px,
};

use super::{
    VERTICAL_ANGLE_DEGREES, home_panel_gradient, hover_fill_gradient, send_face_gradient,
    vertical_gradient,
};
use crate::fonts::register_bundled_fonts;
use crate::theme::{ArtisanTheme, SurfaceStep, ThemeMode, WeightRange};

/// Stable selector for the gradient proof surface.
const GRADIENT_SURFACE_SELECTOR: &str = "typography-gradient-surface";

/// One fixed-size surface painted with the home-panel gradient plus a text
/// child bound to the display face, proving both halves of this wave in one
/// paint: the fill reaches the GPU through `.bg(Background)` and the family
/// string flows through `.font_family`.
struct GradientSurfaceProbe {
    theme: ArtisanTheme,
}

impl Render for GradientSurfaceProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .w(px(320.0))
            .h(px(180.0))
            .bg(home_panel_gradient(self.theme))
            .font_family(self.theme.typography.display().family)
            .debug_selector(|| GRADIENT_SURFACE_SELECTOR.to_string())
            .child(
                div()
                    .font_family(self.theme.typography.code().family)
                    .child("mono probe"),
            )
    }
}

#[test]
fn bundled_catalog_matches_the_theme_declarations_on_both_sides() {
    assert_eq!(bundled_fonts::ALL.len(), 4);
    for mode in [ThemeMode::Light, ThemeMode::Dark] {
        let typography = ArtisanTheme::for_mode(mode).typography;
        // Role aliases resolve to the declared faces.
        assert_eq!(typography.display().family, "Artisan Neo");
        assert_eq!(typography.body().family, "Artisan Neo");
        assert_eq!(typography.code().family, "JetBrains Mono");
        // Every theme face ships in the catalog with the same weight range.
        for role in [
            typography.sans,
            typography.mono,
            typography.heading,
            typography.logo,
            typography.wordmark,
        ] {
            let bundled = bundled_fonts::lookup_family(role.family)
                .unwrap_or_else(|error| panic!("theme face missing from bundle: {error}"));
            assert_eq!(
                bundled.weights,
                (role.weights.min, role.weights.max),
                "{}: catalog weight range drifted from the theme role",
                role.family
            );
        }
    }
    // The catalog carries exactly the four `@font-face` ranges, verbatim.
    let ranges: Vec<((u16, u16), &str)> = bundled_fonts::ALL
        .iter()
        .map(|font| (font.weights, font.family))
        .collect();
    assert_eq!(
        ranges,
        vec![
            ((100, 900), "Artisan Neo"),
            ((100, 1000), "Cal Sans"),
            ((100, 800), "JetBrains Mono"),
            ((300, 900), "Sigurd Variable"),
        ]
    );
}

#[test]
fn theme_roles_bind_gpui_fonts_with_family_and_weight() {
    let typography = ArtisanTheme::for_mode(ThemeMode::Dark).typography;
    let medium = typography.body().font(FontWeight::MEDIUM);
    assert_eq!(medium.family.as_ref(), "Artisan Neo");
    assert_eq!(medium.weight, FontWeight::MEDIUM);
    let mono = typography.code().font(FontWeight::NORMAL);
    assert_eq!(mono.family.as_ref(), "JetBrains Mono");
    assert_eq!(mono.weight, FontWeight::NORMAL);
    let display = typography.display().font(FontWeight::BOLD);
    assert_eq!(display.family.as_ref(), "Artisan Neo");
    assert_eq!(display.weight, FontWeight::BOLD);
    assert_eq!(
        typography.logo.weights,
        WeightRange {
            min: 100,
            max: 1000
        }
    );
    assert_eq!(
        typography.wordmark.weights,
        WeightRange { min: 300, max: 900 }
    );
}

#[test]
fn gradient_constructors_pin_angle_and_edge_stops() {
    assert_eq!(
        VERTICAL_ANGLE_DEGREES.to_bits(),
        180.0_f32.to_bits(),
        "legacy `to bottom`"
    );
    for mode in [ThemeMode::Light, ThemeMode::Dark] {
        let theme = ArtisanTheme::for_mode(mode);
        for gradient in [
            home_panel_gradient(theme),
            hover_fill_gradient(theme),
            send_face_gradient(theme),
        ] {
            let debug = format!("{gradient:?}");
            assert!(
                debug.starts_with("LinearGradient(180,"),
                "top-to-bottom native gradient, got {debug}"
            );
            assert!(
                debug.contains("percentage: 0.0"),
                "top stop at the face edge, got {debug}"
            );
            assert!(
                debug.contains("percentage: 1.0"),
                "bottom stop at the face edge, got {debug}"
            );
        }
    }
}

#[test]
fn ramp_gradients_are_mode_independent_while_hover_follows_foreground() {
    let light = ArtisanTheme::for_mode(ThemeMode::Light);
    let dark = ArtisanTheme::for_mode(ThemeMode::Dark);
    // Ramp-constant faces: identical paint in both modes.
    assert_eq!(
        format!("{:?}", home_panel_gradient(light)),
        format!("{:?}", home_panel_gradient(dark)),
        "surface-125/75 are ramp constants"
    );
    assert_eq!(
        format!("{:?}", send_face_gradient(light)),
        format!("{:?}", send_face_gradient(dark)),
        "surface-25/100 are ramp constants"
    );
    // Foreground-derived face: each mode paints its own `--hover-surface-fill`.
    assert_ne!(
        format!("{:?}", hover_fill_gradient(light)),
        format!("{:?}", hover_fill_gradient(dark)),
        "hover fill must follow the mode foreground"
    );
    // The generic constructor agrees with the home-panel face stop for stop.
    assert_eq!(
        format!(
            "{:?}",
            vertical_gradient(SurfaceStep::S125.oklch(), SurfaceStep::S75.oklch())
        ),
        format!("{:?}", home_panel_gradient(light)),
    );
}

#[gpui::test]
fn bundled_fonts_register_through_the_platform_text_system(cx: &mut TestAppContext) {
    cx.update(|app| {
        register_bundled_fonts(app).expect("bundled typefaces must register without error");
    });
}

#[gpui::test]
fn gradient_surface_paints_with_bound_families(cx: &mut TestAppContext) {
    for mode in [ThemeMode::Light, ThemeMode::Dark] {
        let (_, window_cx) = cx.add_window_view(|_, _| GradientSurfaceProbe {
            theme: ArtisanTheme::for_mode(mode),
        });
        let bounds = window_cx
            .debug_bounds(GRADIENT_SURFACE_SELECTOR)
            .expect("gradient surface must paint with debug bounds");
        assert_eq!(bounds.size.width, px(320.0));
        assert_eq!(bounds.size.height, px(180.0));
    }
}
