//! Behavioral coverage for the native GPUI outline badge primitive.
//!
//! The recipe assertions below compare [`BadgeStyle::resolve`] output against
//! independently transcribed legacy facts (literal pixels and literal OKLCH
//! source colors), never against the primitive's own derivation, so the
//! coverage cannot pass circularly.

use artisan_ui::badge::{BadgeStyle, outline_badge};
use artisan_ui::theme::{ArtisanTheme, Oklch, RadiusStep, RadiusTokens, SurfaceStep, ThemeMode};
use gpui::{
    Context, InteractiveElement, IntoElement, ParentElement, Render, Styled, TestAppContext,
    Window, div, px,
};

const HOST_SELECTOR: &str = "badge-host";
const BADGE_SELECTOR: &str = "outline-badge-under-test";

/// Legacy facts transcribed verbatim from `badge.svelte`
/// (`h-5 gap-1 rounded-4xl px-2 py-0.5 text-xs`).
const LEGACY_HEIGHT_PX: f32 = 20.0;
const LEGACY_HORIZONTAL_PADDING_PX: f32 = 8.0;
const LEGACY_VERTICAL_PADDING_PX: f32 = 2.0;
const LEGACY_CHILD_GAP_PX: f32 = 4.0;
const LEGACY_TEXT_SIZE_PX: f32 = 12.0;
const LEGACY_LINE_HEIGHT_PX: f32 = 16.0;

/// A badge centered inside a fixed-width host row, for fitted-width,
/// fixed-height, and centering geometry.
struct FittedBadgeProbe {
    style: BadgeStyle,
}

impl Render for FittedBadgeProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .w(px(320.0))
            .h(px(80.0))
            .flex()
            .flex_row()
            .items_start()
            .justify_center()
            .debug_selector(|| HOST_SELECTOR.to_string())
            .child(
                outline_badge(self.style, "Changes").debug_selector(|| BADGE_SELECTOR.to_string()),
            )
    }
}

/// A badge whose label's natural width exceeds a narrow host row, so any
/// nonzero flex shrink would compress it below its content width.
struct NarrowHostBadgeProbe {
    style: BadgeStyle,
}

impl Render for NarrowHostBadgeProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .w(px(48.0))
            .flex()
            .flex_row()
            .debug_selector(|| HOST_SELECTOR.to_string())
            .child(
                outline_badge(self.style, "Answered question")
                    .debug_selector(|| BADGE_SELECTOR.to_string()),
            )
    }
}

/// A badge with a caller-chained max-width refinement overriding the fitted
/// recipe default, exercising the later-values-win contract under clipping.
struct CappedBadgeProbe {
    style: BadgeStyle,
}

impl Render for CappedBadgeProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(
            outline_badge(self.style, "Answered question")
                .max_w(px(64.0))
                .debug_selector(|| BADGE_SELECTOR.to_string()),
        )
    }
}

#[test]
fn outline_style_pins_exact_audited_values() {
    for mode in [ThemeMode::Light, ThemeMode::Dark] {
        let theme = ArtisanTheme::for_mode(mode);
        let style = BadgeStyle::resolve(theme);

        assert_eq!(style.height, px(LEGACY_HEIGHT_PX));
        assert_eq!(style.height, theme.density.badge_height);
        assert_eq!(style.horizontal_padding, px(LEGACY_HORIZONTAL_PADDING_PX));
        assert_eq!(style.horizontal_padding, theme.spacing.steps(2.0));
        assert_eq!(style.vertical_padding, px(LEGACY_VERTICAL_PADDING_PX));
        assert_eq!(style.vertical_padding, theme.spacing.steps(0.5));
        assert_eq!(style.child_gap, px(LEGACY_CHILD_GAP_PX));
        assert_eq!(style.child_gap, theme.spacing.steps(1.0));
        assert_eq!(
            style.corner_radius,
            px(26.0),
            "the legacy pill is the --radius-4xl ramp step"
        );
        assert_eq!(style.corner_radius, RadiusTokens::value(RadiusStep::X4l));
        assert_eq!(style.text_size, px(LEGACY_TEXT_SIZE_PX));
        assert_eq!(style.text_size, theme.typography.label_text);
        assert_eq!(
            style.line_height,
            px(LEGACY_LINE_HEIGHT_PX),
            "`text-xs` carries a 16 px leading"
        );
    }
}

