//! Behavioral coverage for the native controlled GPUI textarea surface.

use artisan_ui::button::FocusVisibility;
use artisan_ui::input_state::TextInputState;
use artisan_ui::textarea::{
    DEFAULT_DEBUG_SELECTOR, Textarea, TextareaFlags, TextareaStyle, textarea_has_newline,
    textarea_line_count,
};
use artisan_ui::theme::{ArtisanTheme, RadiusStep, RadiusTokens, SurfaceStep, ThemeMode};
use gpui::prelude::Refineable as _;
use gpui::{
    Context, FocusHandle, IntoElement, ParentElement, Render, Styled, TestAppContext, Window, div,
    px,
};

const TEXTAREA_SELECTOR: &str = "native-textarea-under-test";
const VALUE_SELECTOR: &str = "native-textarea-under-test-value";
const PLACEHOLDER_SELECTOR: &str = "native-textarea-under-test-placeholder";
const CUSTOM_PROBE_SELECTOR: &str = "custom-textarea-under-test";
const CUSTOM_PROBE_PLACEHOLDER_SELECTOR: &str = "custom-textarea-under-test-placeholder";

struct TextareaProbe {
    focus: FocusHandle,
    value: String,
    disabled: bool,
    invalid: bool,
    focus_visibility: FocusVisibility,
}

impl TextareaProbe {
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

impl Render for TextareaProbe {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let textarea = Textarea::new(
            "native-textarea",
            self.focus.clone(),
            ArtisanTheme::for_mode(ThemeMode::Light),
            self.value.clone(),
        )
        .placeholder("Type a message")
        .disabled(self.disabled)
        .invalid(self.invalid)
        .focus_visibility(self.focus_visibility)
        .semantic_label("Message")
        .debug_selector(TEXTAREA_SELECTOR);

        div().w(px(320.0)).child(textarea)
    }
}

#[test]
fn textarea_pure_line_helpers_cover_empty_and_multiline_cases() {
    assert_eq!(textarea_line_count(""), 0);
    assert_eq!(textarea_line_count("single line"), 1);
    assert_eq!(textarea_line_count("a\nb"), 2);
    assert_eq!(textarea_line_count("a\nb\n"), 3);
    assert_eq!(textarea_line_count("\n"), 2);
    assert_eq!(textarea_line_count("\n\n"), 3);
    assert!(!textarea_has_newline("hello"));
    assert!(textarea_has_newline("a\nb"));
    assert!(textarea_has_newline("\n"));
    assert!(!textarea_has_newline(""));
}

