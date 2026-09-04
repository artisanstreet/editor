//! Behavioral coverage for the compact native option selector.

use artisan_ui::button::FocusVisibility;
use artisan_ui::native_select::{
    DEFAULT_DEBUG_SELECTOR, NativeSelect, NativeSelectConstructionError, NativeSelectOption,
    NativeSelectSize, NativeSelectStyle,
};
use artisan_ui::theme::{ArtisanTheme, RadiusStep, RadiusTokens, ThemeMode};
use gpui::{
    Context, FocusHandle, IntoElement, ParentElement, Render, Styled, TestAppContext, Window, div,
    px,
};
use std::cell::RefCell;
use std::rc::Rc;

const SELECTOR: &str = "native-select-under-test";
const VALUE_SELECTOR: &str = "native-select-under-test-value";
const PLACEHOLDER_SELECTOR: &str = "native-select-under-test-placeholder";
const CHEVRON_SELECTOR: &str = "native-select-under-test-chevron";
const EMPTY_SELECTOR: &str = "native-select-under-test-empty";

fn options() -> Vec<NativeSelectOption> {
    vec![
        NativeSelectOption::new("apple", "Apple").expect("valid option"),
        NativeSelectOption::new("banana", "Banana").expect("valid option"),
        NativeSelectOption::new("cherry", "Cherry")
            .expect("valid option")
            .disabled(true),
    ]
}

struct SelectProbe {
    focus: FocusHandle,
    value: String,
    placeholder: Option<String>,
    disabled: bool,
    invalid: bool,
    size: NativeSelectSize,
    focus_visibility: FocusVisibility,
    semantic_label: Option<String>,
}

impl SelectProbe {
    fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus: cx.focus_handle(),
            value: "apple".to_owned(),
            placeholder: Some("Pick a fruit".to_owned()),
            disabled: false,
            invalid: false,
            size: NativeSelectSize::Default,
            focus_visibility: FocusVisibility::Visible,
            semantic_label: Some("Fruit".to_owned()),
        }
    }
}

impl Render for SelectProbe {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let mut select = NativeSelect::new(
            "native-select",
            self.focus.clone(),
            ArtisanTheme::for_mode(ThemeMode::Light),
            self.value.clone(),
            options(),
        )
        .expect("probe options are valid")
        .disabled(self.disabled)
        .invalid(self.invalid)
        .size(self.size)
        .focus_visibility(self.focus_visibility)
        .debug_selector(SELECTOR);

        if let Some(placeholder) = self.placeholder.clone() {
            select = select.placeholder(placeholder);
        }
        if let Some(label) = self.semantic_label.clone() {
            select = select.semantic_label(label);
        }

        div().w(px(260.0)).h(px(80.0)).child(select)
    }
}

#[test]
fn option_construction_rejects_blank_values_and_labels() {
    assert_eq!(
        NativeSelectOption::new("  ", "Apple").err(),
        Some(NativeSelectConstructionError::EmptyOptionValue)
    );
    assert_eq!(
        NativeSelectOption::new("apple", "  ").err(),
        Some(NativeSelectConstructionError::EmptyOptionLabel)
    );
}

#[gpui::test]
fn construction_rejects_empty_options_and_duplicate_values(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, cx| SelectProbe::new(cx));
    cx.update(|_, app| {
        let probe = view.read(app);
        let theme = ArtisanTheme::for_mode(ThemeMode::Light);

        let empty: Vec<NativeSelectOption> = Vec::new();
        assert_eq!(
            NativeSelect::new("empty-select", probe.focus.clone(), theme, "", empty,).err(),
            Some(NativeSelectConstructionError::EmptyOptions)
        );

        let duplicate = vec![
            NativeSelectOption::new("apple", "Apple").expect("valid option"),
            NativeSelectOption::new("apple", "Apple duplicate").expect("valid option"),
        ];
        assert_eq!(
            NativeSelect::new(
                "duplicate-select",
                probe.focus.clone(),
                theme,
                "",
                duplicate,
            )
            .err(),
            Some(NativeSelectConstructionError::DuplicateOptionValue)
        );
    });
}

