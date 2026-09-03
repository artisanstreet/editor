//! Deterministic and GPUI render coverage for the native `LinkPreview` packet.

use std::time::Duration;

use artisan_ui::link_preview::{
    LINK_PREVIEW_CLOSE_DELAY, LINK_PREVIEW_CONTENT_SELECTOR, LINK_PREVIEW_DESCRIPTION_ID,
    LINK_PREVIEW_EXIT_DURATION, LINK_PREVIEW_OPEN_DELAY, LINK_PREVIEW_ROOT_SELECTOR,
    LINK_PREVIEW_SIDE_OFFSET_PX, LINK_PREVIEW_TRIGGER_SELECTOR, LINK_PREVIEW_WIDTH_PX, LinkPreview,
    LinkPreviewAlign, LinkPreviewContentMetadata, LinkPreviewEffectPlan, LinkPreviewEvent,
    LinkPreviewMotionPlan, LinkPreviewPhase, LinkPreviewPlacement, LinkPreviewPointerTransit,
    LinkPreviewSemanticRole, LinkPreviewSide, LinkPreviewState, LinkPreviewStyle,
    LinkPreviewTransformOrigin,
};
use artisan_ui::motion::{MotionPlan, MotionPolicy, MotionRecipe};
use artisan_ui::theme::{ArtisanTheme, RadiusStep, RadiusTokens, ThemeMode};
use gpui::{
    Bounds, Context, InteractiveElement as _, IntoElement, ParentElement, Pixels, Render,
    SharedString, Styled, TestAppContext, Window, div, point, px, size, transparent_black,
};

const CUSTOM_ROOT_SELECTOR: &str = "link-preview-test";
const CALLER_MATERIAL_SELECTOR: &str = "link-preview-caller-material";

fn bounds(x: f32, y: f32, width: f32, height: f32) -> Bounds<Pixels> {
    Bounds::new(point(px(x), px(y)), size(px(width), px(height)))
}

#[test]
fn timing_contract_is_zero_open_120_close_and_50_exit() {
    assert_eq!(LINK_PREVIEW_OPEN_DELAY, Duration::ZERO);
    assert_eq!(LINK_PREVIEW_CLOSE_DELAY, Duration::from_millis(120));
    assert_eq!(LINK_PREVIEW_EXIT_DURATION, Duration::from_millis(50));

    let opening =
        LinkPreviewState::new(false).transition(LinkPreviewEvent::TriggerEnter, Duration::ZERO);
    assert_eq!(opening.phase(), LinkPreviewPhase::Opening);
    assert_eq!(opening.open_deadline(), Some(Duration::ZERO));

    let open = opening.advance(Duration::ZERO);
    assert_eq!(open.phase(), LinkPreviewPhase::Open);

    let closing = open.transition(LinkPreviewEvent::TriggerLeave, Duration::ZERO);
    assert_eq!(closing.close_deadline(), Some(Duration::from_millis(120)));
    assert_eq!(
        closing.advance(Duration::from_millis(119)).phase(),
        LinkPreviewPhase::Open
    );

    let exit = closing.advance(Duration::from_millis(120));
    assert_eq!(exit.phase(), LinkPreviewPhase::Closing);
    assert_eq!(
        exit.finish_close_deadline(),
        Some(Duration::from_millis(170))
    );
    assert_eq!(
        exit.advance(Duration::from_millis(169)).phase(),
        LinkPreviewPhase::Closing
    );
    assert_eq!(
        exit.advance(Duration::from_millis(170)).phase(),
        LinkPreviewPhase::Closed
    );
}

#[test]
fn reduced_motion_settles_the_same_clock_transitions_without_short_animation() {
    let opening = LinkPreviewState::new(false)
        .transition(LinkPreviewEvent::TriggerEnter, Duration::ZERO)
        .advance_with_policy(Duration::ZERO, MotionPolicy::Reduced);
    assert_eq!(opening.phase(), LinkPreviewPhase::Open);

    let closing = opening
        .transition(LinkPreviewEvent::TriggerLeave, Duration::ZERO)
        .advance_with_policy(Duration::from_millis(120), MotionPolicy::Reduced);
    assert_eq!(closing.phase(), LinkPreviewPhase::Closed);
    assert!(closing.is_settled());
}

