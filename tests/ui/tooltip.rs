//! Behavioral coverage for the native tooltip presentation/timing policy.
//!
//! The assertions below compare resolved output against independently
//! transcribed legacy facts (literal pixels, literal durations, literal OKLCH
//! source colors), never against the primitive's own derivation, so the
//! coverage cannot pass circularly.

use std::time::Duration;

use artisan_ui::motion::{MotionCurve, MotionPlan, MotionPolicy, MotionRecipe};
use artisan_ui::theme::{ArtisanTheme, Oklch, RadiusStep, RadiusTokens, SurfaceStep, ThemeMode};
use artisan_ui::tooltip::{TooltipPhase, TooltipStyle, tooltip_content};
use gpui::{
    Context, InteractiveElement, IntoElement, ParentElement, Render, Styled, TestAppContext,
    Window, div, px,
};

const HOST_SELECTOR: &str = "tooltip-host";
const BUBBLE_SELECTOR: &str = "tooltip-bubble-under-test";

fn assert_close(actual: f32, expected: f32) {
    assert!(
        (actual - expected).abs() <= f32::EPSILON,
        "expected {expected}, got {actual}"
    );
}

/// Legacy facts transcribed verbatim from `tooltip-content.svelte`
/// (`rounded-2xl px-3 py-1.5 gap-1.5 text-xs max-w-xs`) and the pinned
/// tailwindcss 4.3.2 theme (`--container-xs: 20rem`; `text-xs` carries a
/// 16 px leading).
const LEGACY_CORNER_RADIUS_PX: f32 = 18.0;
const LEGACY_HORIZONTAL_PADDING_PX: f32 = 12.0;
const LEGACY_VERTICAL_PADDING_PX: f32 = 6.0;
const LEGACY_CHILD_GAP_PX: f32 = 6.0;
const LEGACY_TEXT_SIZE_PX: f32 = 12.0;
const LEGACY_LINE_HEIGHT_PX: f32 = 16.0;
const LEGACY_MAX_WIDTH_PX: f32 = 320.0;

/// Authoritative motion facts (transitions.dev tooltip tokens as encoded by
/// the shared recipes): in = 150 ms + 80 ms delay + ease-out, out =
/// 50 ms + no delay + ease-out.
const ENTRANCE_DURATION_MS: u64 = 150;
const ENTRANCE_DELAY_MS: u64 = 80;
const EXIT_DURATION_MS: u64 = 50;

/// A short-label bubble centered in a wide host, for fitted-width and exact
/// single-line geometry.
struct FittedBubbleProbe {
    style: TooltipStyle,
}

impl Render for FittedBubbleProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .w(px(480.0))
            .h(px(120.0))
            .flex()
            .flex_row()
            .items_start()
            .justify_center()
            .debug_selector(|| HOST_SELECTOR.to_string())
            .child(
                tooltip_content(self.style, "Send message")
                    .debug_selector(|| BUBBLE_SELECTOR.to_string()),
            )
    }
}

/// An unbreakable label whose natural width far exceeds the recipe cap, so
/// the fitted bubble must stop exactly at `max-w-xs`.
struct CappedBubbleProbe {
    style: TooltipStyle,
}

impl Render for CappedBubbleProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(
            tooltip_content(self.style, "w".repeat(240))
                .debug_selector(|| BUBBLE_SELECTOR.to_string()),
        )
    }
}

/// A caller-chained max-width refinement overriding the recipe cap,
/// exercising the later-values-win contract.
struct OverrideBubbleProbe {
    style: TooltipStyle,
}

impl Render for OverrideBubbleProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(
            tooltip_content(self.style, "w".repeat(240))
                .max_w(px(128.0))
                .debug_selector(|| BUBBLE_SELECTOR.to_string()),
        )
    }
}

#[test]
fn phases_bind_to_the_audited_motion_recipes() {
    assert_eq!(
        TooltipPhase::Entrance.recipe(),
        MotionRecipe::TooltipIn,
        "the entrance owns the TooltipIn recipe"
    );
    assert_eq!(
        TooltipPhase::Exit.recipe(),
        MotionRecipe::TooltipOut,
        "the exit owns the TooltipOut recipe"
    );
}