#[test]
fn style_resolves_audited_geometry_and_invalid_paints() {
    let light = ArtisanTheme::for_mode(ThemeMode::Light);
    let dark = ArtisanTheme::for_mode(ThemeMode::Dark);

    let default = NativeSelectStyle::resolve(light, NativeSelectSize::Default, false);
    assert_eq!(default.height, light.density.control_default);
    assert_eq!(default.padding_left, light.spacing.steps(3.0));
    assert_eq!(default.padding_right, light.spacing.steps(8.0));
    assert_eq!(default.padding_vertical, light.spacing.steps(1.0));
    assert_eq!(default.corner_radius, RadiusTokens::value(RadiusStep::X4l));
    assert_eq!(default.border_width, px(1.0));
    assert_eq!(default.border, light.colors.input.to_paint());
    assert_eq!(default.focus_border, light.colors.ring.to_paint());
    assert_eq!(
        default.focus_ring,
        light.interaction.focus_ring_color.to_paint()
    );
    assert_eq!(default.focus_ring_width, light.interaction.focus_ring_width);
    assert_eq!(default.text_size, light.typography.editor_text_desktop);
    assert_eq!(default.line_height, px(20.0));
    assert_eq!(default.chevron_size, px(16.0));
    assert_eq!(
        default.chevron_color,
        light.colors.muted_foreground.to_paint()
    );
    assert_eq!(default.disabled_opacity.to_bits(), 0.5_f32.to_bits());
    assert!(!default.invalid);

    let small = NativeSelectStyle::resolve(light, NativeSelectSize::Small, false);
    assert_eq!(small.height, light.density.control_sm);

    let invalid_light = NativeSelectStyle::resolve(light, NativeSelectSize::Default, true);
    assert!(invalid_light.invalid);
    assert_eq!(invalid_light.border, light.colors.destructive.to_paint());
    assert_eq!(
        invalid_light.focus_border,
        light.colors.destructive.to_paint()
    );
    assert_eq!(
        invalid_light.focus_ring,
        light.interaction.invalid_ring_color.to_paint()
    );

    let invalid_dark = NativeSelectStyle::resolve(dark, NativeSelectSize::Default, true);
    assert_eq!(
        invalid_dark.border,
        dark.colors.destructive.with_alpha(0.5).to_paint()
    );
    assert_eq!(
        invalid_dark.focus_border,
        dark.colors.destructive.with_alpha(0.5).to_paint()
    );
    assert_eq!(
        invalid_dark.focus_ring,
        dark.colors.destructive.with_alpha(0.4).to_paint()
    );
}

#[gpui::test]
fn semantics_keep_controlled_value_placeholder_and_disabled_state(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, cx| SelectProbe::new(cx));
    let semantics = cx.update(|_, app| {
        let probe = view.read(app);
        NativeSelect::new(
            "semantic-select",
            probe.focus.clone(),
            ArtisanTheme::for_mode(ThemeMode::Light),
            "",
            options(),
        )
        .expect("options valid")
        .placeholder("Pick a fruit")
        .invalid(true)
        .semantic_label("Fruit")
        .semantics()
    });

    assert!(!semantics.flags.has_value());
    assert!(semantics.flags.shows_placeholder());
    assert!(!semantics.flags.is_disabled());
    assert!(semantics.flags.is_invalid());
    assert!(semantics.flags.is_focusable());
    assert!(!semantics.selected_is_disabled);
    assert_eq!(semantics.value.as_str(), "");
    assert_eq!(semantics.selected_label, None);
    assert_eq!(
        semantics.placeholder.as_ref().map(|v| v.as_str()),
        Some("Pick a fruit")
    );
    assert_eq!(semantics.option_count, 3);
    assert_eq!(
        semantics.semantic_label.as_ref().map(|v| v.as_str()),
        Some("Fruit")
    );

    let matched = cx.update(|_, app| {
        let probe = view.read(app);
        NativeSelect::new(
            "matched-select",
            probe.focus.clone(),
            ArtisanTheme::for_mode(ThemeMode::Light),
            "banana",
            options(),
        )
        .expect("options valid")
        .semantics()
    });
    assert!(matched.flags.has_value());
    assert!(!matched.flags.shows_placeholder());
    assert_eq!(
        matched.selected_label.as_ref().map(|v| v.as_str()),
        Some("Banana")
    );
    assert!(!matched.selected_is_disabled);

    let disabled_match = cx.update(|_, app| {
        let probe = view.read(app);
        NativeSelect::new(
            "disabled-match-select",
            probe.focus.clone(),
            ArtisanTheme::for_mode(ThemeMode::Light),
            "cherry",
            options(),
        )
        .expect("options valid")
        .semantics()
    });
    assert!(disabled_match.flags.has_value());
    assert!(disabled_match.selected_is_disabled);
    assert!(disabled_match.flags.has_disabled_selection());
}