#[test]
fn hoverable_content_grace_selection_and_explicit_close_behaviors_are_stable() {
    let trigger = bounds(100.0, 160.0, 80.0, 24.0);
    let content = bounds(100.0, 88.0, 200.0, 64.0);

    let open = LinkPreviewState::new(false)
        .transition(LinkPreviewEvent::TriggerEnter, Duration::ZERO)
        .advance(Duration::ZERO);

    let scheduled = open.transition(LinkPreviewEvent::TriggerLeave, Duration::ZERO);
    assert_eq!(scheduled.close_deadline(), Some(LINK_PREVIEW_CLOSE_DELAY));

    let grace = scheduled.transition(
        LinkPreviewEvent::PointerMoved {
            pointer: point(px(150.0), px(156.0)),
            trigger_bounds: trigger,
            content_bounds: content,
        },
        Duration::from_millis(10),
    );
    assert_eq!(grace.phase(), LinkPreviewPhase::Open);
    assert_eq!(grace.close_deadline(), None);

    let inside = grace.transition(LinkPreviewEvent::ContentEnter, Duration::from_millis(10));
    let left = inside.transition(LinkPreviewEvent::ContentLeave, Duration::from_millis(20));
    assert_eq!(left.close_deadline(), Some(Duration::from_millis(140)));

    let held = left.transition(
        LinkPreviewEvent::ContentPointerDown,
        Duration::from_millis(30),
    );
    assert!(held.pointer_down_on_content());
    assert_eq!(held.close_deadline(), None);

    let selected = held.transition(
        LinkPreviewEvent::SelectionChanged {
            contains_selection: true,
        },
        Duration::from_millis(30),
    );
    assert!(selected.contains_selection());

    let released = selected.transition(
        LinkPreviewEvent::ContentPointerUp,
        Duration::from_millis(40),
    );
    assert!(!released.pointer_down_on_content());
    assert_eq!(released.close_deadline(), None);

    let clear_selection = released.transition(
        LinkPreviewEvent::SelectionChanged {
            contains_selection: false,
        },
        Duration::from_millis(40),
    );
    assert_eq!(
        clear_selection.close_deadline(),
        Some(Duration::from_millis(160))
    );

    let escape = open.transition(LinkPreviewEvent::Escape, Duration::ZERO);
    assert_eq!(escape.close_deadline(), Some(Duration::from_millis(120)));
    assert_eq!(
        escape.advance(Duration::from_millis(120)).phase(),
        LinkPreviewPhase::Closing
    );
}

#[test]
fn focus_only_opens_for_focus_visible_and_keeps_the_card_alive() {
    let not_visible = LinkPreviewState::new(false).transition(
        LinkPreviewEvent::TriggerFocus {
            focus_visible: false,
        },
        Duration::ZERO,
    );
    assert_eq!(not_visible.phase(), LinkPreviewPhase::Closed);

    let focused = not_visible
        .transition(
            LinkPreviewEvent::TriggerFocus {
                focus_visible: true,
            },
            Duration::ZERO,
        )
        .advance(Duration::ZERO);
    assert_eq!(focused.phase(), LinkPreviewPhase::Open);
    assert!(focused.trigger_focused());

    let trigger_left = focused.transition(LinkPreviewEvent::TriggerLeave, Duration::ZERO);
    assert_eq!(trigger_left.close_deadline(), None);

    let blurred = trigger_left.transition(LinkPreviewEvent::TriggerBlur, Duration::ZERO);
    assert_eq!(blurred.close_deadline(), Some(Duration::from_millis(120)));
}

#[test]
fn safe_polygon_classifies_trigger_content_grace_and_outside() {
    let trigger = bounds(100.0, 160.0, 80.0, 24.0);
    let content = bounds(100.0, 88.0, 200.0, 64.0);

    assert_eq!(
        artisan_ui::link_preview::pointer_transit(trigger, content, point(px(120.0), px(170.0)),),
        LinkPreviewPointerTransit::Trigger
    );
    assert_eq!(
        artisan_ui::link_preview::pointer_transit(trigger, content, point(px(180.0), px(110.0)),),
        LinkPreviewPointerTransit::Content
    );
    assert_eq!(
        artisan_ui::link_preview::pointer_transit(trigger, content, point(px(150.0), px(156.0)),),
        LinkPreviewPointerTransit::Grace
    );
    assert_eq!(
        artisan_ui::link_preview::pointer_transit(trigger, content, point(px(20.0), px(20.0)),),
        LinkPreviewPointerTransit::Outside
    );
    assert!(LinkPreviewPointerTransit::Grace.keeps_open());
    assert!(!LinkPreviewPointerTransit::Outside.keeps_open());
}

