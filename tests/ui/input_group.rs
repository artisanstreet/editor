//! Behavioral coverage for the native GPUI input-group composition.

use artisan_ui::button::FocusVisibility;
use artisan_ui::input_group::{
    DEFAULT_DEBUG_SELECTOR, InputGroup, InputGroupAddonAlign, InputGroupFlags, InputGroupStyle,
};
use artisan_ui::theme::{ArtisanTheme, RadiusStep, RadiusTokens, SurfaceStep, ThemeMode};
use gpui::{
    Context, FocusHandle, IntoElement, ParentElement, Render, Styled, TestAppContext, div, px,
};

const GROUP_SELECTOR: &str = "native-input-group-under-test";
const ALT_GROUP_SELECTOR: &str = "native-input-group-alt";

fn leaked_selector(value: String) -> &'static str {
    Box::leak(value.into_boxed_str())
}

fn control_element(label: &str) -> impl IntoElement {
    div().flex().flex_1().min_w(px(0.0)).child(label.to_owned())
}

struct GroupProbe {
    focus: FocusHandle,
    disabled: bool,
    invalid: bool,
    focus_visibility: FocusVisibility,
    block: bool,
}

impl GroupProbe {
    fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus: cx.focus_handle(),
            disabled: false,
            invalid: false,
            focus_visibility: FocusVisibility::Visible,
            block: false,
        }
    }
}

impl Render for GroupProbe {
    fn render(&mut self, _window: &mut gpui::Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let theme = ArtisanTheme::for_mode(ThemeMode::Light);
        let mut group = InputGroup::new(
            "probe-group",
            self.focus.clone(),
            theme,
            control_element("value"),
        )
        .leading_addon("search", div().child("leading"))
        .trailing_addon("clear", div().child("trailing"))
        .disabled(self.disabled)
        .invalid(self.invalid)
        .focus_visibility(self.focus_visibility)
        .semantic_label("Search field")
        .debug_selector(GROUP_SELECTOR);

        if self.block {
            group = group.block_start("filters", div().child("filters"));
        }

        div().w(px(360.0)).child(group)
    }
}

struct AltProbe {
    focus: FocusHandle,
}

impl Render for AltProbe {
    fn render(&mut self, _window: &mut gpui::Window, _cx: &mut Context<Self>) -> impl IntoElement {
        // Directly check selector isolation via bounds presence.
        div().w(px(360.0)).child(
            InputGroup::new(
                "alt-group-inner",
                self.focus.clone(),
                ArtisanTheme::for_mode(ThemeMode::Light),
                control_element("alt"),
            )
            .debug_selector(ALT_GROUP_SELECTOR),
        )
    }
}

#[test]
fn style_resolves_audited_geometry_and_light_dark_surfaces() {
    let light = ArtisanTheme::for_mode(ThemeMode::Light);
    let dark = ArtisanTheme::for_mode(ThemeMode::Dark);

    let light_inline = InputGroupStyle::resolve(light, false, false);
    assert_eq!(light_inline.height, Some(light.density.control_default));
    assert_eq!(
        light_inline.corner_radius,
        RadiusTokens::value(RadiusStep::X4l)
    );
    assert_eq!(light_inline.border_width, px(1.0));
    assert_eq!(
        light_inline.background,
        light.surfaces.value(SurfaceStep::S100).to_paint()
    );
    assert_eq!(light_inline.border, light.colors.input.to_paint());
    assert_eq!(light_inline.focus_border, light.colors.ring.to_paint());
    assert_eq!(
        light_inline.focus_ring,
        light.interaction.focus_ring_color.to_paint()
    );
    assert_eq!(
        light_inline.focus_ring_width,
        light.interaction.focus_ring_width
    );
    assert_eq!(
        light_inline.muted_foreground,
        light.colors.muted_foreground.to_paint()
    );
    assert!(!light_inline.invalid);
    assert!(!light_inline.has_block);
    assert_eq!(light_inline.disabled_opacity.to_bits(), 0.5_f32.to_bits());

    let light_block = InputGroupStyle::resolve(light, false, true);
    assert_eq!(light_block.height, None);
    assert_eq!(
        light_block.corner_radius,
        RadiusTokens::value(RadiusStep::X2l)
    );
    assert!(light_block.has_block);

    let dark_inline = InputGroupStyle::resolve(dark, false, false);
    assert_eq!(
        dark_inline.background,
        dark.surfaces.value(SurfaceStep::S900).to_paint()
    );
    assert_eq!(dark_inline.border, dark.colors.input.to_paint());
}