#[gpui::test]
fn missing_value_shows_placeholder_and_empty_without_placeholder(cx: &mut TestAppContext) {
    let (_view, cx) = cx.add_window_view(|_, cx| {
        let mut probe = SelectProbe::new(cx);
        probe.value = "missing".to_owned();
        probe.placeholder = Some("Pick a fruit".to_owned());
        probe
    });

    assert!(cx.debug_bounds(PLACEHOLDER_SELECTOR).is_some());
    assert!(cx.debug_bounds(VALUE_SELECTOR).is_none());

    let (_view2, cx2) = cx.add_window_view(|_, cx| {
        let mut probe = SelectProbe::new(cx);
        probe.value = "missing".to_owned();
        probe.placeholder = None;
        probe
    });

    assert!(cx2.debug_bounds(EMPTY_SELECTOR).is_some());
    assert!(cx2.debug_bounds(VALUE_SELECTOR).is_none());
    assert!(cx2.debug_bounds(PLACEHOLDER_SELECTOR).is_none());
}

#[gpui::test]
fn disabled_option_is_retained_but_not_selectable_via_change_intent(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, cx| SelectProbe::new(cx));
    cx.update(|_, app| {
        let probe = view.read(app);
        let select = NativeSelect::new(
            "change-select",
            probe.focus.clone(),
            ArtisanTheme::for_mode(ThemeMode::Light),
            "apple",
            options(),
        )
        .expect("options valid");

        // Lookup is deterministic.
        let apple = select.option_for("apple").expect("apple must exist");
        assert_eq!(apple.label(), "Apple");
        assert!(!apple.is_disabled());

        let cherry = select.option_for("cherry").expect("cherry must exist");
        assert!(cherry.is_disabled());

        assert!(select.option_for("missing").is_none());

        // Change intent: enabled different value is selectable.
        let next = select.next_value_for("banana").expect("banana is enabled");
        assert_eq!(next.value.as_str(), "banana");
        assert_eq!(next.label.as_str(), "Banana");

        // Same value produces no change.
        assert!(select.next_value_for("apple").is_none());
        // Disabled option produces no change.
        assert!(select.next_value_for("cherry").is_none());
        // Missing option produces no change.
        assert!(select.next_value_for("missing").is_none());

        // Alias behaves identically.
        assert_eq!(select.change_for("banana"), select.next_value_for("banana"));

        // Disabled control suppresses all intents.
        let disabled = NativeSelect::new(
            "disabled-select",
            probe.focus.clone(),
            ArtisanTheme::for_mode(ThemeMode::Light),
            "apple",
            options(),
        )
        .expect("options valid")
        .disabled(true);
        assert!(disabled.next_value_for("banana").is_none());
        assert!(!disabled.semantics().flags.is_focusable());
    });
}