#[test]
fn placement_prefers_top_start_flips_and_clamps_deterministically() {
    let viewport = size(px(1024.0), px(768.0));
    let content_size = size(px(200.0), px(64.0));

    let top =
        LinkPreviewPlacement::resolve(bounds(100.0, 160.0, 80.0, 24.0), content_size, viewport);
    assert_eq!(top.origin, point(px(100.0), px(88.0)));
    assert_eq!(top.side, LinkPreviewSide::Top);
    assert_eq!(top.align, LinkPreviewAlign::Start);
    assert_eq!(top.side_offset, px(LINK_PREVIEW_SIDE_OFFSET_PX));
    assert_eq!(
        top.transform_origin,
        LinkPreviewTransformOrigin::BottomStart
    );
    assert!(!top.was_collision_adjusted());

    let flipped =
        LinkPreviewPlacement::resolve(bounds(100.0, 16.0, 80.0, 24.0), content_size, viewport);
    assert_eq!(flipped.origin, point(px(100.0), px(48.0)));
    assert_eq!(flipped.side, LinkPreviewSide::Bottom);
    assert_eq!(
        flipped.transform_origin,
        LinkPreviewTransformOrigin::TopStart
    );
    assert!(flipped.was_collision_adjusted());

    let clamped =
        LinkPreviewPlacement::resolve(bounds(900.0, 160.0, 80.0, 24.0), content_size, viewport);
    assert_eq!(clamped.origin, point(px(824.0), px(88.0)));
    assert!(clamped.was_collision_adjusted());
}

#[test]
fn theme_resolution_preserves_transparent_chrome_and_reached_geometry() {
    for mode in [ThemeMode::Light, ThemeMode::Dark] {
        let theme = ArtisanTheme::for_mode(mode);
        let style = LinkPreviewStyle::resolve(theme);

        assert_eq!(style.width, px(LINK_PREVIEW_WIDTH_PX));
        assert_eq!(style.max_width, px(320.0));
        assert_eq!(style.corner_radius, RadiusTokens::value(RadiusStep::X2l));
        assert_eq!(style.side_offset, px(8.0));
        assert_eq!(style.background, transparent_black());
        assert_eq!(style.foreground, theme.colors.foreground.to_paint());
    }
}

#[test]
fn motion_plan_uses_shared_tooltip_recipes_and_declares_unsupported_transform() {
    let opening = LinkPreviewMotionPlan::for_phase(LinkPreviewPhase::Opening, MotionPolicy::Full);
    let opening_animation = opening.animation().expect("opening must animate");
    assert_eq!(opening.recipe(), Some(MotionRecipe::TooltipIn));
    assert_eq!(opening.motion, MotionPlan::Animate(opening_animation));
    assert_eq!(opening_animation.duration(), Duration::from_millis(150));
    assert_eq!(opening_animation.delay(), Duration::from_millis(80));
    assert_eq!(opening.opacity, LinkPreviewEffectPlan::Animated);
    assert_eq!(opening.transform, LinkPreviewEffectPlan::UnsupportedByGpui);
    assert!(opening.content_present());

    let closing = LinkPreviewMotionPlan::for_phase(LinkPreviewPhase::Closing, MotionPolicy::Full);
    assert_eq!(closing.recipe(), Some(MotionRecipe::TooltipOut));
    assert_eq!(
        closing
            .animation()
            .expect("closing must animate")
            .duration(),
        Duration::from_millis(50)
    );
    assert!(closing.content_present());

    let reduced_open =
        LinkPreviewMotionPlan::for_phase(LinkPreviewPhase::Opening, MotionPolicy::Reduced);
    assert_eq!(reduced_open.motion, MotionPlan::Immediate);
    assert_eq!(reduced_open.effective_phase(), LinkPreviewPhase::Open);
    assert_eq!(reduced_open.opacity, LinkPreviewEffectPlan::Immediate);

    let reduced_close =
        LinkPreviewMotionPlan::for_phase(LinkPreviewPhase::Closing, MotionPolicy::Reduced);
    assert_eq!(reduced_close.effective_phase(), LinkPreviewPhase::Closed);
    assert!(!reduced_close.content_present());
}