#[test]
fn style_resolves_audited_geometry_and_light_dark_surfaces() {
    let light = ArtisanTheme::for_mode(ThemeMode::Light);
    let dark = ArtisanTheme::for_mode(ThemeMode::Dark);

    let light_style = TextareaStyle::resolve(light, false);
    assert_eq!(light_style.min_height, light.spacing.steps(16.0));
    assert_eq!(light_style.horizontal_padding, light.spacing.steps(3.0));
    assert_eq!(light_style.vertical_padding, light.spacing.steps(3.0));
    assert_eq!(
        light_style.corner_radius,
        RadiusTokens::value(RadiusStep::Xl)
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
    assert!(!light_style.invalid);

    let dark_style = TextareaStyle::resolve(dark, false);
    assert_eq!(
        dark_style.background,
        dark.surfaces.value(SurfaceStep::S900).to_paint()
    );
    assert_eq!(dark_style.border, dark.colors.input.to_paint());
    assert_eq!(
        dark_style.placeholder_foreground,
        dark.colors.muted_foreground.to_paint()
    );
    assert_eq!(dark_style.min_height, px(64.0));
}

#[test]
fn invalid_style_uses_destructive_border_and_ring_for_each_mode() {
    let light = ArtisanTheme::for_mode(ThemeMode::Light);
    let dark = ArtisanTheme::for_mode(ThemeMode::Dark);

    let light_style = TextareaStyle::resolve(light, true);
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

    let dark_style = TextareaStyle::resolve(dark, true);
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

#[test]
fn textarea_flags_cover_all_branches() {
    let flags = TextareaFlags::default()
        .with(TextareaFlags::DISABLED, true)
        .with(TextareaFlags::INVALID, true)
        .with(TextareaFlags::HAS_VALUE, true)
        .with(TextareaFlags::SHOWS_PLACEHOLDER, false)
        .with(TextareaFlags::FOCUSABLE, true)
        .with(TextareaFlags::HAS_NEWLINE, true);

    assert!(flags.is_disabled());
    assert!(flags.is_invalid());
    assert!(flags.has_value());
    assert!(!flags.shows_placeholder());
    assert!(flags.is_focusable());
    assert!(flags.has_newline());

    let cleared = flags.with(TextareaFlags::HAS_NEWLINE, false);
    assert!(!cleared.has_newline());

    let disabled_off = flags.with(TextareaFlags::DISABLED, false);
    assert!(!disabled_off.is_disabled());
}

#[gpui::test]
fn semantics_keep_controlled_state_placeholder_and_newline_awareness(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, cx| TextareaProbe::new(cx));

    let semantics = cx.update(|_, app| {
        let probe = view.read(app);
        Textarea::new(
            "semantic-textarea",
            probe.focus.clone(),
            ArtisanTheme::for_mode(ThemeMode::Light),
            "first\nsecond",
        )
        .placeholder("Write something")
        .invalid(true)
        .semantic_label("Message body")
        .semantics()
    });

    assert!(semantics.flags.has_value());
    assert!(!semantics.flags.shows_placeholder());
    assert!(semantics.flags.is_invalid());
    assert!(!semantics.flags.is_disabled());
    assert!(semantics.flags.is_focusable());
    assert!(semantics.flags.has_newline());
    assert_eq!(semantics.line_count, 2);
    assert_eq!(
        semantics
            .semantic_label
            .as_ref()
            .map(gpui::SharedString::as_str),
        Some("Message body")
    );

    let empty = cx.update(|_, app| {
        let probe = view.read(app);
        Textarea::new(
            "empty-textarea",
            probe.focus.clone(),
            ArtisanTheme::for_mode(ThemeMode::Light),
            "",
        )
        .placeholder("Placeholder")
        .semantics()
    });
    assert!(!empty.flags.has_value());
    assert!(empty.flags.shows_placeholder());
    assert!(!empty.flags.has_newline());
    assert_eq!(empty.line_count, 0);

    let whitespace = cx.update(|_, app| {
        let probe = view.read(app);
        Textarea::new(
            "whitespace-textarea",
            probe.focus.clone(),
            ArtisanTheme::for_mode(ThemeMode::Light),
            "   ",
        )
        .placeholder("Placeholder")
        .semantics()
    });
    // Placeholder visibility keys off strictly empty, so whitespace hides placeholder.
    assert!(whitespace.flags.has_value());
    assert!(!whitespace.flags.shows_placeholder());
    assert_eq!(whitespace.line_count, 1);
}

#[gpui::test]
fn semantics_reflect_disabled_without_newline(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, cx| TextareaProbe::new(cx));
    let semantics = cx.update(|_, app| {
        let probe = view.read(app);
        Textarea::new(
            "disabled-textarea",
            probe.focus.clone(),
            ArtisanTheme::for_mode(ThemeMode::Light),
            "hello",
        )
        .disabled(true)
        .semantics()
    });

    assert!(semantics.flags.is_disabled());
    assert!(!semantics.flags.is_focusable());
    assert!(!semantics.flags.has_newline());
    assert_eq!(semantics.line_count, 1);
}

#[gpui::test]
fn from_state_uses_canonical_normalized_value_without_owning_state(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, cx| TextareaProbe::new(cx));
    let (value, line_count) = cx.update(|_, app| {
        let mut state = TextInputState::default();
        assert!(state.set_value("a\r\nb\u{200B}\nc"));

        let textarea = Textarea::from_state(
            "state-textarea",
            view.read(app).focus.clone(),
            ArtisanTheme::for_mode(ThemeMode::Light),
            &state,
        );
        (textarea.value().to_owned(), textarea.line_count())
    });

    assert_eq!(value, "a\nb\nc");
    assert_eq!(line_count, 3);
}

#[gpui::test]
fn value_and_placeholder_accessors_preserve_newlines_verbatim(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, cx| TextareaProbe::new(cx));
    cx.update(|_, app| {
        let probe = view.read(app);
        let textarea = Textarea::new(
            "verbatim-textarea",
            probe.focus.clone(),
            ArtisanTheme::for_mode(ThemeMode::Light),
            "line one\nline two\n",
        )
        .placeholder("Placeholder\nwith newline");

        assert_eq!(textarea.value(), "line one\nline two\n");
        assert_eq!(textarea.line_count(), 3);
        assert!(textarea.has_newline());
        assert_eq!(
            textarea.placeholder_value(),
            Some("Placeholder\nwith newline")
        );

        let without = textarea.without_placeholder();
        assert_eq!(without.placeholder_value(), None);

        let default = Textarea::new(
            "default-selector-textarea",
            probe.focus.clone(),
            ArtisanTheme::for_mode(ThemeMode::Light),
            "",
        );
        assert_eq!(default.placeholder_value(), None);
        assert_eq!(default.value(), "");
        assert_eq!(
            default.visual_style(),
            TextareaStyle::from(ArtisanTheme::for_mode(ThemeMode::Light))
        );
        assert_eq!(default.visual_style().corner_radius, px(14.0));
        assert_eq!(DEFAULT_DEBUG_SELECTOR, "artisan-textarea");
    });
}

#[gpui::test]
fn render_probe_paints_min_height_and_value_branch(cx: &mut TestAppContext) {
    let (_view, cx) = cx.add_window_view(|_, cx| TextareaProbe::new(cx));
    let bounds = cx
        .debug_bounds(TEXTAREA_SELECTOR)
        .expect("textarea must expose inspectable root bounds");

    assert_eq!(bounds.size.width, px(320.0));
    assert!(f32::from(bounds.size.height) >= 64.0);
    assert!(cx.debug_bounds(VALUE_SELECTOR).is_some());
    assert!(cx.debug_bounds(PLACEHOLDER_SELECTOR).is_none());
}

