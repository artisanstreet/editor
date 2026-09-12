//! Behavioral coverage for the native GPUI `ShimmerText` primitive.
//!
//! Covers the reference gradient geometry and sRGB mix, timing, readable
//! reduced-motion states, and native layout bounds.

use std::time::Duration;

use artisan_ui::motion::MotionPolicy;
use artisan_ui::shimmer_text::{
    DEFAULT_DELAY, DEFAULT_DURATION, DEFAULT_SPREAD, ShimmerMotionPlan, ShimmerText,
    ShimmerTextStyle, ShimmerTextVariant, ShimmerTiming, phase_at,
};
use artisan_ui::theme::{ArtisanTheme, ThemeMode};
use gpui::{
    Context, InteractiveElement as _, IntoElement, ParentElement, Render, Styled, TestAppContext,
    Window, div, px,
};

const HOST_SELECTOR: &str = "shimmer-text-host";
const TEXT_SELECTOR: &str = "shimmer-text-under-test";

#[test]
fn defaults_and_builders_preserve_the_audited_contract() {
    let theme = ArtisanTheme::for_mode(ThemeMode::Dark);
    let text = ShimmerText::new("Working", theme, MotionPolicy::Full)
        .duration(Duration::from_secs(4))
        .delay(Duration::from_millis(250))
        .spread(72.0)
        .variant(ShimmerTextVariant::Blue)
        .semantic_label("Working status")
        .active(false);

    let defaults = ShimmerTiming::default();
    assert_eq!(defaults.duration(), DEFAULT_DURATION);
    assert_eq!(defaults.delay(), DEFAULT_DELAY);
    assert_eq!(defaults.spread().to_bits(), DEFAULT_SPREAD.to_bits());
    assert_eq!(text.content(), "Working");
    assert_eq!(text.duration_value(), Duration::from_secs(4));
    assert_eq!(text.delay_value(), Duration::from_millis(250));
    assert_eq!(text.spread_value().to_bits(), 72.0_f32.to_bits());
    assert_eq!(text.selected_variant(), ShimmerTextVariant::Blue);
    assert_eq!(text.semantic_status_label(), "Working status");
    assert!(!text.is_active());
}

#[test]
fn every_public_variant_resolves_in_both_theme_modes() {
    for mode in [ThemeMode::Light, ThemeMode::Dark] {
        let theme = ArtisanTheme::for_mode(mode);
        for variant in ShimmerTextVariant::ALL {
            let expected_foreground = match variant {
                ShimmerTextVariant::Default => theme.colors.foreground,
                ShimmerTextVariant::Secondary => theme.colors.secondary_foreground,
                ShimmerTextVariant::Destructive => theme.colors.destructive,
                ShimmerTextVariant::Red => theme.colors.banner_error,
                ShimmerTextVariant::Blue | ShimmerTextVariant::Indigo => theme.colors.banner_info,
                ShimmerTextVariant::Green
                | ShimmerTextVariant::Lime
                | ShimmerTextVariant::Emerald => theme.colors.banner_success,
                ShimmerTextVariant::Yellow => theme.colors.banner_warning,
                ShimmerTextVariant::Purple
                | ShimmerTextVariant::Violet
                | ShimmerTextVariant::Fuchsia => theme.colors.question_from,
                ShimmerTextVariant::Pink | ShimmerTextVariant::Rose => theme.colors.question_to,
                ShimmerTextVariant::Orange | ShimmerTextVariant::Amber => theme.colors.favorite,
                ShimmerTextVariant::Cyan | ShimmerTextVariant::Sky => theme.colors.unread,
                ShimmerTextVariant::Slate => theme.colors.muted_foreground,
            };

            let style = ShimmerTextStyle::resolve(theme, variant);
            assert_eq!(
                style.foreground,
                expected_foreground.to_paint(),
                "{mode:?} {variant:?} foreground"
            );
            assert_eq!(
                style.highlight,
                artisan_ui::shimmer_text::shimmer_color(style.foreground, mode, 1.0)
            );
            assert!(style.foreground.alpha > 0.0);
        }
    }

    let light = ShimmerTextVariant::Default.resolve(ArtisanTheme::for_mode(ThemeMode::Light));
    let dark = ShimmerTextVariant::Default.resolve(ArtisanTheme::for_mode(ThemeMode::Dark));
    assert_ne!(light.foreground, dark.foreground);
}