#[test]
fn invalid_style_uses_destructive_border_and_ring_for_each_mode() {
    let light = ArtisanTheme::for_mode(ThemeMode::Light);
    let dark = ArtisanTheme::for_mode(ThemeMode::Dark);

    let light_invalid = InputGroupStyle::resolve(light, true, false);
    assert!(light_invalid.invalid);
    assert_eq!(light_invalid.border, light.colors.destructive.to_paint());
    assert_eq!(
        light_invalid.focus_border,
        light.colors.destructive.to_paint()
    );
    assert_eq!(
        light_invalid.focus_ring,
        light.interaction.invalid_ring_color.to_paint()
    );

    let dark_invalid = InputGroupStyle::resolve(dark, true, true);
    assert!(dark_invalid.invalid);
    assert!(dark_invalid.has_block);
    assert_eq!(
        dark_invalid.border,
        dark.colors.destructive.with_alpha(0.5).to_paint()
    );
    assert_eq!(
        dark_invalid.focus_border,
        dark.colors.destructive.with_alpha(0.5).to_paint()
    );
    assert_eq!(
        dark_invalid.focus_ring,
        dark.colors.destructive.with_alpha(0.4).to_paint()
    );
}

#[test]
fn addon_align_tokens_are_stable_and_cover_variants() {
    assert_eq!(InputGroupAddonAlign::InlineStart.as_str(), "inline-start");
    assert_eq!(InputGroupAddonAlign::InlineEnd.as_str(), "inline-end");
    assert_eq!(InputGroupAddonAlign::BlockStart.as_str(), "block-start");
    assert_eq!(InputGroupAddonAlign::BlockEnd.as_str(), "block-end");

    assert!(InputGroupAddonAlign::InlineStart.is_inline());
    assert!(InputGroupAddonAlign::InlineEnd.is_inline());
    assert!(!InputGroupAddonAlign::InlineStart.is_block());
    assert!(InputGroupAddonAlign::BlockStart.is_block());
    assert!(InputGroupAddonAlign::BlockEnd.is_block());
    assert!(InputGroupAddonAlign::BlockStart.is_leading());
    assert!(!InputGroupAddonAlign::BlockEnd.is_leading());
}

#[test]
fn flags_cover_disabled_error_and_layout_state() {
    let flags = InputGroupFlags::default()
        .with(InputGroupFlags::DISABLED, true)
        .with(InputGroupFlags::INVALID, true)
        .with(InputGroupFlags::HAS_BLOCK, true)
        .with(InputGroupFlags::HAS_INLINE, true)
        .with(InputGroupFlags::FOCUSABLE, true);
    assert!(flags.is_disabled());
    assert!(flags.is_invalid());
    assert!(flags.has_block());
    assert!(flags.has_inline());
    assert!(flags.is_focusable());
    assert!(flags.contains(InputGroupFlags::DISABLED));
    assert!(!InputGroupFlags::default().is_disabled());
}

