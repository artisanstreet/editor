//! Deterministic and in-memory GPUI coverage for the native image viewer.
//!
//! The tests exercise the production state/style/geometry APIs directly and
//! mount the real [`ImageViewerView`] for painted bounds and dismissal. They
//! do not claim image decoding or platform accessibility: the pinned GPUI
//! harness intentionally has neither as a contract.

use std::time::Duration;

use artisan_frontend::image_viewer::{
    BackdropBlurIntent, IMAGE_VIEWER_BACKDROP_SELECTOR, IMAGE_VIEWER_CLOSE_SELECTOR,
    IMAGE_VIEWER_CLOSE_TRANSITION, IMAGE_VIEWER_CONTENT_SELECTOR, IMAGE_VIEWER_DISMISS_SELECTOR,
    IMAGE_VIEWER_IMAGE_ALT, IMAGE_VIEWER_ROOT_SELECTOR, IMAGE_VIEWER_TITLE, ImageViewerGeometry,
    ImageViewerInspectionAction, ImageViewerState, ImageViewerStyle, ImageViewerView,
};
use artisan_ui::motion::{MotionAnimation, MotionPlan, MotionPolicy};
use artisan_ui::theme::{ArtisanTheme, SurfaceStep, ThemeMode};
use gpui::{
    Entity, ImageSource, Modifiers, Pixels, SharedString, TestAppContext, VisualTestContext, point,
    px, size,
};

#[test]
fn controlled_lifecycle_emits_one_retain_and_release_per_open_lease() {
    let mut state = ImageViewerState::new(
        Some(ImageSource::from("file:///fixture.png")),
        Some(SharedString::from("Fixture")),
        false,
    );

    assert!(!state.is_open());
    assert!(!state.inspection_lease_held());
    assert!(state.take_inspection_actions().is_empty());

    assert!(state.set_open(true));
    assert!(state.inspection_lease_held());
    assert_eq!(
        state.take_inspection_actions(),
        vec![ImageViewerInspectionAction::Retain]
    );
    let generation = state.generation();

    assert!(!state.set_open(true));
    assert_eq!(state.generation(), generation);
    assert!(state.take_inspection_actions().is_empty());

    assert!(state.dismiss());
    assert!(!state.inspection_lease_held());
    assert_eq!(
        state.take_inspection_actions(),
        vec![ImageViewerInspectionAction::Release]
    );

    assert!(state.set_open(true));
    assert_eq!(
        state.take_inspection_actions(),
        vec![ImageViewerInspectionAction::Retain]
    );
    assert!(state.release_on_unmount());
    assert_eq!(
        state.take_inspection_actions(),
        vec![ImageViewerInspectionAction::Release]
    );
    assert!(!state.release_on_unmount());
    assert!(state.take_inspection_actions().is_empty());
}

#[test]
fn initial_open_and_escape_use_the_same_controlled_transition() {
    let mut state = ImageViewerState::new(None, None, true);

    assert!(state.is_open());
    assert_eq!(
        state.take_inspection_actions(),
        vec![ImageViewerInspectionAction::Retain]
    );
    assert!(state.handle_escape());
    assert!(!state.is_open());
    assert_eq!(
        state.take_inspection_actions(),
        vec![ImageViewerInspectionAction::Release]
    );
    assert!(!state.handle_escape());
}

#[test]
fn semantic_metadata_has_named_and_fallback_variants() {
    let fallback = ImageViewerState::new(None, None, false).semantics();
    assert_eq!(fallback.title, IMAGE_VIEWER_TITLE);
    assert_eq!(fallback.image_alt, IMAGE_VIEWER_IMAGE_ALT);
    assert_eq!(fallback.content_label, IMAGE_VIEWER_TITLE);
    assert_eq!(fallback.close_label, "Close image preview");

    let named = ImageViewerState::new(None, Some("Sunset".into()), false).semantics();
    assert_eq!(named.title, "Sunset");
    assert_eq!(named.image_alt, "Sunset");
    assert_eq!(named.content_label, "Image preview: Sunset");

    let whitespace = ImageViewerState::new(None, Some("  ".into()), false).semantics();
    assert_eq!(whitespace.title, IMAGE_VIEWER_TITLE);
    assert_eq!(whitespace.image_alt, IMAGE_VIEWER_IMAGE_ALT);
}

#[test]
fn style_keeps_theme_tokens_layer_intent_and_reduced_motion_explicit() {
    for mode in [ThemeMode::Light, ThemeMode::Dark] {
        let theme = ArtisanTheme::for_mode(mode);
        let full = ImageViewerStyle::resolve(theme, px(40.0), MotionPolicy::Full);

        assert!((full.backdrop_alpha - 0.70).abs() < f32::EPSILON);
        assert_eq!(
            full.backdrop_color,
            theme
                .surfaces
                .value(SurfaceStep::S1000)
                .with_alpha(0.70)
                .to_paint()
        );
        assert_eq!(full.backdrop_blur, BackdropBlurIntent::Medium);
        assert_eq!(full.overlay_z_index, 50);
        assert_eq!(full.content_z_index, 51);
        assert_eq!(full.content_padding, px(32.0));
        assert_eq!(full.titlebar_offset, px(40.0));
        assert_eq!(full.close_size, theme.density.control_sm);
        assert_eq!(full.close_inset, px(8.0));
        assert_eq!(IMAGE_VIEWER_CLOSE_TRANSITION, Duration::from_millis(150));
        assert_eq!(
            full.close_motion.animation().map(MotionAnimation::duration),
            Some(Duration::from_millis(150))
        );

        let reduced = ImageViewerStyle::resolve(theme, px(40.0), MotionPolicy::Reduced);
        assert_eq!(reduced.close_motion, MotionPlan::Immediate);
    }
}

