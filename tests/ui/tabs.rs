// Exact GPUI pixel arithmetic is covered by the geometry assertions below.
#![allow(clippy::float_cmp)]

use std::cell::RefCell;
use std::rc::Rc;

use artisan_ui::tabs::{TabSpec, Tabs, TabsOrientation, TabsStyle, TabsVariant};
use artisan_ui::theme::{ArtisanTheme, ThemeMode};
use gpui::{
    Context, FocusHandle, IntoElement, KeyUpEvent, Keystroke, Modifiers, ParentElement, Render,
    Styled, TestAppContext, Window, div, px, transparent_black,
};

const LIST_SELECTOR: &str = "tabs-under-test-list";
const TRIGGER_A_SELECTOR: &str = "tabs-under-test-trigger-a";
const TRIGGER_B_SELECTOR: &str = "tabs-under-test-trigger-b";
const TRIGGER_C_SELECTOR: &str = "tabs-under-test-trigger-c";
const VALUE_A_SELECTOR: &str = "tabs-under-test-value-a";
const VALUE_B_SELECTOR: &str = "tabs-under-test-value-b";
const VALUE_C_SELECTOR: &str = "tabs-under-test-value-c";

fn specs() -> Vec<TabSpec> {
    vec![
        TabSpec::new("a", "Alpha"),
        TabSpec::new("b", "Beta"),
        TabSpec::new("c", "Gamma"),
    ]
}

fn disabled_specs() -> Vec<TabSpec> {
    vec![
        TabSpec::new("a", "Alpha"),
        TabSpec::new("b", "Beta").disabled(true),
        TabSpec::new("c", "Gamma"),
        TabSpec::new("d", "Delta").disabled(true),
        TabSpec::new("e", "Epsilon"),
    ]
}

#[derive(Clone)]
struct ProbeConfig {
    selected: String,
    orientation: TabsOrientation,
    variant: TabsVariant,
    disabled: bool,
    tabs: Vec<TabSpec>,
    values: Rc<RefCell<Vec<String>>>,
    refine: bool,
}

struct TabsProbe {
    focus: FocusHandle,
    config: ProbeConfig,
}

impl TabsProbe {
    fn new(cx: &mut Context<Self>, config: ProbeConfig) -> Self {
        Self {
            focus: cx.focus_handle(),
            config,
        }
    }
}

impl Render for TabsProbe {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let values = self.config.values.clone();
        let theme = ArtisanTheme::for_mode(ThemeMode::Light);
        let tabs = Tabs::new(
            "native-tabs",
            self.focus.clone(),
            theme,
            self.config.selected.clone(),
            self.config.tabs.clone(),
        )
        .variant(self.config.variant)
        .orientation(self.config.orientation)
        .disabled(self.config.disabled)
        .debug_selector("tabs-under-test")
        .on_change(move |value, _, _, _| values.borrow_mut().push(value.to_string()));

        let tabs = if self.config.refine {
            tabs.w(px(240.0))
                .h(px(44.0))
                .bg(theme.colors.accent.to_paint())
        } else {
            tabs
        };

        div().size_full().child(tabs)
    }
}

fn config(values: Rc<RefCell<Vec<String>>>) -> ProbeConfig {
    ProbeConfig {
        selected: "a".to_owned(),
        orientation: TabsOrientation::Horizontal,
        variant: TabsVariant::Default,
        disabled: false,
        tabs: specs(),
        values,
        refine: false,
    }
}

fn assert_values(values: &Rc<RefCell<Vec<String>>>, expected: &[&str]) {
    let actual = values.borrow();
    assert_eq!(actual.len(), expected.len());
    for (actual, expected) in actual.iter().zip(expected.iter().copied()) {
        assert_eq!(actual.as_str(), expected);
    }
}

fn center(cx: &mut gpui::VisualTestContext, selector: &'static str) -> gpui::Point<gpui::Pixels> {
    cx.debug_bounds(selector)
        .expect("selector must expose bounds")
        .center()
}

