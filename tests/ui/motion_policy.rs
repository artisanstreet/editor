use std::time::Duration;

use artisan_ui::motion::{MotionCurve, MotionDuration, MotionPlan, MotionPolicy, MotionRecipe};

const ANCHOR_INPUTS: [f64; 5] = [0.0, 0.25, 0.5, 0.75, 1.0];

fn assert_close(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() <= 1.0e-6,
        "expected {expected:.9}, got {actual:.9}"
    );
}

fn full_animation(recipe: MotionRecipe) -> artisan_ui::motion::MotionAnimation {
    MotionPolicy::Full
        .resolve(recipe)
        .animation()
        .expect("full motion must resolve to an animation")
}

#[test]
fn duration_tokens_keep_the_exact_reachable_values() {
    assert_eq!(
        MotionDuration::Quick.as_duration(),
        Duration::from_millis(150)
    );
    assert_eq!(
        MotionDuration::Fast.as_duration(),
        Duration::from_millis(250)
    );
    assert_eq!(
        MotionDuration::Medium.as_duration(),
        Duration::from_millis(350)
    );
}

#[test]
fn recipes_keep_exact_duration_delay_and_curve_assignments() {
    let expected = [
        (MotionRecipe::MenuOpen, 250, 0, MotionCurve::SmoothOut),
        (MotionRecipe::MenuClose, 150, 0, MotionCurve::SmoothOut),
        (MotionRecipe::TooltipIn, 150, 80, MotionCurve::EaseOut),
        (MotionRecipe::TooltipOut, 50, 0, MotionCurve::EaseOut),
        (MotionRecipe::TextSwap, 150, 0, MotionCurve::EaseInOut),
        (MotionRecipe::StreamWord, 320, 0, MotionCurve::SmoothOut),
        (
            MotionRecipe::AccordionExpand,
            250,
            0,
            MotionCurve::SmoothOut,
        ),
        (
            MotionRecipe::AccordionCollapse,
            250,
            0,
            MotionCurve::SmoothOut,
        ),
        (
            MotionRecipe::AccordionChevron,
            250,
            0,
            MotionCurve::SmoothOut,
        ),
        (MotionRecipe::IconSwap, 250, 0, MotionCurve::EaseInOut),
        (MotionRecipe::Success, 500, 0, MotionCurve::SmoothOut),
        (MotionRecipe::CheckPath, 500, 80, MotionCurve::SmoothOut),
        (MotionRecipe::CheckBob, 500, 0, MotionCurve::CheckBob),
    ];

    assert_eq!(MotionRecipe::ALL, expected.map(|(recipe, _, _, _)| recipe));
    for (recipe, duration_ms, delay_ms, curve) in expected {
        let animation = full_animation(recipe);
        assert_eq!(animation.duration(), Duration::from_millis(duration_ms));
        assert_eq!(animation.delay(), Duration::from_millis(delay_ms));
        assert_eq!(animation.curve(), curve);
        assert!(!animation.duration().is_zero());
    }
}

#[test]
fn reduced_motion_reveals_every_recipe_immediately() {
    for recipe in MotionRecipe::ALL {
        assert_eq!(MotionPolicy::Reduced.resolve(recipe), MotionPlan::Immediate);
        assert_eq!(MotionPolicy::Reduced.resolve(recipe).animation(), None);
    }

    assert_eq!(
        MotionPolicy::Reduced.resolve(MotionRecipe::StreamWord),
        MotionPlan::Immediate,
        "stream-word reveal is bypassed rather than shortened"
    );
}

#[test]
fn full_motion_animates_every_recipe_with_a_positive_duration() {
    for recipe in MotionRecipe::ALL {
        let plan = MotionPolicy::Full.resolve(recipe);
        assert!(matches!(plan, MotionPlan::Animate(_)));
        assert!(
            !plan
                .animation()
                .expect("animated plan")
                .duration()
                .is_zero()
        );
    }
}

#[test]
fn curves_match_the_frozen_css_bezier_samples() {
    let expected = [
        (
            MotionCurve::SmoothOut,
            [0.0, 0.764_864_7, 0.961_382_57, 0.996_894_24, 1.0],
        ),
        (
            MotionCurve::EaseOut,
            [0.0, 0.378_138_12, 0.684_643_2, 0.906_535_3, 1.0],
        ),
        (
            MotionCurve::EaseInOut,
            [0.0, 0.129_161_92, 0.5, 0.870_838_05, 1.0],
        ),
        (
            MotionCurve::CheckBob,
            [0.0, 0.727_583, 1.009_880_3, 1.032_109, 1.0],
        ),
    ];

    for (curve, samples) in expected {
        for (input, sample) in ANCHOR_INPUTS.into_iter().zip(samples) {
            assert_close(curve.sample(input), sample);
        }
    }
}

#[test]
fn only_check_bob_overshoots_the_visual_interval() {
    let bounded = [
        MotionCurve::SmoothOut,
        MotionCurve::EaseOut,
        MotionCurve::EaseInOut,
    ];

    for curve in bounded {
        let mut previous = 0.0;
        for step in 0..=1_000_u16 {
            let sample = curve.sample(f64::from(step) / 1_000.0);
            assert!((0.0..=1.0).contains(&sample));
            assert!(sample >= previous);
            previous = sample;
        }
    }

    let maximum = (0..=10_000_u16)
        .map(|step| MotionCurve::CheckBob.sample(f64::from(step) / 10_000.0))
        .fold(0.0_f64, f64::max);
    assert_close(MotionCurve::CheckBob.sample(0.0), 0.0);
    assert_close(MotionCurve::CheckBob.sample(1.0), 1.0);
    assert!(maximum > 1.04);
    assert!((maximum - 1.040_809).abs() < 1.0e-5);
}

#[test]
fn gpui_clock_remains_linear_even_for_the_overshooting_recipe() {
    let animation = full_animation(MotionRecipe::CheckBob);
    assert!(animation.curve().sample(0.5) > 1.0);

    let clock = animation.gpui_clock();
    assert_eq!(clock.duration, Duration::from_millis(500));
    assert!(clock.oneshot);
    for progress in [0.0_f32, 0.25, 0.5, 0.75, 1.0] {
        assert_close(f64::from((clock.easing)(progress)), f64::from(progress));
        assert!((0.0..=1.0).contains(&(clock.easing)(progress)));
    }
}
