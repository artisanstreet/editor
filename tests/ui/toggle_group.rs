#![allow(clippy::float_cmp)]

use std::cell::RefCell;
use std::rc::Rc;

use artisan_ui::button::FocusVisibility;
use artisan_ui::theme::{ArtisanTheme, ThemeMode};
use artisan_ui::toggle_group::{
    ToggleGroup, ToggleGroupChange, ToggleGroupItem, ToggleGroupOrientation, ToggleGroupSelection,
    ToggleGroupSize, ToggleGroupStyle, ToggleGroupVariant, item_debug_selector,
    stable_value_selector_suffix, toggle_group_item_selector,
};
use gpui::{
    Context, FocusHandle, IntoElement, KeyDownEvent, KeyUpEvent, Keystroke, Modifiers,
    ParentElement, Render, SharedString, Styled, TestAppContext, Window, div, px,
    transparent_black,
};

const GROUP_SELECTOR: &str = "native-toggle-group-under-test";
const VALUES: [&str; 4] = ["one", "two", "three", "four"];
const LABELS: [&str; 4] = ["One", "Two", "Three", "Four"];
const ITEM_ONE_SELECTOR: &str = "native-toggle-group-under-test-item-071558b46adadbf0";
const ITEM_TWO_SELECTOR: &str = "native-toggle-group-under-test-item-74faa9ef5fb63702";
const ITEM_THREE_SELECTOR: &str = "native-toggle-group-under-test-item-5948a0cb7e87db54";

type Events = Rc<RefCell<Vec<ToggleGroupChange<String>>>>;

fn value(index: usize) -> String {
    VALUES[index].to_owned()
}

fn single_selection(index: usize) -> ToggleGroupSelection<String> {
    ToggleGroupSelection::selected(value(index))
}

struct GroupProbe {
    focuses: Vec<FocusHandle>,
    events: Events,
    selection: ToggleGroupSelection<String>,
    disabled_items: [bool; 4],
    disabled: bool,
    orientation: ToggleGroupOrientation,
    spacing: gpui::Pixels,
    variant: ToggleGroupVariant,
    size: ToggleGroupSize,
    selector: SharedString,
    refined: bool,
}

struct GroupProbeConfig {
    selection: ToggleGroupSelection<String>,
    disabled_items: [bool; 4],
    disabled: bool,
    orientation: ToggleGroupOrientation,
    spacing: gpui::Pixels,
    variant: ToggleGroupVariant,
    size: ToggleGroupSize,
    selector: &'static str,
    refined: bool,
}

impl GroupProbe {
    fn new(cx: &mut Context<Self>, events: Events, config: GroupProbeConfig) -> Self {
        Self {
            focuses: (0..4).map(|_| cx.focus_handle()).collect(),
            events,
            selection: config.selection,
            disabled_items: config.disabled_items,
            disabled: config.disabled,
            orientation: config.orientation,
            spacing: config.spacing,
            variant: config.variant,
            size: config.size,
            selector: config.selector.into(),
            refined: config.refined,
        }
    }
}

impl Render for GroupProbe {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let theme = ArtisanTheme::for_mode(ThemeMode::Light);
        let events = self.events.clone();
        let mut group = ToggleGroup::new("native-toggle-group", theme, self.selection.clone())
            .variant(self.variant)
            .size(self.size)
            .orientation(self.orientation)
            .spacing(self.spacing)
            .disabled(self.disabled)
            .focus_visibility(FocusVisibility::Visible)
            .debug_selector(self.selector.clone())
            .on_change(move |change, _, _, _| {
                events.borrow_mut().push(change);
            });

        for (index, label) in LABELS.iter().copied().enumerate() {
            let item = ToggleGroupItem::new(value(index), label, self.focuses[index].clone())
                .disabled(self.disabled_items[index]);
            group = group.with_item(item);
        }

        let group = if self.refined {
            group
                .w(px(240.0))
                .h(px(50.0))
                .bg(theme.colors.accent.to_paint())
        } else {
            group
        };

        div().size_full().p(px(20.0)).child(group)
    }
}

