//! Behavioral coverage for the native GPUI Skeleton primitive.
//!
//! The paint and motion assertions compare against independently transcribed
//! legacy facts. GPUI bounds assertions cover layout only; they make no pixel,
//! platform-accessibility, or loading-state claim.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use artisan_ui::motion::MotionPolicy;
use artisan_ui::skeleton::{SkeletonAnimation, SkeletonMotionPlan, SkeletonStyle, skeleton};
use artisan_ui::theme::{ArtisanTheme, SurfaceStep, ThemeMode};
use gpui::{
    Bounds, Context, IntoElement, ParentElement, Pixels, Render, Styled, TestAppContext, Window,
    div, px,
};

const LEGACY_RADIUS_PX: f32 = 12.0;
const LEGACY_MIN_OPACITY: f32 = 0.5;
const LEGACY_MAX_OPACITY: f32 = 1.0;
const HOST_WIDTH_PX: f32 = 320.0;
const HOST_HEIGHT_PX: f32 = 80.0;

#[test]
fn style_resolves_the_exact_light_and_dark_muted_paint_and_radius() {
    let expected = [
        (ThemeMode::Light, SurfaceStep::S100),
        (ThemeMode::Dark, SurfaceStep::S800),
    ];

    for (mode, muted_step) in expected {
        let theme = ArtisanTheme::for_mode(mode);
        let style = SkeletonStyle::resolve(theme);

        assert_eq!(style.background, muted_step.oklch().to_paint());
        assert_eq!(style.background, theme.colors.muted.to_paint());
        assert_eq!(style.corner_radius, px(LEGACY_RADIUS_PX));
    }
}

#[test]
fn full_motion_uses_the_repeating_two_second_pulse_and_exact_key_samples() {
    let plan = SkeletonMotionPlan::for_policy(MotionPolicy::Full);
    let animation = plan.animation().expect("full motion must animate");

    assert_eq!(animation, SkeletonAnimation::new());
    assert_eq!(animation.duration(), Duration::from_secs(2));

    let clock = animation.gpui_animation();
    assert!(!clock.oneshot, "the legacy pulse repeats forever");
    for progress in [0.0_f32, 0.25, 0.5, 0.75, 1.0] {
        let clock_sample = (clock.easing)(progress);
        assert_eq!(clock_sample.to_bits(), progress.to_bits());
    }

    let expected = [
        LEGACY_MAX_OPACITY,
        0.75,
        LEGACY_MIN_OPACITY,
        0.75,
        LEGACY_MAX_OPACITY,
    ];
    for (progress, expected_opacity) in [0.0_f32, 0.25, 0.5, 0.75, 1.0].into_iter().zip(expected) {
        assert_eq!(
            animation.opacity_at(progress).to_bits(),
            expected_opacity.to_bits()
        );
    }

    for progress in [0.0_f32, 0.1, 0.25, 0.4, 0.5] {
        assert_eq!(
            animation.opacity_at(progress).to_bits(),
            animation.opacity_at(1.0 - progress).to_bits(),
            "the pulse is symmetric around its midpoint"
        );
    }

    for progress in [-1.0_f32, 0.0, 0.5, 1.0, 2.0, f32::NAN] {
        let opacity = animation.opacity_at(progress);
        assert!((LEGACY_MIN_OPACITY..=LEGACY_MAX_OPACITY).contains(&opacity));
    }
}

#[test]
fn reduced_motion_is_opaque_static_and_constructs_no_animation() {
    let plan = SkeletonMotionPlan::for_policy(MotionPolicy::Reduced);

    assert_eq!(plan, SkeletonMotionPlan::Immediate);
    assert_eq!(plan.animation(), None);
    for progress in [0.0_f32, 0.5, 1.0] {
        assert_eq!(
            plan.opacity_at(progress).to_bits(),
            LEGACY_MAX_OPACITY.to_bits()
        );
    }
}

type CapturedBounds = Rc<RefCell<Option<Bounds<Pixels>>>>;

struct SkeletonLayoutProbe {
    style: SkeletonStyle,
    captured: CapturedBounds,
    policy: MotionPolicy,
    no_dimensions: bool,
}

impl Render for SkeletonLayoutProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let captured = Rc::clone(&self.captured);
        let skeleton = skeleton(self.style, self.policy);
        let skeleton = if self.no_dimensions {
            skeleton
        } else {
            skeleton.w(px(64.0)).w(px(96.0)).h(px(16.0)).h(px(24.0))
        };

        div()
            .w(px(HOST_WIDTH_PX))
            .h(px(HOST_HEIGHT_PX))
            .flex()
            .flex_row()
            .items_start()
            .on_children_prepainted(move |children, _, _| {
                *captured.borrow_mut() = children.into_iter().next();
            })
            .child(skeleton)
    }
}

#[gpui::test]
fn caller_chained_width_and_height_values_win_and_layout_is_preserved(cx: &mut TestAppContext) {
    let captured = CapturedBounds::default();
    let (_, _cx) = cx.add_window_view(|_, _| SkeletonLayoutProbe {
        style: SkeletonStyle::resolve(ArtisanTheme::for_mode(ThemeMode::Dark)),
        captured: Rc::clone(&captured),
        policy: MotionPolicy::Full,
        no_dimensions: false,
    });

    let bounds = (*captured.borrow()).expect("the host must observe its Skeleton child bounds");
    assert_eq!(bounds.size.width, px(96.0));
    assert_eq!(bounds.size.height, px(24.0));
}

#[gpui::test]
fn skeleton_has_no_intrinsic_dimensions_without_caller_values(cx: &mut TestAppContext) {
    let captured = CapturedBounds::default();
    let (_, _cx) = cx.add_window_view(|_, _| SkeletonLayoutProbe {
        style: SkeletonStyle::resolve(ArtisanTheme::for_mode(ThemeMode::Light)),
        captured: Rc::clone(&captured),
        policy: MotionPolicy::Reduced,
        no_dimensions: true,
    });

    let bounds = (*captured.borrow()).expect("the host must observe its Skeleton child bounds");
    assert_eq!(bounds.size.width, px(0.0));
    assert_eq!(bounds.size.height, px(0.0));
}