#[gpui::test]
fn change_intent_callback_receives_next_enabled_value(cx: &mut TestAppContext) {
    let next_value = Rc::new(RefCell::new(None::<String>));
    let next_for_view = next_value.clone();
    let (view, cx) = cx.add_window_view(move |_, cx| {
        struct Host {
            focus: FocusHandle,
            next_value: Rc<RefCell<Option<String>>>,
        }
        impl Render for Host {
            fn render(
                &mut self,
                _window: &mut Window,
                _cx: &mut Context<Self>,
            ) -> impl IntoElement {
                let next_value = self.next_value.clone();
                let select = NativeSelect::new(
                    "callback-select",
                    self.focus.clone(),
                    ArtisanTheme::for_mode(ThemeMode::Light),
                    "apple",
                    options(),
                )
                .expect("options valid")
                .debug_selector(SELECTOR)
                .on_change(move |change, _, _| {
                    *next_value.borrow_mut() = Some(change.value.to_string());
                });
                div().size_full().child(select)
            }
        }
        Host {
            focus: cx.focus_handle(),
            next_value: next_for_view,
        }
    });

    // Simulate a click on the control; the handler should fire with the next
    // enabled option that differs from "apple" (banana).
    let bounds = cx
        .debug_bounds(SELECTOR)
        .expect("select must expose inspectable root bounds");
    let center = gpui::point(
        bounds.origin.x + bounds.size.width / 2.0,
        bounds.origin.y + bounds.size.height / 2.0,
    );
    cx.simulate_click(center, gpui::Modifiers::none());
    assert_eq!(next_value.borrow().as_deref(), Some("banana"));

    // Disabled control must not fire.
    let next_value2: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    let next_for_view2 = next_value2.clone();
    let (_view2, cx2) = cx.add_window_view(move |_, cx| {
        struct Host2 {
            focus: FocusHandle,
            next_value: Rc<RefCell<Option<String>>>,
        }
        impl Render for Host2 {
            fn render(
                &mut self,
                _window: &mut Window,
                _cx: &mut Context<Self>,
            ) -> impl IntoElement {
                let next_value = self.next_value.clone();
                let select = NativeSelect::new(
                    "disabled-callback-select",
                    self.focus.clone(),
                    ArtisanTheme::for_mode(ThemeMode::Light),
                    "apple",
                    options(),
                )
                .expect("options valid")
                .disabled(true)
                .debug_selector("disabled-select-under-test")
                .on_change(move |change, _, _| {
                    *next_value.borrow_mut() = Some(change.value.to_string());
                });
                div().size_full().child(select)
            }
        }
        Host2 {
            focus: cx.focus_handle(),
            next_value: next_for_view2,
        }
    });
    let bounds2 = cx2
        .debug_bounds("disabled-select-under-test")
        .expect("disabled select must remain visible");
    cx2.simulate_click(
        gpui::point(
            bounds2.origin.x + bounds2.size.width / 2.0,
            bounds2.origin.y + bounds2.size.height / 2.0,
        ),
        gpui::Modifiers::none(),
    );
    assert!(next_value2.borrow().is_none());
    let _ = view;
}

#[gpui::test]
fn render_probe_paints_exact_heights_and_branches(cx: &mut TestAppContext) {
    let (_view, cx) = cx.add_window_view(|_, cx| SelectProbe::new(cx));
    let bounds = cx
        .debug_bounds(SELECTOR)
        .expect("select must expose inspectable root bounds");
    assert_eq!(bounds.size.width, px(260.0));
    assert_eq!(bounds.size.height, px(36.0));
    assert!(cx.debug_bounds(VALUE_SELECTOR).is_some());
    assert!(cx.debug_bounds(CHEVRON_SELECTOR).is_some());
    assert!(cx.debug_bounds(PLACEHOLDER_SELECTOR).is_none());

    // Small size renders 32 px.
    let (_view2, cx2) = cx.add_window_view(|_, cx| {
        let mut probe = SelectProbe::new(cx);
        probe.size = NativeSelectSize::Small;
        probe
    });
    let small_bounds = cx2
        .debug_bounds(SELECTOR)
        .expect("small select must expose bounds");
    assert_eq!(small_bounds.size.height, px(32.0));
}

#[gpui::test]
fn placeholder_branch_is_inspectable_when_value_is_missing(cx: &mut TestAppContext) {
    let (_view, cx) = cx.add_window_view(|_, cx| {
        let mut probe = SelectProbe::new(cx);
        probe.value.clear();
        probe
    });
    assert!(cx.debug_bounds(PLACEHOLDER_SELECTOR).is_some());
    assert!(cx.debug_bounds(VALUE_SELECTOR).is_none());
}