#[test]
fn selection_modes_are_explicit_and_requests_do_not_mutate_controlled_state() {
    let single = ToggleGroupSelection::selected(String::from("one"));
    assert_eq!(
        single.mode(),
        artisan_ui::toggle_group::ToggleGroupMode::Single
    );
    assert!(single.contains(&String::from("one")));

    let next_single = single.next_for(&String::from("two"), true);
    assert_eq!(
        next_single,
        ToggleGroupSelection::selected(String::from("two"))
    );
    assert_eq!(single, ToggleGroupSelection::selected(String::from("one")));

    let cleared_single = single.next_for(&String::from("one"), true);
    assert_eq!(cleared_single, ToggleGroupSelection::Single(None));

    let persistent_single = single.next_for(&String::from("one"), false);
    assert_eq!(
        persistent_single,
        ToggleGroupSelection::selected(String::from("one"))
    );

    let multiple = ToggleGroupSelection::multiple([String::from("one"), String::from("three")]);
    assert_eq!(
        multiple.next_for(&String::from("two"), true),
        ToggleGroupSelection::multiple([
            String::from("one"),
            String::from("three"),
            String::from("two"),
        ])
    );
    assert_eq!(
        multiple.next_for(&String::from("one"), true),
        ToggleGroupSelection::multiple([String::from("three")])
    );
    assert_eq!(multiple.len(), 2);
    assert_eq!(
        multiple.mode(),
        artisan_ui::toggle_group::ToggleGroupMode::Multiple
    );
}

#[test]
fn styles_follow_shared_theme_variants_sizes_spacing_and_focus_tokens() {
    let light = ArtisanTheme::for_mode(ThemeMode::Light);
    let dark = ArtisanTheme::for_mode(ThemeMode::Dark);

    let default_style = ToggleGroupStyle::resolve(
        light,
        ToggleGroupVariant::Default,
        ToggleGroupSize::Default,
        px(0.0),
    );
    assert_eq!(default_style.height, px(36.0));
    assert_eq!(default_style.min_width, px(36.0));
    assert_eq!(default_style.horizontal_padding, px(12.0));
    assert_eq!(default_style.background, transparent_black());
    assert_eq!(default_style.border, transparent_black());
    assert_eq!(
        default_style.selected_background,
        light.colors.muted.to_paint()
    );
    assert_eq!(default_style.focus_ring_width, px(3.0));
    assert_eq!(default_style.corner_radius, px(26.0));
    assert_eq!(default_style.joined_corner_radius, px(22.0));

    let outline_style = ToggleGroupStyle::resolve(
        light,
        ToggleGroupVariant::Outline,
        ToggleGroupSize::Large,
        px(8.0),
    );
    assert_eq!(outline_style.height, px(40.0));
    assert_eq!(outline_style.min_width, px(40.0));
    assert_eq!(outline_style.horizontal_padding, px(16.0));
    assert_eq!(outline_style.gap, px(8.0));
    assert_eq!(outline_style.border, light.colors.input.to_paint());

    let small_style = ToggleGroupStyle::resolve(
        light,
        ToggleGroupVariant::Default,
        ToggleGroupSize::Small,
        px(0.0),
    );
    assert_eq!(small_style.height, px(32.0));

    let dark_style = ToggleGroupStyle::resolve(
        dark,
        ToggleGroupVariant::Outline,
        ToggleGroupSize::Default,
        px(0.0),
    );
    assert_eq!(
        dark_style.hover_background,
        dark.colors.muted.with_alpha(0.5).to_paint()
    );
    assert_eq!(dark_style.focus_border, dark.colors.ring.to_paint());
    assert_eq!(dark_style.disabled_opacity, 0.5);
}

#[test]
fn selectors_are_stable_and_do_not_include_arbitrary_value_text() {
    let secret = String::from("secret-value-that-must-not-leak");
    let first = stable_value_selector_suffix(&secret);
    let second = stable_value_selector_suffix(&secret);
    assert_eq!(first, second);
    assert!(!first.contains(secret.as_str()));

    let selector = item_debug_selector("toggle-root", &secret);
    assert_eq!(selector, toggle_group_item_selector("toggle-root", &secret));
    assert!(selector.starts_with("toggle-root-item-"));
    assert!(!selector.contains(secret.as_str()));
}