#[test]
fn full_motion_keeps_the_authoritative_entrance_and_exit_timing() {
    let entrance = TooltipPhase::Entrance
        .plan(MotionPolicy::Full)
        .animation()
        .expect("full-motion entrance must animate");
    assert_eq!(
        entrance.duration(),
        Duration::from_millis(ENTRANCE_DURATION_MS)
    );
    assert_eq!(entrance.delay(), Duration::from_millis(ENTRANCE_DELAY_MS));
    assert_eq!(entrance.curve(), MotionCurve::EaseOut);

    let exit = TooltipPhase::Exit
        .plan(MotionPolicy::Full)
        .animation()
        .expect("full-motion exit must animate");
    assert_eq!(exit.duration(), Duration::from_millis(EXIT_DURATION_MS));
    assert_eq!(
        exit.delay(),
        Duration::ZERO,
        "the exit recipe carries no delay"
    );
    assert_eq!(exit.curve(), MotionCurve::EaseOut);
}

#[test]
fn reduced_motion_collapses_both_phases_immediately() {
    for phase in [TooltipPhase::Entrance, TooltipPhase::Exit] {
        assert_eq!(
            phase.plan(MotionPolicy::Reduced),
            MotionPlan::Immediate,
            "reduced motion must not animate or wait"
        );
        assert_eq!(phase.plan(MotionPolicy::Reduced).animation(), None);
    }
}

#[test]
fn style_pins_exact_audited_values_in_both_modes() {
    for mode in [ThemeMode::Light, ThemeMode::Dark] {
        let theme = ArtisanTheme::for_mode(mode);
        let style = TooltipStyle::resolve(theme);

        assert_eq!(
            style.corner_radius,
            px(LEGACY_CORNER_RADIUS_PX),
            "the bubble is the --radius-2xl ramp step"
        );
        assert_eq!(style.corner_radius, RadiusTokens::value(RadiusStep::X2l));
        assert_eq!(style.horizontal_padding, px(LEGACY_HORIZONTAL_PADDING_PX));
        assert_eq!(style.horizontal_padding, theme.spacing.steps(3.0));
        assert_eq!(style.vertical_padding, px(LEGACY_VERTICAL_PADDING_PX));
        assert_eq!(style.vertical_padding, theme.spacing.steps(1.5));
        assert_eq!(style.child_gap, px(LEGACY_CHILD_GAP_PX));
        assert_eq!(style.child_gap, theme.spacing.steps(1.5));
        assert_eq!(style.text_size, px(LEGACY_TEXT_SIZE_PX));
        assert_eq!(style.text_size, theme.typography.label_text);
        assert_eq!(
            style.line_height,
            px(LEGACY_LINE_HEIGHT_PX),
            "`text-xs` carries a 16 px leading"
        );
        assert_eq!(style.max_width, px(LEGACY_MAX_WIDTH_PX));
    }
}

#[test]
fn inverted_palette_resolves_per_mode_from_exact_legacy_sources() {
    let light_theme = ArtisanTheme::for_mode(ThemeMode::Light);
    let dark_theme = ArtisanTheme::for_mode(ThemeMode::Dark);
    let light_style = TooltipStyle::resolve(light_theme);
    let dark_style = TooltipStyle::resolve(dark_theme);

    // Background is each mode's shared `--foreground`, transcribed from
    // `theme.css` (`oklch(0.2269 0.0045 285.823)` light,
    // `oklch(0.9006 0.0005 285.823)` dark).
    assert_eq!(
        light_style.background,
        Oklch::new(0.2269, 0.0045, 285.823).to_paint()
    );
    assert_eq!(
        dark_style.background,
        Oklch::new(0.9006, 0.0005, 285.823).to_paint()
    );

    // Label color is each mode's `--background`: surface 0 light, surface
    // 950 dark.
    assert_eq!(light_style.foreground, SurfaceStep::S0.oklch().to_paint());
    assert_eq!(dark_style.foreground, SurfaceStep::S950.oklch().to_paint());

    // Both roles stay inverted per mode and differ across modes.
    assert_eq!(
        light_style.background,
        light_theme.colors.foreground.to_paint()
    );
    assert_eq!(
        dark_style.background,
        dark_theme.colors.foreground.to_paint()
    );
    assert_ne!(
        light_style.background, dark_style.background,
        "light and dark inverted fills must differ"
    );
    assert_ne!(light_style.foreground, dark_style.foreground);
}