#[gpui::test]
fn focus_ring_requires_actual_focus_and_visibility_intent(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, cx| SelectProbe::new(cx));
    cx.update(|window, app| {
        let focus = view.read(app).focus.clone();
        window.focus(&focus, app);
    });
    cx.run_until_parked();

    cx.update(|window, app| {
        let focus = view.read(app).focus.clone();
        let visible = NativeSelect::new(
            "focused-select",
            focus.clone(),
            ArtisanTheme::for_mode(ThemeMode::Light),
            "apple",
            options(),
        )
        .expect("options valid")
        .focus_visibility(FocusVisibility::Visible);
        assert!(visible.focus_ring_visible(window));

        let hidden = NativeSelect::new(
            "hidden-focus-select",
            focus,
            ArtisanTheme::for_mode(ThemeMode::Light),
            "apple",
            options(),
        )
        .expect("options valid")
        .focus_visibility(FocusVisibility::Hidden);
        assert!(!hidden.focus_ring_visible(window));
    });
}

#[gpui::test]
fn disabled_select_remains_visible_but_does_not_report_focus_ring(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, cx| {
        let mut probe = SelectProbe::new(cx);
        probe.disabled = true;
        probe
    });

    let bounds = cx
        .debug_bounds(SELECTOR)
        .expect("disabled select must remain visible");
    assert_eq!(bounds.size.height, px(36.0));

    cx.update(|window, app| {
        let focus = view.read(app).focus.clone();
        window.focus(&focus, app);
        let select = NativeSelect::new(
            "disabled-select",
            focus,
            ArtisanTheme::for_mode(ThemeMode::Light),
            "apple",
            options(),
        )
        .expect("options valid")
        .disabled(true)
        .focus_visibility(FocusVisibility::Visible);
        assert!(!select.focus_ring_visible(window));
        assert!(!select.semantics().flags.is_focusable());
    });
}

#[gpui::test]
fn identity_and_accessibility_metadata_are_stable(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, cx| SelectProbe::new(cx));
    cx.update(|_, app| {
        let probe = view.read(app);
        let first = NativeSelect::new(
            "identity-select",
            probe.focus.clone(),
            ArtisanTheme::for_mode(ThemeMode::Light),
            "apple",
            options(),
        )
        .expect("options valid")
        .accessible_label("Fruit picker")
        .debug_selector("custom-select")
        .semantics();
        assert_eq!(
            first.semantic_label.as_ref().map(|v| v.as_str()),
            Some("Fruit picker")
        );

        let second = NativeSelect::new(
            "identity-select-2",
            probe.focus.clone(),
            ArtisanTheme::for_mode(ThemeMode::Light),
            "apple",
            options(),
        )
        .expect("options valid")
        .semantic_label("Fruit picker")
        .debug_selector("custom-select")
        .semantics();
        assert_eq!(first.semantic_label, second.semantic_label);

        // Default selector is stable.
        assert_eq!(DEFAULT_DEBUG_SELECTOR, "artisan-native-select");

        // Option lookup is deterministic: same input yields same output.
        let select = NativeSelect::new(
            "deterministic-select",
            probe.focus.clone(),
            ArtisanTheme::for_mode(ThemeMode::Light),
            "apple",
            options(),
        )
        .expect("options valid");
        assert_eq!(
            select.option_for("banana").map(|o| o.label()),
            Some("Banana")
        );
        assert_eq!(
            select.option_for("banana").map(|o| o.label()),
            Some("Banana")
        );
        assert_eq!(select.options().len(), 3);
    });
}

#[gpui::test]
fn invalid_state_is_retained_in_semantics_and_style(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, cx| {
        let mut probe = SelectProbe::new(cx);
        probe.invalid = true;
        probe
    });
    let bounds = cx
        .debug_bounds(SELECTOR)
        .expect("invalid select must remain visible");
    assert_eq!(bounds.size.height, px(36.0));

    cx.update(|_, app| {
        let probe = view.read(app);
        let select = NativeSelect::new(
            "invalid-select",
            probe.focus.clone(),
            ArtisanTheme::for_mode(ThemeMode::Light),
            "apple",
            options(),
        )
        .expect("options valid")
        .invalid(true)
        .semantics();
        assert!(select.flags.is_invalid());
    });
}