#[gpui::test]
fn pointer_activation_emits_one_request_and_keeps_selection_controlled(cx: &mut TestAppContext) {
    let events = Rc::new(RefCell::new(Vec::new()));
    let events_for_view = events.clone();
    let (view, cx) = cx.add_window_view(move |_, cx| {
        GroupProbe::new(
            cx,
            events_for_view,
            GroupProbeConfig {
                selection: single_selection(0),
                disabled_items: [false; 4],
                disabled: false,
                orientation: ToggleGroupOrientation::Horizontal,
                spacing: px(0.0),
                variant: ToggleGroupVariant::Outline,
                size: ToggleGroupSize::Default,
                selector: GROUP_SELECTOR,
                refined: false,
            },
        )
    });

    let bounds = cx
        .debug_bounds(ITEM_TWO_SELECTOR)
        .expect("second toggle item must expose debug bounds");
    cx.simulate_click(bounds.center(), Modifiers::none());

    cx.update(|_, app| {
        assert_eq!(view.read(app).selection, single_selection(0));
    });

    let changes = events.borrow();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].value, value(1));
    assert!(changes[0].pressed);
    assert_eq!(changes[0].selection, single_selection(1));
}

#[gpui::test]
fn enter_and_space_emit_one_complete_keyboard_request_each(cx: &mut TestAppContext) {
    let events = Rc::new(RefCell::new(Vec::new()));
    let events_for_view = events.clone();
    let (view, cx) = cx.add_window_view(move |window, cx| {
        let probe = GroupProbe::new(
            cx,
            events_for_view,
            GroupProbeConfig {
                selection: single_selection(0),
                disabled_items: [false; 4],
                disabled: false,
                orientation: ToggleGroupOrientation::Horizontal,
                spacing: px(0.0),
                variant: ToggleGroupVariant::Default,
                size: ToggleGroupSize::Default,
                selector: GROUP_SELECTOR,
                refined: false,
            },
        );
        window.focus(&probe.focuses[1]);
        probe
    });
    cx.run_until_parked();

    cx.simulate_event(KeyUpEvent {
        keystroke: Keystroke::parse("enter").expect("enter is a valid keystroke"),
    });
    assert_eq!(events.borrow().len(), 1);
    assert_eq!(events.borrow()[0].value, value(1));
    assert!(events.borrow()[0].pressed);

    let next_selection = events.borrow()[0].selection.clone();
    cx.update(|_, app| {
        view.update(app, |probe, cx| {
            probe.selection = next_selection;
            cx.notify();
        });
    });
    cx.run_until_parked();

    cx.simulate_event(KeyUpEvent {
        keystroke: Keystroke::parse("space").expect("space is a valid keystroke"),
    });

    let changes = events.borrow();
    assert_eq!(changes.len(), 2);
    assert_eq!(changes[1].value, value(1));
    assert!(!changes[1].pressed);
    assert_eq!(changes[1].selection, ToggleGroupSelection::Single(None));
}

#[gpui::test]
fn activation_key_down_is_inert_until_gpui_key_up_synthesis(cx: &mut TestAppContext) {
    let events = Rc::new(RefCell::new(Vec::new()));
    let events_for_view = events.clone();
    let (_view, cx) = cx.add_window_view(move |window, cx| {
        let probe = GroupProbe::new(
            cx,
            events_for_view,
            GroupProbeConfig {
                selection: single_selection(0),
                disabled_items: [false; 4],
                disabled: false,
                orientation: ToggleGroupOrientation::Horizontal,
                spacing: px(0.0),
                variant: ToggleGroupVariant::Default,
                size: ToggleGroupSize::Default,
                selector: GROUP_SELECTOR,
                refined: false,
            },
        );
        window.focus(&probe.focuses[0]);
        probe
    });
    cx.run_until_parked();

    for key in ["enter", "space"] {
        cx.simulate_event(KeyDownEvent {
            keystroke: Keystroke::parse(key).expect("activation key must parse"),
            is_held: false,
        });
    }

    assert!(events.borrow().is_empty());
}