#[gpui::test]
fn deterministic_slot_ordering_is_independent_of_insertion_order(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, cx| GroupProbe::new(cx));
    let theme = ArtisanTheme::for_mode(ThemeMode::Light);
    let focus = cx.update(|_, app| view.read(app).focus.clone());

    // Build groups with addons registered in opposite orders and compare render order.
    let build = |order: &[InputGroupAddonAlign]| {
        let mut group = InputGroup::new(
            "order-group",
            focus.clone(),
            theme,
            control_element("value"),
        );
        for align in order {
            group = group.addon(*align, format!("{align:?}"), div().child(align.as_str()));
        }
        group.ordered_addon_aligns()
    };

    let order_a = [
        InputGroupAddonAlign::BlockEnd,
        InputGroupAddonAlign::InlineEnd,
        InputGroupAddonAlign::InlineStart,
        InputGroupAddonAlign::BlockStart,
    ];
    let order_b = [
        InputGroupAddonAlign::InlineStart,
        InputGroupAddonAlign::BlockStart,
        InputGroupAddonAlign::BlockEnd,
        InputGroupAddonAlign::InlineEnd,
    ];

    let expected = vec![
        InputGroupAddonAlign::BlockStart,
        InputGroupAddonAlign::InlineStart,
        InputGroupAddonAlign::InlineEnd,
        InputGroupAddonAlign::BlockEnd,
    ];
    assert_eq!(build(&order_a), expected);
    assert_eq!(build(&order_b), expected);

    // Insertion order is preserved in the raw view, but render order is sorted.
    let group = InputGroup::new(
        "insertion-group",
        focus.clone(),
        theme,
        control_element("value"),
    )
    .addon(InputGroupAddonAlign::BlockEnd, "end", div().child("end"))
    .addon(
        InputGroupAddonAlign::InlineStart,
        "start",
        div().child("start"),
    );
    assert_eq!(
        group.addon_aligns(),
        vec![
            InputGroupAddonAlign::BlockEnd,
            InputGroupAddonAlign::InlineStart
        ]
    );
    assert_eq!(
        group.ordered_addon_aligns(),
        vec![
            InputGroupAddonAlign::InlineStart,
            InputGroupAddonAlign::BlockEnd
        ]
    );
    assert!(group.has_block());
    assert!(group.has_inline());
    assert_eq!(group.addon_count(), 2);
}

#[gpui::test]
fn semantics_keep_accessibility_and_layout_flags_explicit(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, cx| GroupProbe::new(cx));
    let theme = ArtisanTheme::for_mode(ThemeMode::Light);
    let focus = cx.update(|_, app| view.read(app).focus.clone());
    let group = InputGroup::new(
        "semantic-group",
        focus.clone(),
        theme,
        control_element("value"),
    )
    .leading_addon("icon", div().child("icon"))
    .block_end("help", div().child("help"))
    .invalid(true)
    .semantic_label("Search")
    .debug_selector("semantic-group");

    let semantics = group.semantics();
    assert!(semantics.flags.is_invalid());
    assert!(semantics.flags.has_block());
    assert!(semantics.flags.has_inline());
    assert!(semantics.flags.is_focusable());
    assert!(!semantics.flags.is_disabled());
    assert_eq!(
        semantics
            .semantic_label
            .as_ref()
            .map(gpui::SharedString::as_str),
        Some("Search")
    );

    let disabled = InputGroup::new(
        "disabled-semantic",
        focus.clone(),
        theme,
        control_element("value"),
    )
    .disabled(true)
    .semantics();
    assert!(disabled.flags.is_disabled());
    assert!(!disabled.flags.is_focusable());
}

#[gpui::test]
fn accessible_label_alias_retains_semantic_label(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, cx| GroupProbe::new(cx));
    let theme = ArtisanTheme::for_mode(ThemeMode::Light);
    let focus = cx.update(|_, app| view.read(app).focus.clone());
    let group = InputGroup::new("alias-group", focus, theme, control_element("value"))
        .accessible_label("Alias")
        .debug_selector("alias-group");
    assert_eq!(
        group
            .semantics()
            .semantic_label
            .as_ref()
            .map(gpui::SharedString::as_str),
        Some("Alias")
    );
}