#[test]
fn caret_defaults_to_the_legacy_geometry_and_can_opt_out() {
    let style = TooltipStyle::resolve(ArtisanTheme::for_mode(ThemeMode::Dark));

    let arrow = style.arrow.expect("the reached default keeps its caret");
    assert_eq!(arrow.edge, px(10.0), "`size-2.5` is a 10 px square");
    assert_eq!(
        arrow.corner_radius,
        RadiusTokens::value(RadiusStep::Xs),
        "`rounded-xs` is the 4 px ramp step"
    );
    assert_close(arrow.rotation_degrees, 45.0);

    // Layered/glass materials opt out wholesale, exactly like the audited
    // `arrow={false}` call site.
    let mut bare = style;
    bare.arrow = None;
    assert_eq!(bare.arrow, None);
    assert_eq!(
        bare,
        TooltipStyle {
            arrow: None,
            ..style
        },
        "opting out of the caret changes nothing else"
    );
}

#[test]
fn constructor_chains_styled_refinements() {
    // Compile-only API-shape evidence, mirroring the badge coverage: one
    // resolved recipe feeds the constructor, later refinements chain onto the
    // returned Div, and the visible label stays owned by the bubble.
    let style = TooltipStyle::resolve(ArtisanTheme::for_mode(ThemeMode::Dark));
    let refined = tooltip_content(style, "Blocked").px(px(4.0)).gap(px(8.0));
    let _ = refined;
}

#[gpui::test]
fn rendered_bubble_hugs_content_at_the_exact_single_line_height(cx: &mut TestAppContext) {
    let (_, cx) = cx.add_window_view(|_, _| FittedBubbleProbe {
        style: TooltipStyle::resolve(ArtisanTheme::for_mode(ThemeMode::Dark)),
    });

    let host = cx
        .debug_bounds(HOST_SELECTOR)
        .expect("host must paint inspectable bounds");
    let bubble = cx
        .debug_bounds(BUBBLE_SELECTOR)
        .expect("bubble must paint inspectable bounds");

    // Vertical padding plus one 16 px text line.
    let expected_height = px(LEGACY_VERTICAL_PADDING_PX * 2.0 + LEGACY_LINE_HEIGHT_PX);
    assert_eq!(bubble.size.height, expected_height);
    // The bubble hugs its label instead of filling the 480 px host.
    assert!(
        bubble.size.width < host.size.width,
        "the fitted bubble must not grow to the host width"
    );
    // The host centers the bubble horizontally; its top edge sits at the top.
    assert_eq!(
        bubble.origin.x - host.origin.x,
        (host.size.width - bubble.size.width) / 2.0
    );
    assert_eq!(bubble.origin.y - host.origin.y, px(0.0));
}

#[gpui::test]
fn long_label_stops_exactly_at_the_legacy_max_width(cx: &mut TestAppContext) {
    let (_, cx) = cx.add_window_view(|_, _| CappedBubbleProbe {
        style: TooltipStyle::resolve(ArtisanTheme::for_mode(ThemeMode::Light)),
    });

    let bubble = cx
        .debug_bounds(BUBBLE_SELECTOR)
        .expect("bubble must paint inspectable bounds");

    assert_eq!(
        bubble.size.width,
        px(LEGACY_MAX_WIDTH_PX),
        "`max-w-xs` caps the fitted bubble at 320 px"
    );
}

#[gpui::test]
fn caller_max_width_refinement_overrides_the_recipe_cap(cx: &mut TestAppContext) {
    let (_, cx) = cx.add_window_view(|_, _| OverrideBubbleProbe {
        style: TooltipStyle::resolve(ArtisanTheme::for_mode(ThemeMode::Dark)),
    });

    let bubble = cx
        .debug_bounds(BUBBLE_SELECTOR)
        .expect("bubble must paint inspectable bounds");

    assert_eq!(
        bubble.size.width,
        px(128.0),
        "later refinements win over the recipe default"
    );
}