#[gpui::test]
fn disabled_group_and_items_block_pointer_and_keyboard_activation(cx: &mut TestAppContext) {
    let events = Rc::new(RefCell::new(Vec::new()));
    let events_for_view = events.clone();
    let (view, cx) = cx.add_window_view(move |_, cx| {
        GroupProbe::new(
            cx,
            events_for_view,
            GroupProbeConfig {
                selection: single_selection(0),
                disabled_items: [false; 4],
                disabled: true,
                orientation: ToggleGroupOrientation::Horizontal,
                spacing: px(0.0),
                variant: ToggleGroupVariant::Default,
                size: ToggleGroupSize::Default,
                selector: GROUP_SELECTOR,
                refined: false,
            },
        )
    });

    let enabled_item = cx
        .debug_bounds(ITEM_ONE_SELECTOR)
        .expect("disabled group remains visible");
    cx.simulate_click(enabled_item.center(), Modifiers::none());
    assert!(events.borrow().is_empty());

    cx.update(|_, app| {
        view.update(app, |probe, cx| {
            probe.disabled = false;
            probe.disabled_items[1] = true;
            cx.notify();
        });
    });
    cx.run_until_parked();

    let disabled_item = cx
        .debug_bounds(ITEM_TWO_SELECTOR)
        .expect("disabled item remains visible");
    cx.simulate_click(disabled_item.center(), Modifiers::none());
    assert!(events.borrow().is_empty());

    cx.update(|window, app| {
        let focus = view.read(app).focuses[1].clone();
        window.focus(&focus);
    });
    cx.simulate_event(KeyUpEvent {
        keystroke: Keystroke::parse("enter").expect("enter is a valid keystroke"),
    });
    assert!(events.borrow().is_empty());

    let active_item = cx
        .debug_bounds(ITEM_THREE_SELECTOR)
        .expect("enabled item remains visible");
    cx.simulate_click(active_item.center(), Modifiers::none());
    assert_eq!(events.borrow().len(), 1);
    assert_eq!(
        ToggleGroupStyle::resolve(
            ArtisanTheme::for_mode(ThemeMode::Light),
            ToggleGroupVariant::Default,
            ToggleGroupSize::Default,
            px(0.0),
        )
        .disabled_opacity,
        0.5
    );
}

#[gpui::test]
fn horizontal_roving_focus_skips_disabled_items_wraps_and_honors_home_end(cx: &mut TestAppContext) {
    let events = Rc::new(RefCell::new(Vec::new()));
    let events_for_view = events.clone();
    let (view, cx) = cx.add_window_view(move |_, cx| {
        GroupProbe::new(
            cx,
            events_for_view,
            GroupProbeConfig {
                selection: single_selection(0),
                disabled_items: [false, true, false, false],
                disabled: false,
                orientation: ToggleGroupOrientation::Horizontal,
                spacing: px(0.0),
                variant: ToggleGroupVariant::Default,
                size: ToggleGroupSize::Default,
                selector: GROUP_SELECTOR,
                refined: false,
            },
        )
    });

    cx.update(|window, app| {
        let focus = view.read(app).focuses[0].clone();
        window.focus(&focus);
    });

    cx.simulate_keystrokes("right");
    cx.update(|window, app| {
        assert!(view.read(app).focuses[2].is_focused(window));
    });

    cx.simulate_keystrokes("right");
    cx.update(|window, app| {
        assert!(view.read(app).focuses[3].is_focused(window));
    });

    cx.simulate_keystrokes("right");
    cx.update(|window, app| {
        assert!(view.read(app).focuses[0].is_focused(window));
    });

    cx.simulate_keystrokes("up");
    cx.update(|window, app| {
        assert!(view.read(app).focuses[0].is_focused(window));
    });

    cx.simulate_keystrokes("end");
    cx.update(|window, app| {
        assert!(view.read(app).focuses[3].is_focused(window));
    });

    cx.simulate_keystrokes("home");
    cx.update(|window, app| {
        assert!(view.read(app).focuses[0].is_focused(window));
    });
    assert!(events.borrow().is_empty());
}