#[test]
fn phase_honors_delay_and_wraps_deterministically() {
    let duration = Duration::from_secs(3);
    let delay = Duration::from_secs(1);

    assert_eq!(
        phase_at(Duration::ZERO, duration, delay).to_bits(),
        0.0_f32.to_bits()
    );
    assert_eq!(
        phase_at(Duration::from_secs(1), duration, delay).to_bits(),
        0.0_f32.to_bits()
    );
    assert_eq!(
        phase_at(Duration::from_millis(2_500), duration, delay).to_bits(),
        0.5_f32.to_bits()
    );
    assert_eq!(
        phase_at(Duration::from_millis(4_000), duration, delay).to_bits(),
        0.0_f32.to_bits()
    );
    assert_eq!(
        phase_at(Duration::from_millis(4_750), duration, delay).to_bits(),
        0.25_f32.to_bits()
    );
    assert_eq!(
        phase_at(Duration::from_secs(20), duration, delay).to_bits(),
        (19.0_f32 / 3.0).fract().to_bits()
    );
}

#[test]
fn timing_bounds_invalid_spread() {
    assert_eq!(
        ShimmerTiming::default()
            .with_spread(-20.0)
            .spread()
            .to_bits(),
        0.0_f32.to_bits()
    );
    assert_eq!(
        ShimmerTiming::default()
            .with_spread(140.0)
            .spread()
            .to_bits(),
        100.0_f32.to_bits()
    );
    assert_eq!(
        ShimmerTiming::default()
            .with_spread(f32::NAN)
            .spread()
            .to_bits(),
        0.0_f32.to_bits()
    );
}

#[test]
fn inactive_and_reduced_motion_are_immediate_but_keep_semantic_content() {
    let theme = ArtisanTheme::for_mode(ThemeMode::Light);
    let inactive = ShimmerText::new("Still here", theme, MotionPolicy::Full)
        .active(false)
        .status_label("Finished");
    assert_eq!(inactive.motion_plan(), ShimmerMotionPlan::Immediate);
    assert_eq!(inactive.semantic_status_label(), "Finished");
    assert!(!inactive.semantic_state().active);
    assert_eq!(inactive.content(), "Still here");

    let reduced = ShimmerText::new("Still here", theme, MotionPolicy::Reduced);
    assert_eq!(reduced.motion_plan(), ShimmerMotionPlan::Immediate);
    assert_eq!(reduced.motion_plan().animation(), None);
    assert_eq!(reduced.semantic_state().label.as_ref(), "Still here");

    let full = ShimmerText::new("Still here", theme, MotionPolicy::Full);
    let animation = full
        .motion_plan()
        .animation()
        .expect("full active motion must animate");
    assert_eq!(animation.duration(), DEFAULT_DURATION);
    assert_eq!(animation.delay(), DEFAULT_DELAY);
    assert!(!animation.gpui_animation().oneshot);
}

struct ShimmerLayoutProbe {
    motion: MotionPolicy,
}

impl Render for ShimmerLayoutProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let text = div()
            .debug_selector(|| TEXT_SELECTOR.to_string())
            .child(ShimmerText::new(
                "Receiving response",
                ArtisanTheme::for_mode(ThemeMode::Dark),
                self.motion,
            ));

        div()
            .w(px(320.0))
            .h(px(80.0))
            .debug_selector(|| HOST_SELECTOR.to_string())
            .child(text)
    }
}

#[gpui::test]
fn reduced_motion_shimmer_has_real_nonempty_gpui_geometry(cx: &mut TestAppContext) {
    let (_, cx) = cx.add_window_view(|_, _| ShimmerLayoutProbe {
        motion: MotionPolicy::Reduced,
    });
    let host = cx
        .debug_bounds(HOST_SELECTOR)
        .expect("host must paint inspectable bounds");
    let text = cx
        .debug_bounds(TEXT_SELECTOR)
        .expect("text must paint inspectable bounds");

    assert_eq!(host.size.width, px(320.0));
    assert_eq!(host.size.height, px(80.0));
    assert!(text.size.width > px(0.0));
    assert!(text.size.height > px(0.0));
}