#[gpui::test]
fn multiline_value_is_preserved_in_rendered_branch(cx: &mut TestAppContext) {
    let (_view, cx) = cx.add_window_view(|_, cx| {
        let mut probe = TextareaProbe::new(cx);
        probe.value = "first\nsecond\nthird".to_owned();
        probe
    });

    assert!(cx.debug_bounds(VALUE_SELECTOR).is_some());
    assert!(cx.debug_bounds(PLACEHOLDER_SELECTOR).is_none());
}

#[gpui::test]
fn focus_ring_requires_actual_focus_and_visibility_intent(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, cx| TextareaProbe::new(cx));

    cx.update(|window, app| {
        let focus = view.read(app).focus.clone();
        window.focus(&focus);
    });
    cx.run_until_parked();

    cx.update(|window, app| {
        let focus = view.read(app).focus.clone();
        let visible = Textarea::new(
            "focused-textarea",
            focus.clone(),
            ArtisanTheme::for_mode(ThemeMode::Light),
            "value\nwith newline",
        )
        .focus_visibility(FocusVisibility::Visible);
        assert!(visible.focus_ring_visible(window));

        let hidden = Textarea::new(
            "hidden-focus-textarea",
            focus,
            ArtisanTheme::for_mode(ThemeMode::Light),
            "value",
        )
        .focus_visibility(FocusVisibility::Hidden);
        assert!(!hidden.focus_ring_visible(window));
    });
}

#[gpui::test]
fn disabled_textarea_remains_visible_but_does_not_report_focus_ring(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, cx| {
        let mut probe = TextareaProbe::new(cx);
        probe.disabled = true;
        probe
    });

    let bounds = cx
        .debug_bounds(TEXTAREA_SELECTOR)
        .expect("disabled textarea must remain visible");
    assert!(f32::from(bounds.size.height) >= 64.0);

    cx.update(|window, app| {
        let focus = view.read(app).focus.clone();
        window.focus(&focus);

        let textarea = Textarea::new(
            "disabled-textarea",
            focus,
            ArtisanTheme::for_mode(ThemeMode::Light),
            "value\nmultiline",
        )
        .disabled(true)
        .focus_visibility(FocusVisibility::Visible);

        assert!(!textarea.focus_ring_visible(window));
        assert!(!textarea.semantics().flags.is_focusable());
    });
}

#[gpui::test]
fn placeholder_branch_is_inspectable_when_value_is_empty(cx: &mut TestAppContext) {
    let (_view, cx) = cx.add_window_view(|_, cx| {
        let mut probe = TextareaProbe::new(cx);
        probe.value.clear();
        probe
    });

    assert!(cx.debug_bounds(PLACEHOLDER_SELECTOR).is_some());
    assert!(cx.debug_bounds(VALUE_SELECTOR).is_none());
}

#[gpui::test]
fn styled_refinement_and_debug_selector_alias_work(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, cx| TextareaProbe::new(cx));
    cx.update(|_, app| {
        let probe = view.read(app);
        let mut textarea = Textarea::new(
            "refined-textarea",
            probe.focus.clone(),
            ArtisanTheme::for_mode(ThemeMode::Dark),
            "hello\nworld",
        )
        .accessible_label("Accessible message")
        .debug_selector("custom-textarea");

        // Styled refinement later wins.
        textarea.style().refine(&gpui::StyleRefinement::default());

        assert_eq!(textarea.value(), "hello\nworld");
        assert_eq!(textarea.line_count(), 2);
        assert!(textarea.has_newline());
        assert!(!textarea.is_disabled());
        assert!(!textarea.is_invalid());
        assert_eq!(
            textarea
                .semantics()
                .semantic_label
                .as_ref()
                .map(gpui::SharedString::as_str),
            Some("Accessible message")
        );
        // Verify the debug selector was applied by rendering via probe with custom selector.
    });

    // Render a textarea with explicit custom selector to verify bounds.
    let (_view2, cx2) = cx.add_window_view(|_, cx| CustomProbe::new(cx));
    assert!(cx2.debug_bounds(CUSTOM_PROBE_SELECTOR).is_some());
    assert!(cx2.debug_bounds(CUSTOM_PROBE_PLACEHOLDER_SELECTOR).is_some());
}

struct CustomProbe {
    focus: FocusHandle,
}

impl CustomProbe {
    fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus: cx.focus_handle(),
        }
    }
}

impl Render for CustomProbe {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        Textarea::new(
            "custom-id",
            self.focus.clone(),
            ArtisanTheme::for_mode(ThemeMode::Light),
            "",
        )
        .placeholder("hint")
        .debug_selector(CUSTOM_PROBE_SELECTOR)
    }
}
