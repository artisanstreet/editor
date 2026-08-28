//! Behavioral coverage for the native controlled GPUI input surface.

use artisan_ui::button::FocusVisibility;
use artisan_ui::input::{Input, InputStyle, InputType};
use artisan_ui::input_state::TextInputState;
use artisan_ui::theme::{ArtisanTheme, RadiusStep, RadiusTokens, SurfaceStep, ThemeMode};
use gpui::{
    Context, FocusHandle, IntoElement, ParentElement, Render, Styled, TestAppContext, Window, div,
    px,
};

const INPUT_SELECTOR: &str = "native-input-under-test";
const VALUE_SELECTOR: &str = "native-input-under-test-value";
const PLACEHOLDER_SELECTOR: &str = "native-input-under-test-placeholder";

struct InputProbe {
    focus: FocusHandle,
    value: String,
    disabled: bool,
    invalid: bool,
    focus_visibility: FocusVisibility,
}

impl InputProbe {
    fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus: cx.focus_handle(),
            value: "hello".to_owned(),
            disabled: false,
            invalid: false,
            focus_visibility: FocusVisibility::Visible,
        }
    }
}

impl Render for InputProbe {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let input = Input::new(
            "native-input",
            self.focus.clone(),
            ArtisanTheme::for_mode(ThemeMode::Light),
            self.value.clone(),
        )
        .placeholder("Type a value")
        .disabled(self.disabled)
        .invalid(self.invalid)
        .focus_visibility(self.focus_visibility)
        .semantic_label("Value")
        .debug_selector(INPUT_SELECTOR);

        div().w(px(240.0)).h(px(80.0)).child(input)
    }
}

#[test]
fn style_resolves_audited_geometry_and_light_dark_surfaces() {
    let light = ArtisanTheme::for_mode(ThemeMode::Light);
    let dark = ArtisanTheme::for_mode(ThemeMode::Dark);

    let light_style = InputStyle::resolve(light, false);
    assert_eq!(light_style.height, light.density.control_default);
    assert_eq!(light_style.horizontal_padding, light.spacing.steps(3.0));
    assert_eq!(light_style.vertical_padding, light.spacing.steps(1.0));
    assert_eq!(
        light_style.corner_radius,
        RadiusTokens::value(RadiusStep::X4l)
    );
    assert_eq!(light_style.border_width, px(1.0));
    assert_eq!(
        light_style.background,
        light.surfaces.value(SurfaceStep::S100).to_paint()
    );
    assert_eq!(light_style.border, light.colors.input.to_paint());
    assert_eq!(light_style.focus_border, light.colors.ring.to_paint());
    assert_eq!(
        light_style.focus_ring,
        light.interaction.focus_ring_color.to_paint()
    );
    assert_eq!(
        light_style.focus_ring_width,
        light.interaction.focus_ring_width
    );
    assert_eq!(light_style.text_size, light.typography.editor_text_desktop);
    assert_eq!(light_style.line_height, px(20.0));
    assert_eq!(light_style.disabled_opacity.to_bits(), 0.5_f32.to_bits());

    let dark_style = InputStyle::resolve(dark, false);
    assert_eq!(
        dark_style.background,
        dark.surfaces.value(SurfaceStep::S900).to_paint()
    );
    assert_eq!(dark_style.border, dark.colors.input.to_paint());
    assert_eq!(
        dark_style.placeholder_foreground,
        dark.colors.muted_foreground.to_paint()
    );
}

#[test]
fn invalid_style_uses_destructive_border_and_ring_for_each_mode() {
    let light = ArtisanTheme::for_mode(ThemeMode::Light);
    let dark = ArtisanTheme::for_mode(ThemeMode::Dark);

    let light_style = InputStyle::resolve(light, true);
    assert!(light_style.invalid);
    assert_eq!(light_style.border, light.colors.destructive.to_paint());
    assert_eq!(
        light_style.focus_border,
        light.colors.destructive.to_paint()
    );
    assert_eq!(
        light_style.focus_ring,
        light.interaction.invalid_ring_color.to_paint()
    );

    let dark_style = InputStyle::resolve(dark, true);
    assert!(dark_style.invalid);
    assert_eq!(
        dark_style.border,
        dark.colors.destructive.with_alpha(0.5).to_paint()
    );
    assert_eq!(
        dark_style.focus_border,
        dark.colors.destructive.with_alpha(0.5).to_paint()
    );
    assert_eq!(
        dark_style.focus_ring,
        dark.colors.destructive.with_alpha(0.4).to_paint()
    );
}