#[test]
fn outline_palette_resolves_per_mode_from_exact_legacy_sources() {
    let light_style = BadgeStyle::resolve(ArtisanTheme::for_mode(ThemeMode::Light));
    let dark_style = BadgeStyle::resolve(ArtisanTheme::for_mode(ThemeMode::Dark));

    // Light background: legacy `bg-surface-100`; dark: `bg-surface-900`.
    assert_eq!(light_style.background, SurfaceStep::S100.oklch().to_paint());
    assert_eq!(dark_style.background, SurfaceStep::S900.oklch().to_paint());
    assert_ne!(
        light_style.background, dark_style.background,
        "light surface 100 and dark surface 900 must differ"
    );

    // Foreground: each mode's shared `--foreground`, transcribed from
    // `theme.css` (`oklch(0.2269 0.0045 285.823)` light,
    // `oklch(0.9006 0.0005 285.823)` dark).
    assert_eq!(
        light_style.foreground,
        Oklch::new(0.2269, 0.0045, 285.823).to_paint()
    );
    assert_eq!(
        dark_style.foreground,
        Oklch::new(0.9006, 0.0005, 285.823).to_paint()
    );
    assert_ne!(light_style.foreground, dark_style.foreground);

    // Border: legacy `border-border`. Light `--border` is surface 200; dark
    // `--border` is white at exactly 10% alpha.
    assert_eq!(light_style.border, SurfaceStep::S200.oklch().to_paint());
    assert_eq!(
        dark_style.border,
        Oklch::new(1.0, 0.0, 0.0).with_alpha(0.10).to_paint()
    );
    assert_eq!(
        dark_style.border.a.to_bits(),
        0.10_f32.to_bits(),
        "dark --border carries exactly 10% alpha"
    );
    assert_ne!(light_style.border, dark_style.border);
}

#[test]
fn compile_only_constructor_chains_styled_refinements() {
    // Compile-only API-shape evidence, mirroring the card coverage: one
    // resolved recipe feeds the constructor, later refinements chain onto the
    // returned Div, and the visible label stays owned by the badge.
    let style = BadgeStyle::resolve(ArtisanTheme::for_mode(ThemeMode::Dark));
    let refined = outline_badge(style, "Changes").py(px(2.0)).gap(px(8.0));
    let _ = refined;
}

#[gpui::test]
fn rendered_badge_is_20px_tall_fitted_and_centered(cx: &mut TestAppContext) {
    let (_, cx) = cx.add_window_view(|_, _| FittedBadgeProbe {
        style: BadgeStyle::resolve(ArtisanTheme::for_mode(ThemeMode::Dark)),
    });

    let host = cx
        .debug_bounds(HOST_SELECTOR)
        .expect("host must paint inspectable bounds");
    let badge = cx
        .debug_bounds(BADGE_SELECTOR)
        .expect("badge must paint inspectable bounds");

    // The fixed legacy height is honored exactly.
    assert_eq!(badge.size.height, px(LEGACY_HEIGHT_PX));
    // The badge hugs its content instead of filling the 320 px host.
    assert!(
        badge.size.width < host.size.width,
        "the fitted badge must not grow to the host width"
    );
    // The host centers the badge horizontally; its top edge sits at the top.
    assert_eq!(
        badge.origin.x - host.origin.x,
        (host.size.width - badge.size.width) / 2.0
    );
    assert_eq!(badge.origin.y - host.origin.y, px(0.0));
}

#[gpui::test]
fn badge_refuses_to_shrink_and_stays_on_one_line_in_a_narrow_host(cx: &mut TestAppContext) {
    let (_, cx) = cx.add_window_view(|_, _| NarrowHostBadgeProbe {
        style: BadgeStyle::resolve(ArtisanTheme::for_mode(ThemeMode::Light)),
    });

    let host = cx
        .debug_bounds(HOST_SELECTOR)
        .expect("host must paint inspectable bounds");
    let badge = cx
        .debug_bounds(BADGE_SELECTOR)
        .expect("badge must paint inspectable bounds");

    // Zero flex shrink keeps the content width even under extreme pressure;
    // any positive shrink factor would clamp the item to the 48 px host.
    assert!(
        badge.size.width > host.size.width,
        "the badge must overflow the host rather than compress ({badge:?} in {host:?})"
    );
    // `whitespace_nowrap` keeps the two-word label on one line at exactly
    // the fixed height.
    assert_eq!(badge.size.height, px(LEGACY_HEIGHT_PX));
}

#[gpui::test]
fn caller_max_width_refinement_caps_the_fitted_width(cx: &mut TestAppContext) {
    let (_, cx) = cx.add_window_view(|_, _| CappedBadgeProbe {
        style: BadgeStyle::resolve(ArtisanTheme::for_mode(ThemeMode::Dark)),
    });

    let badge = cx
        .debug_bounds(BADGE_SELECTOR)
        .expect("badge must paint inspectable bounds");

    // The caller's chained refinement wins over the fitted recipe default.
    assert_eq!(badge.size.width, px(64.0));
    // The capped badge still holds the fixed legacy height under clipping.
    assert_eq!(badge.size.height, px(LEGACY_HEIGHT_PX));
}