fn focus_probe(cx: &mut gpui::VisualTestContext, view: &gpui::Entity<TabsProbe>) {
    cx.update(|window, app| {
        let focus = view.read(app).focus.clone();
        window.focus(&focus);
    });
}

fn update_selected(
    cx: &mut gpui::VisualTestContext,
    view: &gpui::Entity<TabsProbe>,
    selected: &str,
) {
    cx.update(|_, app| {
        view.update(app, |probe, cx| {
            selected.clone_into(&mut probe.config.selected);
            cx.notify();
        });
    });
    cx.run_until_parked();
}

#[test]
fn default_and_line_recipes_resolve_exactly_in_both_theme_modes() {
    for mode in [ThemeMode::Light, ThemeMode::Dark] {
        let theme = ArtisanTheme::for_mode(mode);
        let default = TabsStyle::resolve(theme, TabsVariant::Default, TabsOrientation::Horizontal);
        assert_eq!(default.list_height, Some(theme.density.tabs_list_height));
        assert_eq!(default.list_padding, px(3.0));
        assert_eq!(default.list_gap, px(0.0));
        assert_eq!(default.list_corner_radius, px(26.0));
        assert_eq!(default.list_background, theme.colors.muted.to_paint());
        assert_eq!(default.trigger_horizontal_padding, px(8.0));
        assert_eq!(default.trigger_vertical_padding, px(4.0));
        assert_eq!(default.trigger_text_size, px(14.0));
        assert_eq!(default.trigger_weight, gpui::FontWeight::MEDIUM);
        assert_eq!(
            default.inactive_foreground,
            theme.colors.foreground.with_alpha(0.6).to_paint()
        );
        assert_eq!(
            default.active_foreground,
            theme.colors.foreground.to_paint()
        );
        assert_eq!(
            default.active_background,
            theme.colors.background.to_paint()
        );
        assert_eq!(default.active_corner_radius, px(12.0));
        assert_eq!(default.disabled_opacity.to_bits(), 0.5_f32.to_bits());

        let line = TabsStyle::resolve(theme, TabsVariant::Line, TabsOrientation::Horizontal);
        assert_eq!(line.list_height, Some(theme.density.tabs_list_height));
        assert_eq!(line.list_padding, px(3.0));
        assert_eq!(line.list_gap, px(4.0));
        assert_eq!(line.list_corner_radius, px(0.0));
        assert_eq!(line.list_background, transparent_black());
        assert_eq!(line.active_background, transparent_black());
        assert_eq!(line.indicator_color, theme.colors.primary.to_paint());
        assert_eq!(line.indicator_thickness, px(2.0));
    }

    let vertical = TabsStyle::resolve(
        ArtisanTheme::for_mode(ThemeMode::Light),
        TabsVariant::Default,
        TabsOrientation::Vertical,
    );
    assert_eq!(vertical.list_height, None);
    assert_eq!(vertical.list_corner_radius, px(18.0));
}

#[gpui::test]
fn pointer_activation_is_controlled_and_selected_pointer_is_a_noop(cx: &mut TestAppContext) {
    let values = Rc::new(RefCell::new(Vec::new()));
    let config = config(values.clone());
    let (view, cx) = cx.add_window_view(move |_, cx| TabsProbe::new(cx, config));

    let destination_tab_center = center(cx, TRIGGER_B_SELECTOR);
    cx.simulate_click(destination_tab_center, Modifiers::none());
    cx.update(|window, app| {
        assert!(view.read(app).focus.is_focused(window));
        assert_values(&values, &["b"]);
        assert_eq!(view.read(app).config.selected, "a");
    });

    let origin_tab_center = center(cx, TRIGGER_A_SELECTOR);
    cx.simulate_click(origin_tab_center, Modifiers::none());
    assert_values(&values, &["b"]);
}

#[gpui::test]
fn horizontal_arrows_loop_and_skip_to_the_next_controlled_value(cx: &mut TestAppContext) {
    let values = Rc::new(RefCell::new(Vec::new()));
    let config = config(values.clone());
    let (view, cx) = cx.add_window_view(move |_, cx| TabsProbe::new(cx, config));
    focus_probe(cx, &view);

    for (key, next) in [("right", "b"), ("right", "c"), ("right", "a")] {
        cx.simulate_keystrokes(key);
        assert_eq!(values.borrow().last().map(String::as_str), Some(next));
        update_selected(cx, &view, next);
    }
    assert_values(&values, &["b", "c", "a"]);
}