#[gpui::test]
fn vertical_roving_focus_uses_up_down_and_keeps_cross_axis_inert(cx: &mut TestAppContext) {
    let events = Rc::new(RefCell::new(Vec::new()));
    let events_for_view = events.clone();
    let (view, cx) = cx.add_window_view(move |_, cx| {
        GroupProbe::new(
            cx,
            events_for_view,
            GroupProbeConfig {
                selection: ToggleGroupSelection::Single(None),
                disabled_items: [false, true, false, false],
                disabled: false,
                orientation: ToggleGroupOrientation::Vertical,
                spacing: px(0.0),
                variant: ToggleGroupVariant::Outline,
                size: ToggleGroupSize::Small,
                selector: GROUP_SELECTOR,
                refined: false,
            },
        )
    });

    cx.update(|window, app| {
        let focus = view.read(app).focuses[0].clone();
        window.focus(&focus);
    });

    cx.simulate_keystrokes("down");
    cx.update(|window, app| {
        assert!(view.read(app).focuses[2].is_focused(window));
    });

    cx.simulate_keystrokes("right");
    cx.update(|window, app| {
        assert!(view.read(app).focuses[2].is_focused(window));
    });

    cx.simulate_keystrokes("up");
    cx.update(|window, app| {
        assert!(view.read(app).focuses[0].is_focused(window));
    });

    cx.simulate_keystrokes("end");
    cx.update(|window, app| {
        assert!(view.read(app).focuses[3].is_focused(window));
    });

    cx.simulate_keystrokes("down");
    cx.update(|window, app| {
        assert!(view.read(app).focuses[0].is_focused(window));
    });
    assert!(events.borrow().is_empty());
}

#[gpui::test]
fn zero_spacing_joins_items_positive_spacing_preserves_gap_and_selectors(cx: &mut TestAppContext) {
    let events = Rc::new(RefCell::new(Vec::new()));
    let events_for_view = events.clone();
    let (view, cx) = cx.add_window_view(move |_, cx| {
        GroupProbe::new(
            cx,
            events_for_view,
            GroupProbeConfig {
                selection: ToggleGroupSelection::Single(None),
                disabled_items: [false; 4],
                disabled: false,
                orientation: ToggleGroupOrientation::Horizontal,
                spacing: px(0.0),
                variant: ToggleGroupVariant::Outline,
                size: ToggleGroupSize::Default,
                selector: GROUP_SELECTOR,
                refined: false,
            },
        )
    });

    let zero_first = cx
        .debug_bounds(ITEM_ONE_SELECTOR)
        .expect("first zero-spacing item must paint");
    let zero_second = cx
        .debug_bounds(ITEM_TWO_SELECTOR)
        .expect("second zero-spacing item must paint");
    let root = cx
        .debug_bounds(GROUP_SELECTOR)
        .expect("group root must expose debug bounds");
    assert_eq!(
        zero_second.origin.x - (zero_first.origin.x + zero_first.size.width),
        px(0.0)
    );
    assert!(root.size.width > zero_second.origin.x - root.origin.x);

    cx.update(|_, app| {
        view.update(app, |probe, cx| {
            probe.spacing = px(8.0);
            cx.notify();
        });
    });
    cx.run_until_parked();

    let positive_first = cx
        .debug_bounds(ITEM_ONE_SELECTOR)
        .expect("first positive-spacing item must paint");
    let positive_second = cx
        .debug_bounds(ITEM_TWO_SELECTOR)
        .expect("second positive-spacing item must paint");
    assert_eq!(
        positive_second.origin.x - (positive_first.origin.x + positive_first.size.width),
        px(8.0)
    );
    assert_eq!(positive_first.size.height, px(36.0));
    assert_eq!(positive_second.size.height, px(36.0));
}

#[gpui::test]
fn caller_styled_refinements_override_root_defaults(cx: &mut TestAppContext) {
    let events = Rc::new(RefCell::new(Vec::new()));
    let events_for_view = events.clone();
    let (_view, cx) = cx.add_window_view(move |_, cx| {
        GroupProbe::new(
            cx,
            events_for_view,
            GroupProbeConfig {
                selection: single_selection(0),
                disabled_items: [false; 4],
                disabled: false,
                orientation: ToggleGroupOrientation::Horizontal,
                spacing: px(0.0),
                variant: ToggleGroupVariant::Default,
                size: ToggleGroupSize::Small,
                selector: GROUP_SELECTOR,
                refined: true,
            },
        )
    });

    let root = cx
        .debug_bounds(GROUP_SELECTOR)
        .expect("refined group root must expose debug bounds");
    assert_eq!(root.size.width, px(240.0));
    assert_eq!(root.size.height, px(50.0));

    let item = cx
        .debug_bounds(ITEM_ONE_SELECTOR)
        .expect("refined group item must expose debug bounds");
    assert_eq!(item.size.height, px(32.0));
}
