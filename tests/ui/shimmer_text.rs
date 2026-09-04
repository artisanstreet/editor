//! Behavioral coverage for the native GPUI `ShimmerText` primitive.
//!
//! The deterministic tests cover the legacy timing/color contract and the
//! native segmented-glyph fallback. The GPUI test covers real layout bounds;
//! it intentionally makes no screenshot, platform-accessibility, or gradient
//! fidelity claim.

use std::time::Duration;

use artisan_ui::motion::MotionPolicy;
use artisan_ui::shimmer_text::{
    DEFAULT_DELAY, DEFAULT_DURATION, DEFAULT_SPREAD, ShimmerMotionPlan, ShimmerSegmentStyle,
    ShimmerText, ShimmerTextStyle, ShimmerTextVariant, ShimmerTiming, highlighted_ranges, phase_at,
    segments_for,
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
            assert_eq!(style.highlight, theme.colors.highlight.to_paint());
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
fn timing_and_segments_bound_invalid_spread_and_keep_utf8_ranges_valid() {
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

    let segments = segments_for("Åbc", 0.5, 50.0);
    assert_eq!(segments.len(), 3);
    assert_eq!(segments[0].range, 0..2);
    assert_eq!(segments[1].range, 2..3);
    assert_eq!(segments[2].range, 3..4);
    assert_eq!(segments[0].position.to_bits(), 0.0_f32.to_bits());
    assert_eq!(segments[2].position.to_bits(), 1.0_f32.to_bits());
    assert!(
        segments
            .iter()
            .any(|segment| { segment.style == ShimmerSegmentStyle::Highlight })
    );
    assert!(highlighted_ranges("", 0.5, 50.0).is_empty());
    assert!(
        segments_for("hello", 0.5, 0.0)
            .iter()
            .all(|segment| segment.style == ShimmerSegmentStyle::Base)
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

struct ShimmerLayoutProbe;

impl Render for ShimmerLayoutProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let text = div()
            .debug_selector(|| TEXT_SELECTOR.to_string())
            .child(ShimmerText::new(
                "Receiving response",
                ArtisanTheme::for_mode(ThemeMode::Dark),
                MotionPolicy::Reduced,
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
    let (_, cx) = cx.add_window_view(|_, _| ShimmerLayoutProbe);
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