#[gpui::test]
fn vertical_arrows_loop_on_the_vertical_axis(cx: &mut TestAppContext) {
    let values = Rc::new(RefCell::new(Vec::new()));
    let mut config = config(values.clone());
    config.orientation = TabsOrientation::Vertical;
    config.selected = "b".to_owned();
    let (view, cx) = cx.add_window_view(move |_, cx| TabsProbe::new(cx, config));
    focus_probe(cx, &view);

    for (key, next) in [("down", "c"), ("down", "a"), ("up", "c")] {
        cx.simulate_keystrokes(key);
        assert_eq!(values.borrow().last().map(String::as_str), Some(next));
        update_selected(cx, &view, next);
    }
    assert_values(&values, &["c", "a", "c"]);
}

#[gpui::test]
fn home_end_and_arrows_skip_disabled_items(cx: &mut TestAppContext) {
    let values = Rc::new(RefCell::new(Vec::new()));
    let mut config = config(values.clone());
    config.tabs = disabled_specs();
    config.selected = "c".to_owned();
    let (view, cx) = cx.add_window_view(move |_, cx| TabsProbe::new(cx, config));
    focus_probe(cx, &view);

    let trigger_b_center_248 = center(cx, TRIGGER_B_SELECTOR);
    cx.simulate_click(trigger_b_center_248, Modifiers::none());
    assert_values(&values, &[]);

    cx.simulate_keystrokes("home");
    assert_values(&values, &["a"]);
    update_selected(cx, &view, "a");

    cx.simulate_keystrokes("right");
    assert_values(&values, &["a", "c"]);
    update_selected(cx, &view, "c");

    cx.simulate_keystrokes("end");
    assert_values(&values, &["a", "c", "e"]);
    update_selected(cx, &view, "e");

    cx.simulate_keystrokes("right");
    assert_values(&values, &["a", "c", "e", "a"]);
}

#[gpui::test]
fn automatic_activation_does_not_mutate_selected_value(cx: &mut TestAppContext) {
    let values = Rc::new(RefCell::new(Vec::new()));
    let config = config(values.clone());
    let (view, cx) = cx.add_window_view(move |_, cx| TabsProbe::new(cx, config));
    focus_probe(cx, &view);

    cx.simulate_keystrokes("right");
    cx.update(|_, app| {
        assert_eq!(view.read(app).config.selected, "a");
        assert_values(&values, &["b"]);
    });
}

#[gpui::test]
fn enter_and_space_activate_the_roved_trigger_without_internal_selection(cx: &mut TestAppContext) {
    let values = Rc::new(RefCell::new(Vec::new()));
    let config = config(values.clone());
    let (view, cx) = cx.add_window_view(move |_, cx| TabsProbe::new(cx, config));
    focus_probe(cx, &view);

    cx.simulate_keystrokes("right enter space");
    assert_values(&values, &["b", "b", "b"]);
    cx.update(|_, app| assert_eq!(view.read(app).config.selected, "a"));
}

