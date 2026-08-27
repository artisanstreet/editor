//! Behavioral and native-render coverage for the GPUI `FadeArc` primitive.
//!
//! The deterministic assertions compare geometry, phase, motion, state, and
//! theme decisions independently of GPUI's wall clock. The final probe enters
//! the real GPUI canvas/path paint lifecycle; its bounds assertion deliberately
//! does not claim screenshot or platform-accessibility fidelity.

use std::time::Duration;

use artisan_ui::fade_arc::{
    DEFAULT_DURATION, DEFAULT_PHASE, DEFAULT_SIZE, DEFAULT_STATUS_LABEL, FADE_ALPHA, FadeArc,
    FadeArcGeometry, FadeArcMotionPlan, FadeArcSegment, FadeArcStyle, LEADING_START_DEGREES,
    LEADING_SWEEP_DEGREES, TRAILING_SWEEP_DEGREES, normalize_phase, phase_at, rotation_degrees,
};
use artisan_ui::motion::MotionPolicy;
use artisan_ui::theme::{ArtisanTheme, Oklch, ThemeMode};
use gpui::{Context, IntoElement, ParentElement, Render, Styled, TestAppContext, Window, div, px};

const RENDER_SELECTOR: &str = "fade-arc-render-probe";

fn assert_f32_eq(actual: f32, expected: f32) {
    assert_eq!(actual.to_bits(), expected.to_bits());
}

#[test]
fn defaults_resolve_the_reached_loading_recipe() {
    let theme = ArtisanTheme::for_mode(ThemeMode::Dark);
    let arc = FadeArc::new("fade-arc-default", theme);

    assert_eq!(arc.size_value(), DEFAULT_SIZE);
    assert_eq!(arc.duration_value(), DEFAULT_DURATION);
    assert_f32_eq(arc.initial_phase(), DEFAULT_PHASE);
    assert!(arc.is_active());
    assert_eq!(arc.motion_policy_value(), MotionPolicy::Full);
    assert_eq!(arc.status_label_value(), DEFAULT_STATUS_LABEL);
    assert!(arc.motion_plan().is_animated());
    assert_eq!(
        arc.visual_style().color,
        theme.colors.muted_foreground.to_paint()
    );
}

#[test]
fn builder_overrides_keep_state_and_style_configurable() {
    let explicit_color = Oklch::new(0.6, 0.1, 30.0).to_paint();
    let arc = FadeArc::new(
        "fade-arc-overrides",
        ArtisanTheme::for_mode(ThemeMode::Light),
    )
    .size(px(14.0))
    .duration(Duration::from_millis(750))
    .active(false)
    .motion_policy(MotionPolicy::Reduced)
    .phase(-0.25)
    .status_label("Refreshing engine usage")
    .color(explicit_color)
    .debug_selector(RENDER_SELECTOR)
    .opacity(0.8);

    assert_eq!(arc.size_value(), px(14.0));
    assert_eq!(arc.duration_value(), Duration::from_millis(750));
    assert!(!arc.is_active());
    assert_eq!(arc.motion_policy_value(), MotionPolicy::Reduced);
    assert_f32_eq(arc.initial_phase(), 0.75);
    assert_eq!(arc.status_label_value(), "Refreshing engine usage");
    assert_eq!(arc.visual_style().color, explicit_color);
    assert!(!arc.motion_plan().is_animated());
}

#[test]
fn phases_normalize_and_rotate_deterministically() {
    assert_f32_eq(normalize_phase(-0.25), 0.75);
    assert_f32_eq(normalize_phase(2.25), 0.25);
    assert_f32_eq(normalize_phase(f32::NAN), 0.0);
    assert_f32_eq(normalize_phase(f32::INFINITY), 0.0);

    assert_f32_eq(
        phase_at(Duration::from_millis(500), DEFAULT_DURATION, 0.25),
        0.75,
    );
    assert_f32_eq(
        phase_at(Duration::from_millis(1500), DEFAULT_DURATION, 0.25),
        0.75,
    );
    assert_f32_eq(phase_at(Duration::from_secs(1), Duration::ZERO, 0.25), 0.25);
    assert_f32_eq(rotation_degrees(0.5), 180.0);

    let geometry = FadeArcGeometry::for_size(DEFAULT_SIZE);
    assert_eq!(geometry.size, DEFAULT_SIZE);
    assert_eq!(geometry.stroke_width, px(3.0));
    assert_eq!(
        geometry.segment_angles(FadeArcSegment::Leading, 0.0),
        (
            LEADING_START_DEGREES,
            LEADING_START_DEGREES + LEADING_SWEEP_DEGREES
        )
    );
    assert_eq!(
        geometry.segment_angles(FadeArcSegment::Trailing, 0.0),
        (
            LEADING_START_DEGREES + LEADING_SWEEP_DEGREES,
            LEADING_START_DEGREES + LEADING_SWEEP_DEGREES + TRAILING_SWEEP_DEGREES
        )
    );
    assert_f32_eq(
        geometry.segment_angles(FadeArcSegment::Leading, 0.5).0,
        90.0,
    );
}