fn assert_close(actual: Pixels, expected: f32) {
    assert!(
        (f32::from(actual) - expected).abs() < 0.01,
        "expected {expected}px, got {actual:?}"
    );
}

#[test]
fn geometry_clamps_titlebar_fits_without_upscaling_and_centers() {
    let geometry = ImageViewerGeometry::new(size(px(1200.0), px(800.0)), px(40.0), px(32.0));
    let content = geometry.content_bounds();

    assert_eq!(content.origin, point(px(0.0), px(40.0)));
    assert_eq!(content.size, size(px(1200.0), px(760.0)));
    assert_eq!(geometry.available_image_size(), size(px(1136.0), px(696.0)));

    let large = geometry.image_bounds(size(px(1600.0), px(900.0)));
    assert_close(large.size.width, 1136.0);
    assert_close(large.size.height, 639.0);
    assert_close(large.origin.x, 32.0);
    assert_close(large.origin.y, 100.5);

    let small = geometry.image_bounds(size(px(200.0), px(100.0)));
    assert_eq!(small.size, size(px(200.0), px(100.0)));
    assert_eq!(small.origin.x, px(500.0));
    assert_eq!(small.origin.y, px(370.0));

    let clamped = ImageViewerGeometry::new(size(px(200.0), px(100.0)), px(140.0), px(32.0));
    let bounds = clamped.image_bounds(size(px(400.0), px(200.0)));
    assert!(bounds.origin.y >= px(100.0));
    assert!(bounds.bottom() <= px(100.0) + px(0.001));

    let invalid = geometry.image_bounds(size(px(-1.0), px(0.0)));
    assert_eq!(invalid.size, size(px(0.0), px(0.0)));
}

fn open_viewer(cx: &mut TestAppContext) -> (Entity<ImageViewerView>, &mut VisualTestContext) {
    let (viewer, cx) = cx.add_window_view(|window, viewer_cx| {
        ImageViewerView::new(
            window,
            viewer_cx,
            None,
            Some("Rendered fixture".into()),
            ArtisanTheme::for_mode(ThemeMode::Dark),
            true,
        )
        .with_titlebar_offset(px(40.0))
    });
    cx.simulate_resize(size(px(800.0), px(600.0)));
    cx.run_until_parked();
    (viewer, cx)
}

fn bounds(cx: &mut VisualTestContext, selector: &'static str) -> gpui::Bounds<gpui::Pixels> {
    cx.debug_bounds(selector)
        .unwrap_or_else(|| panic!("selector `{selector}` must paint bounds"))
}

#[gpui::test]
fn real_gpui_render_fills_viewport_below_titlebar_and_owns_dismissal(cx: &mut TestAppContext) {
    let (viewer, cx) = open_viewer(cx);
    let root = bounds(cx, IMAGE_VIEWER_ROOT_SELECTOR);
    let backdrop = bounds(cx, IMAGE_VIEWER_BACKDROP_SELECTOR);
    let content = bounds(cx, IMAGE_VIEWER_CONTENT_SELECTOR);
    let dismiss = bounds(cx, IMAGE_VIEWER_DISMISS_SELECTOR);
    let close = bounds(cx, IMAGE_VIEWER_CLOSE_SELECTOR);

    assert_eq!(root.size, size(px(800.0), px(600.0)));
    assert_eq!(backdrop, root);
    assert_eq!(content.origin.y, px(40.0));
    assert_eq!(content.size, size(px(800.0), px(560.0)));
    assert_eq!(dismiss, content);
    assert_eq!(close.size, size(px(32.0), px(32.0)));
    assert!(close.origin.x >= content.origin.x);
    assert!(close.origin.y >= content.origin.y);

    cx.simulate_click(point(px(12.0), px(100.0)), Modifiers::none());
    cx.run_until_parked();
    cx.update(|_, app| {
        assert!(!viewer.read(app).is_open());
    });
}

#[gpui::test]
fn real_gpui_close_control_closes_viewer(cx: &mut TestAppContext) {
    let (viewer, cx) = open_viewer(cx);
    let close = bounds(cx, IMAGE_VIEWER_CLOSE_SELECTOR);

    cx.simulate_click(close.center(), Modifiers::none());
    cx.run_until_parked();
    cx.update(|_, app| {
        assert!(!viewer.read(app).is_open());
    });
}

#[gpui::test]
fn real_gpui_escape_closes_and_queues_release_for_the_caller(cx: &mut TestAppContext) {
    let (viewer, cx) = open_viewer(cx);

    cx.update(|window, app| {
        let view = viewer.read(app);
        assert!(view.focus_handle().is_focused(window));
        assert_eq!(
            view.state().pending_inspection_actions(),
            &[ImageViewerInspectionAction::Retain]
        );
    });

    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    cx.update(|_, app| {
        let view = viewer.read(app);
        assert!(!view.is_open());
        assert_eq!(
            view.state().pending_inspection_actions(),
            &[
                ImageViewerInspectionAction::Retain,
                ImageViewerInspectionAction::Release
            ]
        );
    });
}