#[test]
fn semantic_metadata_keeps_trigger_content_and_description_intent_explicit() {
    let placement = LinkPreviewPlacement::resolve(
        bounds(100.0, 160.0, 80.0, 24.0),
        size(px(200.0), px(64.0)),
        size(px(1024.0), px(768.0)),
    );

    let preview = LinkPreview::new(
        "context-usage",
        div(),
        div(),
        LinkPreviewStyle::resolve(ArtisanTheme::for_mode(ThemeMode::Dark)),
        LinkPreviewState::new(false),
    )
    .description(div().child("Context usage description"))
    .trigger_label("Context window 42% full")
    .content_id("context-usage-card")
    .placement(placement);

    let trigger = preview.trigger_metadata();
    assert_eq!(trigger.role, LinkPreviewSemanticRole::Button);
    assert_eq!(trigger.popup, LinkPreviewSemanticRole::Dialog);
    assert!(!trigger.expanded);
    assert_eq!(trigger.controls, SharedString::from("context-usage-card"));
    assert_eq!(
        trigger.label,
        Some(SharedString::from("Context window 42% full"))
    );
    assert_eq!(
        trigger.described_by,
        SharedString::from(LINK_PREVIEW_DESCRIPTION_ID)
    );
    assert!(trigger.description_always_mounted);

    let content: LinkPreviewContentMetadata = preview.content_metadata();
    assert_eq!(content.role, LinkPreviewSemanticRole::Dialog);
    assert_eq!(content.tab_index, -1);
    assert!(!content.focusable);
    assert!(content.focusout_prevented);
    assert!(content.auto_focus_prevented);
    assert_eq!(
        content.transform_origin,
        Some(LinkPreviewTransformOrigin::BottomStart)
    );

    let opened = LinkPreview::new(
        "context-usage",
        div(),
        div(),
        LinkPreviewStyle::resolve(ArtisanTheme::for_mode(ThemeMode::Dark)),
        LinkPreviewState::new(true),
    );
    assert!(opened.trigger_metadata().expanded);
}

struct LinkPreviewGeometryProbe {
    state: LinkPreviewState,
    style: LinkPreviewStyle,
    placement: LinkPreviewPlacement,
}

impl Render for LinkPreviewGeometryProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let preview = LinkPreview::new(
            "link-preview-test",
            div().w(px(24.0)).h(px(24.0)),
            div()
                .w_full()
                .h(px(40.0))
                .debug_selector(|| CALLER_MATERIAL_SELECTOR.to_string()),
            self.style,
            self.state,
        )
        .description(div().child("always mounted description"))
        .trigger_label("Context window 42% full")
        .debug_selector(CUSTOM_ROOT_SELECTOR)
        .placement(self.placement)
        .motion_policy(MotionPolicy::Reduced);

        div().w(px(640.0)).h(px(300.0)).child(preview)
    }
}

#[gpui::test]
fn real_gpui_render_mounts_description_trigger_and_anchored_material(cx: &mut TestAppContext) {
    let placement = LinkPreviewPlacement::resolve(
        bounds(100.0, 160.0, 80.0, 24.0),
        size(px(288.0), px(40.0)),
        size(px(1024.0), px(768.0)),
    );

    let (_view, cx) = cx.add_window_view(|_, _| LinkPreviewGeometryProbe {
        state: LinkPreviewState::new(true),
        style: LinkPreviewStyle::resolve(ArtisanTheme::for_mode(ThemeMode::Dark)),
        placement,
    });

    let root = cx
        .debug_bounds(CUSTOM_ROOT_SELECTOR)
        .expect("custom root selector must be present");
    let trigger = cx
        .debug_bounds("link-preview-test-trigger")
        .expect("trigger selector must be present");
    let content = cx
        .debug_bounds("link-preview-test-content")
        .expect("content selector must be present");
    let material = cx
        .debug_bounds(CALLER_MATERIAL_SELECTOR)
        .expect("caller material must be mounted");

    assert!(root.size.width >= px(24.0));
    assert_eq!(trigger.size, size(px(24.0), px(24.0)));
    assert_eq!(content.size.width, px(288.0));
    assert_eq!(content.size.height, px(40.0));
    assert_eq!(material.size.width, px(288.0));
    assert_eq!(material.size.height, px(40.0));
    assert_eq!(content.origin, placement.origin);
}

#[gpui::test]
fn closed_real_gpui_render_keeps_trigger_and_description_but_omits_content(
    cx: &mut TestAppContext,
) {
    let placement = LinkPreviewPlacement::resolve(
        bounds(100.0, 160.0, 80.0, 24.0),
        size(px(288.0), px(40.0)),
        size(px(1024.0), px(768.0)),
    );

    let (_view, cx) = cx.add_window_view(|_, _| LinkPreviewGeometryProbe {
        state: LinkPreviewState::new(false),
        style: LinkPreviewStyle::resolve(ArtisanTheme::for_mode(ThemeMode::Light)),
        placement,
    });

    assert!(cx.debug_bounds(CUSTOM_ROOT_SELECTOR).is_some());
    assert!(cx.debug_bounds(LINK_PREVIEW_TRIGGER_SELECTOR).is_none());
    assert!(cx.debug_bounds(LINK_PREVIEW_CONTENT_SELECTOR).is_none());
    assert!(cx.debug_bounds(CALLER_MATERIAL_SELECTOR).is_none());
    assert!(cx.debug_bounds(LINK_PREVIEW_ROOT_SELECTOR).is_none());
}