#[gpui::test]
fn semantics_keep_controlled_state_and_file_branch_explicit(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, cx| InputProbe::new(cx));
    let semantics = cx.update(|_, app| {
        let probe = view.read(app);
        Input::new(
            "semantic-input",
            probe.focus.clone(),
            ArtisanTheme::for_mode(ThemeMode::Light),
            "",
        )
        .placeholder("Search")
        .input_type(InputType::File)
        .invalid(true)
        .semantic_label("Choose file")
        .semantics()
    });

    assert_eq!(semantics.input_type, InputType::File);
    assert!(semantics.input_type.is_file());
    assert!(!semantics.input_type.is_non_file());
    assert!(InputType::Number.is_non_file());
    assert!(!semantics.flags.has_value());
    assert!(semantics.flags.shows_placeholder());
    assert!(!semantics.flags.is_disabled());
    assert!(semantics.flags.is_invalid());
    assert!(semantics.flags.is_focusable());
    assert_eq!(
        semantics
            .semantic_label
            .as_ref()
            .map(gpui::SharedString::as_str),
        Some("Choose file")
    );
}

#[gpui::test]
fn from_state_uses_the_canonical_normalized_value_without_owning_state(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, cx| InputProbe::new(cx));
    let value = cx.update(|_, app| {
        let mut state = TextInputState::default();
        assert!(state.set_value("a\r\nb\u{200B}"));

        Input::from_state(
            "state-input",
            view.read(app).focus.clone(),
            ArtisanTheme::for_mode(ThemeMode::Light),
            &state,
        )
        .value()
        .to_owned()
    });

    assert_eq!(value, "a\nb");
}

#[gpui::test]
fn render_probe_paints_exact_height_and_value_branch(cx: &mut TestAppContext) {
    let (_view, cx) = cx.add_window_view(|_, cx| InputProbe::new(cx));
    let bounds = cx
        .debug_bounds(INPUT_SELECTOR)
        .expect("input must expose inspectable root bounds");

    assert_eq!(bounds.size.width, px(240.0));
    assert_eq!(bounds.size.height, px(36.0));
    assert!(cx.debug_bounds(VALUE_SELECTOR).is_some());
    assert!(cx.debug_bounds(PLACEHOLDER_SELECTOR).is_none());
}

#[gpui::test]
fn focus_ring_requires_actual_focus_and_visibility_intent(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, cx| InputProbe::new(cx));

    cx.update(|window, app| {
        let focus = view.read(app).focus.clone();
        window.focus(&focus);
    });
    cx.run_until_parked();

    cx.update(|window, app| {
        let focus = view.read(app).focus.clone();
        let visible = Input::new(
            "focused-input",
            focus.clone(),
            ArtisanTheme::for_mode(ThemeMode::Light),
            "value",
        )
        .focus_visibility(FocusVisibility::Visible);
        assert!(visible.focus_ring_visible(window));

        let hidden = Input::new(
            "hidden-focus-input",
            focus,
            ArtisanTheme::for_mode(ThemeMode::Light),
            "value",
        )
        .focus_visibility(FocusVisibility::Hidden);
        assert!(!hidden.focus_ring_visible(window));
    });
}

#[gpui::test]
fn disabled_input_remains_visible_but_does_not_report_focus_ring(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, cx| {
        let mut probe = InputProbe::new(cx);
        probe.disabled = true;
        probe
    });

    let bounds = cx
        .debug_bounds(INPUT_SELECTOR)
        .expect("disabled input must remain visible");
    assert_eq!(bounds.size.height, px(36.0));

    cx.update(|window, app| {
        let focus = view.read(app).focus.clone();
        window.focus(&focus);

        let input = Input::new(
            "disabled-input",
            focus,
            ArtisanTheme::for_mode(ThemeMode::Light),
            "value",
        )
        .disabled(true)
        .focus_visibility(FocusVisibility::Visible);

        assert!(!input.focus_ring_visible(window));
        assert!(!input.semantics().flags.is_focusable());
    });
}

#[gpui::test]
fn placeholder_branch_is_inspectable_when_value_is_empty(cx: &mut TestAppContext) {
    let (_view, cx) = cx.add_window_view(|_, cx| {
        let mut probe = InputProbe::new(cx);
        probe.value.clear();
        probe
    });

    assert!(cx.debug_bounds(PLACEHOLDER_SELECTOR).is_some());
    assert!(cx.debug_bounds(VALUE_SELECTOR).is_none());
}
