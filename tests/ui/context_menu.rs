use std::cell::Cell;
use std::rc::Rc;

use artisan_ui::context_menu::{
    CONTEXT_MENU_CONTENT_SELECTOR, CONTEXT_MENU_TRIGGER_SELECTOR, ContextMenu, ContextMenuEntry,
    ContextMenuFutureExtension, ContextMenuGroup, ContextMenuItem, ContextMenuPhase,
    ContextMenuPlacement, ContextMenuState, ContextMenuStyle, ContextMenuTransition,
    context_menu_geometry, resolve_context_menu_geometry,
};
use artisan_ui::motion::{MotionPlan, MotionPolicy};
use artisan_ui::theme::{ArtisanTheme, RadiusStep, RadiusTokens, ThemeMode};
use gpui::{Modifiers, MouseButton, ParentElement, Styled, TestAppContext, div, point, px, size};

#[test]
fn entries_preserve_actions_and_name_the_unsupported_extension_boundary() {
    let item = ContextMenuItem::new("settle", "Settle")
        .shortcut("Enter")
        .destructive(true)
        .inset(true)
        .disabled(true);

    assert_eq!(item.id().as_ref(), "settle");
    assert_eq!(item.label().as_ref(), "Settle");
    assert_eq!(item.shortcut_text().map(AsRef::as_ref), Some("Enter"));
    assert!(!item.is_enabled());
    assert!(item.is_destructive());
    assert!(item.is_inset());

    let group = ContextMenuGroup::new("Actions").item(item.clone());
    let entries = [
        ContextMenuEntry::label("Thread"),
        ContextMenuEntry::separator(),
        ContextMenuEntry::Group(group.clone()),
    ];

    assert_eq!(entries.len(), 3);
    assert_eq!(group.heading().map(AsRef::as_ref), Some("Actions"));
    assert_eq!(group.items().len(), 1);
    assert_eq!(ContextMenuFutureExtension::ALL.len(), 3);
    assert_eq!(
        ContextMenuFutureExtension::Submenu.contract(),
        "submenu trigger/content with safe-polygon behavior"
    );
}

#[test]
fn state_opens_at_secondary_anchor_skips_disabled_rows_and_closes_before_activation() {
    let anchor = point(px(120.0), px(80.0));
    let mut state = ContextMenuState::new(3);

    state.open_at(anchor, &[true, false, true]);
    assert!(state.is_open());
    assert_eq!(state.anchor(), Some(anchor));
    assert_eq!(state.highlighted_index(), Some(0));

    assert_eq!(state.move_next(), Some(2));
    assert_eq!(state.move_next(), Some(0));
    assert_eq!(state.move_previous(), Some(2));
    assert_eq!(state.activate_highlighted(), Some(2));

    assert!(!state.is_open());
    assert_eq!(state.anchor(), None);
    assert_eq!(state.highlighted_index(), None);
}

#[test]
fn typeahead_cycles_repeated_characters_and_restarts_missed_prefixes_after_timeout() {
    let labels = ["Project", "Preview", "Delete"];
    let mut state = ContextMenuState::new(labels.len());
    state.open(point(px(20.0), px(20.0)));

    assert_eq!(state.handle_typeahead('p', 0, &labels), Some(1));
    assert_eq!(state.typeahead_buffer(), "p");
    assert_eq!(state.handle_typeahead('p', 1, &labels), Some(0));
    assert_eq!(state.handle_typeahead('d', 2, &labels), Some(2));
    assert_eq!(state.typeahead_buffer(), "d");
    assert_eq!(state.handle_typeahead('p', 1_002, &labels), Some(0));
    assert_eq!(state.typeahead_buffer(), "p");
}

#[test]
fn geometry_prefers_below_right_then_flips_each_axis_and_clamps_oversized_content() {
    let menu = size(px(192.0), px(100.0));
    let viewport = size(px(800.0), px(600.0));

    let preferred = resolve_context_menu_geometry(
        point(px(100.0), px(100.0)),
        menu,
        viewport,
        px(4.0),
        px(4.0),
    );
    assert_eq!(preferred.placement, ContextMenuPlacement::BelowRight);
    assert_eq!(preferred.bounds.origin, point(px(104.0), px(104.0)));

    let left = context_menu_geometry(
        point(px(790.0), px(100.0)),
        menu,
        viewport,
        px(4.0),
        px(4.0),
    );
    assert_eq!(left.placement, ContextMenuPlacement::BelowLeft);
    assert!(left.placement.is_left());
    assert_eq!(left.bounds.origin.x, px(594.0));

    let above = resolve_context_menu_geometry(
        point(px(100.0), px(590.0)),
        menu,
        viewport,
        px(4.0),
        px(4.0),
    );
    assert_eq!(above.placement, ContextMenuPlacement::AboveRight);
    assert!(above.placement.is_above());
    assert_eq!(above.bounds.origin.y, px(486.0));

    let clamped = resolve_context_menu_geometry(
        point(px(190.0), px(150.0)),
        size(px(900.0), px(900.0)),
        size(px(200.0), px(160.0)),
        px(4.0),
        px(4.0),
    );
    assert_eq!(clamped.bounds.origin, point(px(4.0), px(4.0)));
    assert_eq!(clamped.bounds.size, size(px(192.0), px(152.0)));
    assert!(clamped.contains(point(px(100.0), px(80.0))));
}

