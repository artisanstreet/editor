//! Behavioral coverage for the controlled native GPUI `LipCard` primitive.

use std::time::Duration;

use artisan_ui::lip_card::{
    LIP_CARD_LIP_SELECTOR, LIP_CARD_PANEL_SELECTOR, LIP_CARD_ROOT_SELECTOR, LipCard,
    LipCardClipPlan, LipCardEffectPlan, LipCardHeightPlan, LipCardMotionPlan, LipCardPhase,
    LipCardState, LipCardStyle, LipCardVariant,
};
use artisan_ui::motion::{MotionPlan, MotionPolicy, MotionRecipe};
use artisan_ui::theme::{ArtisanTheme, RadiusStep, RadiusTokens, SurfaceStep, ThemeMode};
use gpui::{
    AbsoluteLength, Context, DefiniteLength, InteractiveElement, IntoElement, Length,
    ParentElement, Render, Styled, TestAppContext, Window, div, linear_color_stop, linear_gradient,
    px,
};

const PANEL_CONTENT_SELECTOR: &str = "lip-card-panel-content";
const CUSTOM_ROOT_SELECTOR: &str = "lip-card-custom";

#[test]
fn solid_light_and_dark_paint_and_glass_geometry_match_the_audited_recipe() {
    let expected = [
        (ThemeMode::Light, SurfaceStep::S200, SurfaceStep::S125),
        (ThemeMode::Dark, SurfaceStep::S850, SurfaceStep::S900),
    ];

    for (mode, top, bottom) in expected {
        let theme = ArtisanTheme::for_mode(mode);
        let solid = LipCardStyle::resolve(theme, LipCardVariant::Solid);
        let expected_background = linear_gradient(
            0.0,
            linear_color_stop(top.oklch().to_paint(), 0.0),
            linear_color_stop(bottom.oklch().to_paint(), 1.0),
        );

        assert_eq!(solid.variant, LipCardVariant::Solid);
        assert_eq!(solid.background, Some(expected_background));
        assert_eq!(solid.corner_radius, RadiusTokens::value(RadiusStep::X2l));
        assert_eq!(solid.card_shadows.len(), 4);

        let glass = LipCardStyle::resolve(theme, LipCardVariant::Glass);
        assert_eq!(glass.variant, LipCardVariant::Glass);
        assert_eq!(glass.background, None);
        assert_eq!(glass.corner_radius, solid.corner_radius);
        assert_eq!(glass.card_shadows, solid.card_shadows);
    }
}

#[test]
fn phase_truth_table_uses_accordion_recipes_and_reverses_without_stale_entrance_state() {
    let cases = [
        (false, false, true, MotionPolicy::Full, LipCardPhase::Closed),
        (false, true, true, MotionPolicy::Full, LipCardPhase::Opening),
        (true, false, true, MotionPolicy::Full, LipCardPhase::Closing),
        (true, true, true, MotionPolicy::Full, LipCardPhase::Open),
        (false, true, false, MotionPolicy::Full, LipCardPhase::Open),
        (true, false, false, MotionPolicy::Full, LipCardPhase::Closed),
        (false, true, true, MotionPolicy::Reduced, LipCardPhase::Open),
        (
            true,
            false,
            true,
            MotionPolicy::Reduced,
            LipCardPhase::Closed,
        ),
    ];

    for (previous_open, open, animate, policy, expected_phase) in cases {
        let state = LipCardState::new(previous_open).transition(open, animate, policy);
        assert_eq!(state.phase(), expected_phase);
    }

    let opening = LipCardState::new(false).transition(true, true, MotionPolicy::Full);
    assert_eq!(opening.phase(), LipCardPhase::Opening);
    assert_eq!(
        opening
            .transition(true, true, MotionPolicy::Full)
            .generation(),
        opening.generation()
    );

    let closing = opening.transition(false, true, MotionPolicy::Full);
    assert_eq!(closing.phase(), LipCardPhase::Closing);

    let reopened = closing.transition(true, true, MotionPolicy::Full);
    assert_eq!(reopened.phase(), LipCardPhase::Opening);
    assert!(reopened.generation() > closing.generation());
    assert_ne!(reopened.generation(), opening.generation());

    let settled = reopened.settle();
    assert_eq!(settled.phase(), LipCardPhase::Open);
    assert_eq!(settled.generation(), reopened.generation());
}

#[test]
fn closed_phase_is_absent_and_inert_while_close_phase_is_explicitly_present() {
    let closed = LipCardState::new(false);
    assert_eq!(closed.phase(), LipCardPhase::Closed);
    assert!(!closed.phase().panel_present());
    assert!(closed.phase().is_inert());
    assert!(closed.phase().is_pointer_inert());
    assert!(closed.phase().is_focus_inert());

    let close = LipCardState::new(true).transition(false, true, MotionPolicy::Full);
    assert_eq!(close.phase(), LipCardPhase::Closing);
    assert!(close.phase().panel_present());
    assert!(close.phase().is_pointer_inert());
    assert!(!close.phase().is_focus_inert());
}