#[gpui::test]
fn visual_style_reflects_invalid_and_block_state(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, cx| GroupProbe::new(cx));
    let theme = ArtisanTheme::for_mode(ThemeMode::Light);
    let focus = cx.update(|_, app| view.read(app).focus.clone());
    let inline = InputGroup::new(
        "inline-style",
        focus.clone(),
        theme,
        control_element("value"),
    )
    .visual_style();
    assert_eq!(inline.corner_radius, RadiusTokens::value(RadiusStep::X4l));
    assert!(!inline.has_block);

    let block = InputGroup::new(
        "block-style",
        focus.clone(),
        theme,
        control_element("value"),
    )
    .block_start("top", div().child("top"))
    .visual_style();
    assert_eq!(block.corner_radius, RadiusTokens::value(RadiusStep::X2l));
    assert!(block.has_block);
    assert_eq!(block.height, None);
}

#[gpui::test]
fn render_probe_paints_group_and_control_and_respects_inline_order(cx: &mut TestAppContext) {
    let (_view, cx) = cx.add_window_view(|_, cx| GroupProbe::new(cx));
    let group_bounds = cx
        .debug_bounds(GROUP_SELECTOR)
        .expect("group must expose inspectable root bounds");
    assert_eq!(group_bounds.size.width, px(360.0));
    assert_eq!(group_bounds.size.height, px(36.0));

    let control = leaked_selector(format!("{GROUP_SELECTOR}-control-control"));
    let leading = leaked_selector(format!("{GROUP_SELECTOR}-addon-inline-start-search"));
    let trailing = leaked_selector(format!("{GROUP_SELECTOR}-addon-inline-end-clear"));
    assert!(cx.debug_bounds(control).is_some());
    assert!(cx.debug_bounds(leading).is_some());
    assert!(cx.debug_bounds(trailing).is_some());

    let leading_bounds = cx.debug_bounds(leading).expect("leading addon bounds");
    let control_bounds = cx.debug_bounds(control).expect("control bounds");
    let trailing_bounds = cx.debug_bounds(trailing).expect("trailing addon bounds");
    assert!(leading_bounds.origin.x <= control_bounds.origin.x);
    assert!(control_bounds.origin.x <= trailing_bounds.origin.x);
}

#[gpui::test]
fn block_align_stacks_vertically_and_uses_block_radius(cx: &mut TestAppContext) {
    let (_view, cx) = cx.add_window_view(|_, cx| {
        let mut probe = GroupProbe::new(cx);
        probe.block = true;
        probe
    });

    let group_bounds = cx
        .debug_bounds(GROUP_SELECTOR)
        .expect("block group must still expose root bounds");
    assert!(group_bounds.size.height > px(36.0));

    let block = leaked_selector(format!("{GROUP_SELECTOR}-addon-block-start-filters"));
    let control = leaked_selector(format!("{GROUP_SELECTOR}-control-control"));
    let block_bounds = cx.debug_bounds(block).expect("block addon bounds");
    let control_bounds = cx.debug_bounds(control).expect("control bounds");
    assert!(block_bounds.origin.y <= control_bounds.origin.y);
}

#[gpui::test]
fn focus_ring_requires_actual_focus_and_visibility_intent(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, cx| GroupProbe::new(cx));

    cx.update(|window, app| {
        let focus = view.read(app).focus.clone();
        window.focus(&focus, app);
    });
    cx.run_until_parked();

    cx.update(|window, app| {
        let focus = view.read(app).focus.clone();
        let visible = InputGroup::new(
            "focused-group",
            focus.clone(),
            ArtisanTheme::for_mode(ThemeMode::Light),
            control_element("value"),
        )
        .focus_visibility(FocusVisibility::Visible);
        assert!(visible.focus_ring_visible(window));

        let hidden = InputGroup::new(
            "hidden-focus-group",
            focus,
            ArtisanTheme::for_mode(ThemeMode::Light),
            control_element("value"),
        )
        .focus_visibility(FocusVisibility::Hidden);
        assert!(!hidden.focus_ring_visible(window));
    });
}