#[test]
fn style_matches_the_shared_theme_radius_colors_shadow_and_motion_tokens() {
    let light = ContextMenuStyle::resolve_with_motion(
        ArtisanTheme::for_mode(ThemeMode::Light),
        MotionPolicy::Full,
    );
    let dark = ContextMenuStyle::resolve_with_motion(
        ArtisanTheme::for_mode(ThemeMode::Dark),
        MotionPolicy::Reduced,
    );

    assert_eq!(light.menu_min_width, px(192.0));
    assert_eq!(light.menu_padding, px(4.0));
    assert_eq!(
        light.menu_corner_radius,
        RadiusTokens::value(RadiusStep::X2l)
    );
    assert_eq!(
        light.item_corner_radius,
        RadiusTokens::value(RadiusStep::Xl)
    );
    assert_eq!(light.item_horizontal_padding, px(12.0));
    assert_eq!(light.item_vertical_padding, px(8.0));
    assert_eq!(light.item_gap, px(10.0));
    assert_eq!(light.menu_shadow.blur_radius, px(50.0));
    assert_eq!(light.menu_shadow.spread_radius, px(-12.0));
    assert_eq!(light.menu_ring.spread_radius, px(1.0));
    assert!((light.menu_ring.color.a - 0.05).abs() < 1e-6);
    assert!((light.destructive_hover_background.a - 0.10).abs() < 1e-6);
    assert!(matches!(light.motion, MotionPlan::Animate(_)));
    assert!((dark.destructive_hover_background.a - 0.20).abs() < 1e-6);
    assert_eq!(dark.motion, MotionPlan::Immediate);
}

#[test]
fn transitions_keep_open_and_close_timing_distinct_and_reduced_motion_immediate() {
    let opening = ContextMenuTransition::Open.plan(MotionPolicy::Full);
    let closing = ContextMenuTransition::Close.plan(MotionPolicy::Full);

    assert_eq!(
        opening
            .animation()
            .expect("open animation")
            .duration()
            .as_millis(),
        250
    );
    assert_eq!(
        closing
            .animation()
            .expect("close animation")
            .duration()
            .as_millis(),
        150
    );
    assert_eq!(
        ContextMenuTransition::Open.plan(MotionPolicy::Reduced),
        MotionPlan::Immediate
    );

    assert_eq!(
        ContextMenuPhase::Closed.transition(true, true, MotionPolicy::Full),
        ContextMenuPhase::Opening
    );
    assert_eq!(
        ContextMenuPhase::Opening.transition(false, true, MotionPolicy::Full),
        ContextMenuPhase::Closing
    );
    assert!(!ContextMenuPhase::Closed.content_present());
    assert!(ContextMenuPhase::Closing.content_present());
    assert_eq!(
        ContextMenuPhase::Closed.transition(true, true, MotionPolicy::Reduced),
        ContextMenuPhase::Open
    );
}

#[gpui::test]
fn native_render_opens_on_secondary_press_activates_by_keyboard_and_dismisses_outside(
    cx: &mut TestAppContext,
) {
    let activations = Rc::new(Cell::new(0));
    let activations_for_item = Rc::clone(&activations);

    let (view, cx) = cx.add_window_view(move |_, cx| {
        ContextMenu::new(
            "native-context-menu-test",
            ArtisanTheme::for_mode(ThemeMode::Dark),
            cx,
        )
        .motion_policy(MotionPolicy::Reduced)
        .trigger(|| div().w(px(240.0)).h(px(64.0)).child("thread target"))
        .item(
            ContextMenuItem::new("settle", "Settle")
                .shortcut("Enter")
                .on_activate(move |_, _| {
                    activations_for_item.set(activations_for_item.get() + 1);
                }),
        )
        .separator()
        .item(ContextMenuItem::new("delete", "Delete").disabled(true))
    });

    cx.run_until_parked();

    let trigger = cx
        .debug_bounds(CONTEXT_MENU_TRIGGER_SELECTOR)
        .expect("the composed trigger must paint a test bound");

    cx.simulate_mouse_down(trigger.center(), MouseButton::Right, Modifiers::none());
    cx.run_until_parked();

    cx.update(|_, app| {
        assert!(view.read(app).state().is_open());
        assert_eq!(view.read(app).state().highlighted_index(), Some(0));
    });
    assert!(cx.debug_bounds(CONTEXT_MENU_CONTENT_SELECTOR).is_some());

    cx.simulate_keystrokes("down enter");
    cx.run_until_parked();

    cx.update(|window, app| {
        let menu = view.read(app);
        assert!(!menu.state().is_open());
        assert_eq!(menu.last_activation().as_deref(), Some("settle"));
        assert!(menu.trigger_focus().is_focused(window));
    });
    assert_eq!(activations.get(), 1);

    cx.simulate_mouse_down(trigger.center(), MouseButton::Right, Modifiers::none());
    cx.run_until_parked();
    assert!(cx.debug_bounds(CONTEXT_MENU_CONTENT_SELECTOR).is_some());

    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    cx.update(|_, app| assert!(!view.read(app).state().is_open()));

    cx.simulate_mouse_down(trigger.center(), MouseButton::Right, Modifiers::none());
    cx.run_until_parked();

    cx.simulate_mouse_down(
        point(px(700.0), px(500.0)),
        MouseButton::Left,
        Modifiers::none(),
    );
    cx.run_until_parked();

    cx.update(|_, app| assert!(!view.read(app).state().is_open()));
}
