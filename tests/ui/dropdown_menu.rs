//! Deterministic coverage for the native dropdown menu packet.

use artisan_ui::dropdown_menu::{
    DropdownMenu, DropdownMenuAlign, DropdownMenuEntry, DropdownMenuGeometry, DropdownMenuItem,
    DropdownMenuSide, DropdownMenuState, DropdownMenuStyle, TYPEAHEAD_BUFFER_MILLIS,
};
use artisan_ui::theme::{ArtisanTheme, RadiusStep, RadiusTokens, SurfaceStep, ThemeMode};
use gpui::{
    Bounds, Edges, IntoElement, ParentElement, Styled, TestAppContext, div, point, px, size,
};

fn entries() -> Vec<DropdownMenuEntry> {
    vec![
        DropdownMenuEntry::label("File"),
        DropdownMenuItem::new("open", "Open")
            .shortcut("Enter")
            .into(),
        DropdownMenuItem::new("rename", "Rename")
            .disabled(true)
            .into(),
        DropdownMenuEntry::separator(),
        DropdownMenuItem::new("delete", "Delete")
            .shortcut("Ôî½")
            .destructive(true)
            .into(),
    ]
}

#[test]
fn typeahead_cycles_and_skips_disabled_entries() {
    let entries = vec![
        DropdownMenuItem::new("alpha", "Alpha").into(),
        DropdownMenuItem::new("archive", "Archive")
            .disabled(true)
            .into(),
        DropdownMenuItem::new("alpine", "Alpine").into(),
        DropdownMenuItem::new("beta", "Beta").into(),
    ];
    let mut state = DropdownMenuState::with_open(entries, true);

    assert_eq!(state.handle_typeahead("a", 0), Some(0));
    assert_eq!(state.typeahead_buffer(), "a");

    assert_eq!(state.handle_typeahead("a", 100), Some(2));
    assert_eq!(state.typeahead_buffer(), "a");

    assert_eq!(state.handle_typeahead("b", 999), Some(3));
    assert_eq!(state.typeahead_buffer(), "b");

    assert_eq!(
        state.handle_typeahead("a", 999 + TYPEAHEAD_BUFFER_MILLIS),
        Some(0)
    );
    assert_eq!(state.typeahead_buffer(), "a");
}

#[test]
fn navigation_wraps_and_activation_queues_an_action() {
    let mut state = DropdownMenuState::with_open(entries(), true);

    assert_eq!(state.highlighted_index(), Some(1));
    assert_eq!(state.move_next(), Some(4));
    assert_eq!(state.move_previous(), Some(1));
    assert_eq!(state.move_first(), Some(1));
    assert_eq!(state.move_last(), Some(4));

    assert!(!state.activate_index(2));
    assert!(state.is_open());

    assert!(state.activate_highlighted());
    assert!(!state.is_open());

    let actions = state.take_actions();
    assert_eq!(actions.len(), 1);
    assert_eq!(actions[0].item_id().as_str(), "delete");
}

#[test]
fn controlled_state_dismisses_and_disabled_state_refuses_opening() {
    let mut state = DropdownMenuState::new(entries());

    state.set_open(true);
    assert!(state.is_open());
    assert_eq!(state.highlighted_index(), Some(1));

    let _ = state.handle_typeahead("d", 10);
    assert_eq!(state.typeahead_buffer(), "d");

    assert!(state.dismiss());
    assert!(!state.is_open());
    assert_eq!(state.highlighted_index(), None);
    assert_eq!(state.typeahead_buffer(), "");

    state.set_open(true);
    state.set_disabled(true);
    state.set_open(true);
    assert!(!state.is_open());

    state.set_disabled(false);
    assert!(state.press_trigger());
    assert!(state.is_open());
}

#[test]
fn labels_and_separators_are_not_selectable() {
    let mut state = DropdownMenuState::with_open(entries(), true);

    assert!(!state.is_item_enabled(0));
    assert!(!state.is_item_enabled(2));
    assert!(!state.is_item_enabled(3));
    assert_eq!(state.selectable_item_count(), 2);

    assert!(!state.activate_index(0));
    assert!(!state.activate_index(3));
    assert!(state.is_open());
}

#[test]
fn geometry_flips_and_clamps_against_viewport_margins() {
    let geometry = DropdownMenuGeometry::new(
        DropdownMenuSide::Top,
        DropdownMenuAlign::Start,
        px(10.0),
        Edges {
            top: px(8.0),
            right: px(8.0),
            bottom: px(8.0),
            left: px(8.0),
        },
    );
    let viewport = size(px(400.0), px(300.0));
    let menu_size = size(px(192.0), px(120.0));

    let above = geometry.resolve(
        Bounds::new(point(px(100.0), px(220.0)), size(px(80.0), px(32.0))),
        menu_size,
        viewport,
    );
    assert_eq!(above.bounds.origin, point(px(100.0), px(90.0)));
    assert_eq!(above.side, DropdownMenuSide::Top);
    assert!(!above.flipped);
    assert!(above.fits_within(viewport, geometry.viewport_margin));

    let flipped = geometry.resolve(
        Bounds::new(point(px(100.0), px(20.0)), size(px(80.0), px(32.0))),
        menu_size,
        viewport,
    );
    assert_eq!(flipped.side, DropdownMenuSide::Bottom);
    assert!(flipped.flipped);
    assert_eq!(flipped.bounds.origin.y, px(62.0));

    let clamped = geometry.resolve(
        Bounds::new(point(px(370.0), px(220.0)), size(px(40.0), px(32.0))),
        menu_size,
        viewport,
    );
    assert_eq!(clamped.bounds.origin.x, px(200.0));
    assert!(clamped.fits_within(viewport, geometry.viewport_margin));
}