#[test]
fn motion_plan_honestly_separates_full_reduced_and_disabled_paths() {
    let opening = LipCardMotionPlan::for_phase(LipCardPhase::Opening, true, MotionPolicy::Full);
    let opening_animation = opening.animation().expect("full opening must animate");
    assert_eq!(opening.recipe(), Some(MotionRecipe::AccordionExpand));
    assert_eq!(opening.motion, MotionPlan::Animate(opening_animation));
    assert_eq!(opening_animation.duration(), Duration::from_millis(250));
    assert_eq!(opening.opacity, LipCardEffectPlan::Animated);
    assert_eq!(opening.blur, LipCardEffectPlan::UnsupportedByGpui);
    assert_eq!(opening.clip, LipCardClipPlan::StaticOverflowHidden);
    assert_eq!(opening.height, LipCardHeightPlan::NaturalLayout);
    assert!(opening.panel_present());

    let closing = LipCardMotionPlan::for_phase(LipCardPhase::Closing, true, MotionPolicy::Full);
    assert_eq!(closing.recipe(), Some(MotionRecipe::AccordionCollapse));
    assert!(closing.animation().is_some());
    assert_eq!(closing.opacity, LipCardEffectPlan::Animated);
    assert!(closing.panel_present());

    let reduced = LipCardMotionPlan::for_phase(LipCardPhase::Opening, true, MotionPolicy::Reduced);
    assert_eq!(reduced.motion, MotionPlan::Immediate);
    assert_eq!(reduced.effective_phase(), LipCardPhase::Open);
    assert_eq!(reduced.opacity, LipCardEffectPlan::Immediate);
    assert!(reduced.panel_present());

    let disabled = LipCardMotionPlan::for_phase(LipCardPhase::Closing, false, MotionPolicy::Full);
    assert_eq!(disabled.motion, MotionPlan::Immediate);
    assert_eq!(disabled.effective_phase(), LipCardPhase::Closed);
    assert_eq!(disabled.opacity, LipCardEffectPlan::Immediate);
    assert_eq!(disabled.height, LipCardHeightPlan::ZeroWhenClosed);
    assert!(!disabled.panel_present());
}

#[test]
fn caller_root_styled_refinements_override_component_defaults() {
    let style = LipCardStyle::solid(ArtisanTheme::for_mode(ThemeMode::Dark));
    let mut card = LipCard::new(div(), div(), style, false)
        .w(px(48.0))
        .w(px(96.0));

    assert_eq!(
        card.style().size.width,
        Some(Length::Definite(DefiniteLength::Absolute(
            AbsoluteLength::Pixels(px(96.0)),
        )))
    );
    assert_eq!(
        card.visual_style().corner_radius,
        RadiusTokens::value(RadiusStep::X2l)
    );
}

struct LipCardGeometryProbe {
    state: LipCardState,
    style: LipCardStyle,
    root_selector: Option<&'static str>,
}

impl Render for LipCardGeometryProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let mut card = LipCard::new(
            div().w(px(120.0)).h(px(24.0)),
            div()
                .w(px(120.0))
                .h(px(40.0))
                .debug_selector(|| PANEL_CONTENT_SELECTOR.to_string()),
            self.style.clone(),
            self.state.is_open(),
        )
        .with_state(self.state)
        .motion_policy(MotionPolicy::Reduced);

        if let Some(selector) = self.root_selector {
            card = card.debug_selector(selector);
        }

        div().w(px(240.0)).h(px(200.0)).child(card)
    }
}

#[gpui::test]
fn closed_geometry_keeps_the_lip_and_has_zero_panel_height_and_no_descendants(
    cx: &mut TestAppContext,
) {
    let (_view, cx) = cx.add_window_view(|_, _| LipCardGeometryProbe {
        state: LipCardState::new(false),
        style: LipCardStyle::solid(ArtisanTheme::for_mode(ThemeMode::Dark)),
        root_selector: None,
    });

    let root = cx
        .debug_bounds(LIP_CARD_ROOT_SELECTOR)
        .expect("closed root selector must be present");
    let lip = cx
        .debug_bounds(LIP_CARD_LIP_SELECTOR)
        .expect("the lip must remain mounted while closed");

    assert_eq!(root.size.height, px(24.0));
    assert_eq!(lip.size.height, px(24.0));
    assert!(cx.debug_bounds(LIP_CARD_PANEL_SELECTOR).is_none());
    assert!(cx.debug_bounds(PANEL_CONTENT_SELECTOR).is_none());
}

#[gpui::test]
fn open_geometry_mounts_the_panel_and_derives_stable_custom_selectors(cx: &mut TestAppContext) {
    let (_view, cx) = cx.add_window_view(|_, _| LipCardGeometryProbe {
        state: LipCardState::new(true),
        style: LipCardStyle::glass(ArtisanTheme::for_mode(ThemeMode::Light)),
        root_selector: Some(CUSTOM_ROOT_SELECTOR),
    });

    let root = cx
        .debug_bounds(CUSTOM_ROOT_SELECTOR)
        .expect("custom root selector must be present");
    let lip = cx
        .debug_bounds("lip-card-custom-lip")
        .expect("custom lip selector must be stable");
    let panel = cx
        .debug_bounds("lip-card-custom-panel")
        .expect("open panel selector must be present");
    let content = cx
        .debug_bounds(PANEL_CONTENT_SELECTOR)
        .expect("open panel content must be mounted");

    assert_eq!(lip.size.height, px(24.0));
    assert_eq!(panel.size.height, px(40.0));
    assert_eq!(content.size.height, px(40.0));
    assert_eq!(root.size.height, px(64.0));
}