#[test]
fn builders_retain_fragment_inputs_with_empty_defaults() {
    use artisan_ui::selectable_text::TextRunOverride;
    use gpui::HighlightStyle;
    let theme = ArtisanTheme::for_mode(ThemeMode::Dark);
    let plain = ShimmerText::new("Working", theme, MotionPolicy::Full);
    assert!(plain.text_runs_value().is_none());
    let styled = ShimmerText::new("read Cargo", theme, MotionPolicy::Full).text_runs(
        "summary-runs",
        vec![(5..10, HighlightStyle::default())],
        vec![TextRunOverride {
            range: 5..10,
            font_family: Some("Mono".into()),
            letter_spacing: Some(gpui::px(0.0)),
        }],
    );
    let (id, highlights, overrides) = styled
        .text_runs_value()
        .expect("styled runs must be retained");
    assert_eq!(id.as_ref(), "summary-runs");
    assert_eq!(highlights, &[(5..10, HighlightStyle::default())]);
    assert_eq!(overrides.len(), 1);
    assert_eq!(overrides[0].range, 5..10);
    assert_eq!(
        overrides[0].font_family.as_ref().map(AsRef::as_ref),
        Some("Mono")
    );
    // Styled runs never change the motion decision: active Full still
    // schedules frames, so the sweep animates over compiled runs.
    assert!(styled.motion_plan().is_animating());
}

#[test]
fn gradient_matches_electron_stops_and_background_position() {
    use artisan_ui::shimmer_text::gradient_strength;
    // At 3/7 of the sweep, the 50%-wide image starts at x=50%.
    let phase = 3.0 / 7.0;
    for (position, expected) in [
        (0.5, 0.0),
        (0.6, 0.5),
        (0.7, 1.0),
        (0.8, 1.0),
        (0.9, 0.5),
        (1.0, 0.0),
    ] {
        assert!((gradient_strength(position, phase, 50.0) - expected).abs() < 0.00001);
    }
    assert_eq!(
        gradient_strength(0.5, 0.0, 50.0).to_bits(),
        0.0_f32.to_bits()
    );
    assert_eq!(
        gradient_strength(0.5, phase, 0.0).to_bits(),
        0.0_f32.to_bits()
    );
    assert_eq!(
        gradient_strength(f32::NAN, phase, 50.0).to_bits(),
        0.0_f32.to_bits()
    );
    // A fade changes within a single wide glyph, rather than at character boundaries.
    assert!(gradient_strength(0.61, phase, 50.0) > gradient_strength(0.60, phase, 50.0));
}

#[test]
fn gradient_mixes_the_actual_run_color_in_srgb() {
    use artisan_ui::shimmer_text::shimmer_color;
    use gpui::{Rgba, hsla_to_rgba, rgb_to_hsla};
    let base = rgb_to_hsla(Rgba::new(0.2, 0.4, 0.6, 0.7));
    for (mode, expected) in [
        (ThemeMode::Light, [0.72, 0.79, 0.86]),
        (ThemeMode::Dark, [0.09, 0.18, 0.27]),
    ] {
        let peak = hsla_to_rgba(shimmer_color(base, mode, 1.0));
        for (actual, expected) in [peak.red, peak.green, peak.blue].into_iter().zip(expected) {
            assert!((actual - expected).abs() < 0.00001);
        }
        assert!((peak.alpha - 0.7).abs() < 0.00001);
        assert_eq!(shimmer_color(base, mode, 0.0), base);
    }
}

#[gpui::test]
fn full_motion_gradient_keeps_the_measured_text_geometry(cx: &mut TestAppContext) {
    let (_, full) = cx.add_window_view(|_, _| ShimmerLayoutProbe {
        motion: MotionPolicy::Full,
    });
    let animated = full
        .debug_bounds(TEXT_SELECTOR)
        .expect("animated text paints");
    let (_, reduced) = cx.add_window_view(|_, _| ShimmerLayoutProbe {
        motion: MotionPolicy::Reduced,
    });
    let settled = reduced
        .debug_bounds(TEXT_SELECTOR)
        .expect("settled text paints");
    assert_eq!(animated.size, settled.size);
    assert!(animated.size.width > px(0.0));
}