#[test]
fn theme_recipes_follow_light_and_dark_tokens() {
    let light_theme = ArtisanTheme::for_mode(ThemeMode::Light);
    let dark_theme = ArtisanTheme::for_mode(ThemeMode::Dark);

    let light = DropdownMenuStyle::resolve(light_theme, DropdownMenuGeometry::default());
    let dark = DropdownMenuStyle::resolve(dark_theme, DropdownMenuGeometry::default());

    assert_eq!(
        light.content.corner_radius,
        RadiusTokens::value(RadiusStep::X2l)
    );
    assert_eq!(light.content.min_width, px(192.0));
    assert_eq!(light.content.padding, px(4.0));
    assert_eq!(
        light.content.background,
        light_theme.colors.popover.to_paint()
    );
    assert_eq!(
        light.content.foreground,
        light_theme.colors.popover_foreground.to_paint()
    );

    assert_eq!(light.item.horizontal_padding, px(12.0));
    assert_eq!(light.item.vertical_padding, px(8.0));
    assert_eq!(light.item.gap, px(10.0));
    assert_eq!(
        light.item.corner_radius,
        RadiusTokens::value(RadiusStep::Xl)
    );
    assert_eq!(light.item.height(), px(36.0));
    assert_eq!(light.item.disabled_opacity.to_bits(), 0.5f32.to_bits());

    assert_eq!(light.label.foreground, SurfaceStep::S500.oklch().to_paint());
    assert_eq!(
        light.shortcut.foreground,
        light_theme.colors.muted_foreground.to_paint()
    );
    assert_eq!(light.separator.height, px(1.0));
    assert_eq!(light.separator.horizontal_margin, px(-4.0));

    assert_eq!(
        light.item.destructive_focus_background,
        light_theme.colors.destructive.with_alpha(0.10).to_paint()
    );
    assert_eq!(
        dark.item.destructive_focus_background,
        dark_theme.colors.destructive.with_alpha(0.20).to_paint()
    );
    assert_ne!(
        light.item.destructive_focus_background,
        dark.item.destructive_focus_background
    );
}

#[test]
fn width_and_height_recipes_respect_viewport_space() {
    let style = DropdownMenuStyle::resolve(
        ArtisanTheme::for_mode(ThemeMode::Dark),
        DropdownMenuGeometry::default(),
    );

    assert_eq!(style.content_width(px(120.0)), px(192.0));
    assert_eq!(style.content_width(px(280.0)), px(280.0));
    assert_eq!(
        style.content_width_for_viewport(px(280.0), size(px(240.0), px(300.0))),
        px(224.0)
    );
    assert_eq!(
        style.max_height_for_viewport(size(px(400.0), px(300.0))),
        px(284.0)
    );
}

const TRIGGER_SELECTOR: &str = "dropdown-under-test-trigger";
const CONTENT_SELECTOR: &str = "dropdown-under-test-content";

#[gpui::test]
fn real_gpui_render_probe_places_the_menu_against_the_trigger(cx: &mut TestAppContext) {
    let (menu, cx) = cx.add_window_view(|_, cx| {
        let trigger_focus = cx.focus_handle();

        DropdownMenu::new(
            "dropdown-under-test",
            trigger_focus,
            ArtisanTheme::for_mode(ThemeMode::Dark),
            || {
                div()
                    .w(px(160.0))
                    .h(px(36.0))
                    .child("Open")
                    .into_any_element()
            },
            entries(),
        )
        .open(true)
        .debug_selector("dropdown-under-test")
    });

    cx.simulate_resize(size(px(320.0), px(240.0)));
    cx.run_until_parked();

    let trigger = cx
        .debug_bounds(TRIGGER_SELECTOR)
        .expect("trigger probe must expose bounds");
    let content = cx
        .debug_bounds(CONTENT_SELECTOR)
        .expect("open menu must expose content bounds");

    assert_eq!(trigger.size, size(px(160.0), px(36.0)));
    assert!(content.size.width >= px(192.0));
    assert!(content.origin.x >= px(0.0));
    assert!(content.origin.y >= px(0.0));
    assert!(content.origin.x + content.size.width <= px(320.0));
    assert!(content.origin.y + content.size.height <= px(240.0));

    cx.update(|_, app| {
        let state = menu.read(app).state();
        assert!(state.is_open());
        assert_eq!(state.highlighted_index(), Some(1));
        assert!(content.size.height > px(0.0));
    });
}