#[test]
fn active_reduced_inactive_and_zero_duration_motion_are_distinct() {
    let full = FadeArcMotionPlan::for_state(true, MotionPolicy::Full, DEFAULT_DURATION, 0.25);
    assert!(full.is_animated());

    let animation = full.animation().expect("full active motion must animate");
    assert_eq!(animation.duration(), DEFAULT_DURATION);
    assert_f32_eq(animation.initial_phase(), 0.25);
    let clock = animation.gpui_animation();
    assert!(!clock.oneshot);
    assert_f32_eq((clock.easing)(0.5), 0.5);

    let reduced = FadeArcMotionPlan::for_state(true, MotionPolicy::Reduced, DEFAULT_DURATION, 0.25);
    assert_eq!(reduced, FadeArcMotionPlan::Static { phase: 0.25 });
    assert!(!reduced.is_animated());
    assert_eq!(reduced.animation(), None);

    let inactive = FadeArcMotionPlan::for_state(false, MotionPolicy::Full, DEFAULT_DURATION, 0.25);
    assert_eq!(inactive, FadeArcMotionPlan::Static { phase: 0.25 });
    assert!(!inactive.is_animated());

    let zero_duration =
        FadeArcMotionPlan::for_state(true, MotionPolicy::Full, Duration::ZERO, 0.25);
    assert_eq!(zero_duration, FadeArcMotionPlan::Static { phase: 0.25 });
}

#[test]
fn theme_colors_drive_both_fading_gradient_stops() {
    let light = ArtisanTheme::for_mode(ThemeMode::Light);
    let dark = ArtisanTheme::for_mode(ThemeMode::Dark);
    let light_style = FadeArcStyle::resolve(light);
    let dark_style = FadeArcStyle::resolve(dark);

    assert_eq!(light_style.color, light.colors.muted_foreground.to_paint());
    assert_eq!(dark_style.color, dark.colors.muted_foreground.to_paint());
    assert_ne!(light_style.color, dark_style.color);

    for style in [light_style, dark_style] {
        assert_eq!(style.leading.start, style.color);
        assert_eq!(style.leading.end, style.color.opacity(FADE_ALPHA));
        assert_eq!(style.trailing.start, style.color.opacity(0.0));
        assert_eq!(style.trailing.end, style.color.opacity(FADE_ALPHA));
        assert_ne!(style.leading.background(), style.trailing.background());
    }
}

#[test]
fn semantic_status_state_is_retained_for_future_accessibility() {
    let arc = FadeArc::new("fade-arc-state", ArtisanTheme::for_mode(ThemeMode::Dark))
        .active(false)
        .status_label("Loading thread");

    assert!(!arc.state().is_active());
    assert_eq!(arc.state().status_label(), "Loading thread");
    assert_eq!(arc.status_label_value(), "Loading thread");
}

#[test]
fn instances_have_independent_animation_ids_and_phases() {
    let theme = ArtisanTheme::for_mode(ThemeMode::Dark);
    let first = FadeArc::new("fade-arc-first", theme).phase(0.1);
    let second = FadeArc::new("fade-arc-second", theme).phase(0.6);

    assert_ne!(first.animation_id(), second.animation_id());
    assert_ne!(first.element_id(), second.element_id());
    assert_ne!(
        first.motion_plan().phase().to_bits(),
        second.motion_plan().phase().to_bits()
    );
    assert_eq!(first.visual_style(), second.visual_style());
}

struct FadeArcRenderProbe;

impl Render for FadeArcRenderProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(
            FadeArc::new("fade-arc-render", ArtisanTheme::for_mode(ThemeMode::Dark))
                .size(px(24.0))
                .active(false)
                .motion_policy(MotionPolicy::Reduced)
                .debug_selector(RENDER_SELECTOR),
        )
    }
}

#[gpui::test]
fn reduced_static_arc_reaches_the_real_gpui_canvas_paint_probe(cx: &mut TestAppContext) {
    let (_, cx) = cx.add_window_view(|_, _| FadeArcRenderProbe);

    let bounds = cx
        .debug_bounds(RENDER_SELECTOR)
        .expect("the static FadeArc canvas must expose debug bounds");
    assert_eq!(bounds.size.width, px(24.0));
    assert_eq!(bounds.size.height, px(24.0));
}