#[gpui::test]
fn empty_all_disabled_and_component_disabled_inputs_suppress_activation(cx: &mut TestAppContext) {
    let values = Rc::new(RefCell::new(Vec::new()));
    let mut empty = config(values.clone());
    empty.tabs = Vec::new();
    let (view, cx) = cx.add_window_view(move |_, cx| TabsProbe::new(cx, empty));
    focus_probe(cx, &view);
    cx.simulate_keystrokes("home right enter space");
    assert_values(&values, &[]);

    cx.update(|_, app| {
        view.update(app, |probe, cx| {
            probe.config.tabs = vec![
                TabSpec::new("a", "Alpha").disabled(true),
                TabSpec::new("b", "Beta").disabled(true),
            ];
            probe.config.selected = "a".to_owned();
            cx.notify();
        });
    });
    cx.run_until_parked();
    focus_probe(cx, &view);
    let trigger_b_center_315 = center(cx, TRIGGER_B_SELECTOR);
    cx.simulate_click(trigger_b_center_315, Modifiers::none());
    cx.simulate_keystrokes("home end left right enter space");
    assert_values(&values, &[]);

    cx.update(|_, app| {
        view.update(app, |probe, cx| {
            probe.config.tabs = specs();
            probe.config.disabled = true;
            probe.config.selected = "a".to_owned();
            cx.notify();
        });
    });
    cx.run_until_parked();
    let trigger_b_center_328 = center(cx, TRIGGER_B_SELECTOR);
    cx.simulate_click(trigger_b_center_328, Modifiers::none());
    focus_probe(cx, &view);
    cx.simulate_keystrokes("home right enter space");
    assert_values(&values, &[]);
}

#[gpui::test]
fn caller_list_refinements_win_and_debug_geometry_is_stable(cx: &mut TestAppContext) {
    let values = Rc::new(RefCell::new(Vec::new()));
    let mut refined = config(values);
    refined.refine = true;
    let (_view, cx) = cx.add_window_view(move |_, cx| TabsProbe::new(cx, refined));

    let list = cx
        .debug_bounds(LIST_SELECTOR)
        .expect("tabs list must expose debug bounds");
    assert_eq!(list.size.width, px(240.0));
    assert_eq!(list.size.height, px(44.0));

    let trigger_a = cx
        .debug_bounds(TRIGGER_A_SELECTOR)
        .expect("first trigger must expose debug bounds");
    let trigger_b = cx
        .debug_bounds(TRIGGER_B_SELECTOR)
        .expect("second trigger must expose debug bounds");
    let trigger_c = cx
        .debug_bounds(TRIGGER_C_SELECTOR)
        .expect("third trigger must expose debug bounds");
    assert!(trigger_a.size.width > px(0.0));
    assert_eq!(trigger_a.size.height, trigger_b.size.height);
    assert_eq!(trigger_b.size.height, trigger_c.size.height);
    assert!(trigger_b.origin.x > trigger_a.origin.x);
    assert!(trigger_c.origin.x > trigger_b.origin.x);

    let value_a = cx
        .debug_bounds(VALUE_A_SELECTOR)
        .expect("first stable value selector must expose bounds");
    let value_b = cx
        .debug_bounds(VALUE_B_SELECTOR)
        .expect("second stable value selector must expose bounds");
    let value_c = cx
        .debug_bounds(VALUE_C_SELECTOR)
        .expect("third stable value selector must expose bounds");
    assert!(value_a.size.width > px(0.0));
    assert!(value_b.size.width > px(0.0));
    assert!(value_c.size.width > px(0.0));
    assert!(value_b.origin.x > value_a.origin.x);
    assert!(value_c.origin.x > value_b.origin.x);
}

#[test]
fn tab_spec_is_cloneable_and_keeps_stable_value_separate_from_label() {
    let original = TabSpec::new("engine-id", "Visible engine").disabled(true);
    let clone = original.clone();
    assert_eq!(clone.value(), "engine-id");
    assert_eq!(clone.label(), "Visible engine");
    assert!(clone.is_disabled());
    assert_eq!(clone, original);
}

#[gpui::test]
fn key_activation_paths_are_suppressed_when_selected_already_matches(cx: &mut TestAppContext) {
    let values = Rc::new(RefCell::new(Vec::new()));
    let config = config(values.clone());
    let (view, cx) = cx.add_window_view(move |window, cx| {
        let probe = TabsProbe::new(cx, config);
        window.focus(&probe.focus);
        probe
    });
    cx.run_until_parked();

    for key in ["enter", "space"] {
        cx.simulate_event(KeyUpEvent {
            keystroke: Keystroke::parse(key).expect("known keyboard activation key"),
        });
    }
    cx.update(|window, app| {
        assert!(view.read(app).focus.is_focused(window));
        assert_values(&values, &[]);
    });
}