#[gpui::test]
fn disabled_group_remains_visible_but_does_not_report_focus_ring(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, cx| {
        let mut probe = GroupProbe::new(cx);
        probe.disabled = true;
        probe
    });

    let bounds = cx
        .debug_bounds(GROUP_SELECTOR)
        .expect("disabled group must remain visible");
    assert_eq!(bounds.size.width, px(360.0));

    cx.update(|window, app| {
        let focus = view.read(app).focus.clone();
        window.focus(&focus, app);

        let group = InputGroup::new(
            "disabled-group",
            focus.clone(),
            ArtisanTheme::for_mode(ThemeMode::Light),
            control_element("value"),
        )
        .disabled(true)
        .focus_visibility(FocusVisibility::Visible);

        assert!(!group.focus_ring_visible(window));
        assert!(!group.semantics().flags.is_focusable());
        assert!(group.is_disabled());
    });
}

#[gpui::test]
fn invalid_group_exposes_error_semantics_and_stable_identity(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, cx| {
        let mut probe = GroupProbe::new(cx);
        probe.invalid = true;
        probe
    });

    // Root and control selectors remain stable under invalid state.
    assert!(cx.debug_bounds(GROUP_SELECTOR).is_some());
    assert!(
        cx.debug_bounds(leaked_selector(format!("{GROUP_SELECTOR}-control-control")))
            .is_some()
    );

    cx.update(|_, app| {
        let probe = view.read(app);
        let group = InputGroup::new(
            "invalid-group",
            probe.focus.clone(),
            ArtisanTheme::for_mode(ThemeMode::Light),
            control_element("value"),
        )
        .invalid(true)
        .debug_selector(GROUP_SELECTOR);
        assert!(group.is_invalid());
        assert!(group.semantics().flags.is_invalid());
        assert!(group.visual_style().invalid);
    });
}

#[gpui::test]
fn identity_selectors_and_semantic_labels_are_stable_across_renders(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, cx| GroupProbe::new(cx));

    let first = cx
        .debug_bounds(GROUP_SELECTOR)
        .expect("first render bounds");
    let leading = leaked_selector(format!("{GROUP_SELECTOR}-addon-inline-start-search"));
    let first_leading = cx.debug_bounds(leading).expect("leading bounds");

    cx.update(|_, app| {
        view.update(app, |_, cx| cx.notify());
    });
    cx.run_until_parked();

    assert_eq!(cx.debug_bounds(GROUP_SELECTOR), Some(first));
    assert_eq!(cx.debug_bounds(leading), Some(first_leading));

    // Distinct selector namespaces do not collide.
    let (_alt_view, alt_cx) = cx.add_window_view(|_, cx| AltProbe {
        focus: cx.focus_handle(),
    });
    // Both selectors coexist; the alt control is namespaced.
    assert!(
        alt_cx
            .debug_bounds(leaked_selector(format!(
                "{ALT_GROUP_SELECTOR}-control-control"
            )))
            .is_some()
    );
    assert!(alt_cx.debug_bounds(leading).is_none());
    let alt_leading = leaked_selector(format!("{ALT_GROUP_SELECTOR}-addon-inline-start-search"));
    assert!(alt_cx.debug_bounds(alt_leading).is_none());

    // Default selector is used when none is supplied.
    let default_group = InputGroup::new(
        "default-selector-group",
        view.read_with(cx, |probe, _| probe.focus.clone()),
        ArtisanTheme::for_mode(ThemeMode::Light),
        control_element("value"),
    );
    assert_eq!(
        default_group.visual_style().corner_radius,
        RadiusTokens::value(RadiusStep::X4l)
    );
    // Verify default debug selector constant is the documented value.
    assert_eq!(DEFAULT_DEBUG_SELECTOR, "artisan-input-group");
}
