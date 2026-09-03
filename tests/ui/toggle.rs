#![allow(clippy::float_cmp)]

use std::cell::Cell;
use std::rc::Rc;

use artisan_ui::AssetId;
use artisan_ui::button::{AccessibleLabel, FocusVisibility};
use artisan_ui::theme::{ArtisanTheme, ThemeMode};
use artisan_ui::toggle::{
    Toggle, ToggleConstructionError, ToggleContent, ToggleSize, ToggleStyle, ToggleVariant,
    next_pressed, toggled,
};
use gpui::{
    Context, FocusHandle, IntoElement, KeyUpEvent, Keystroke, Modifiers, ParentElement, Render,
    Styled, TestAppContext, Window, div, px, transparent_black,
};

const TOGGLE_SELECTOR: &str = "native-toggle-under-test";

struct ToggleProbe {
    focus: FocusHandle,
    pressed: bool,
    disabled: bool,
    variant: ToggleVariant,
    size: ToggleSize,
    content: ToggleContent,
    activations: Rc<Cell<u32>>,
    next: Rc<Cell<Option<bool>>>,
    selector: Option<String>,
    refined: bool,
}

impl ToggleProbe {
    fn new(
        cx: &mut Context<Self>,
        pressed: bool,
        disabled: bool,
        variant: ToggleVariant,
        size: ToggleSize,
        content: ToggleContent,
        refined: bool,
    ) -> Self {
        Self {
            focus: cx.focus_handle(),
            pressed,
            disabled,
            variant,
            size,
            content,
            activations: Rc::new(Cell::new(0)),
            next: Rc::new(Cell::new(None)),
            selector: Some(TOGGLE_SELECTOR.to_owned()),
            refined,
        }
    }

    fn activations(&self) -> u32 {
        self.activations.get()
    }

    fn next_value(&self) -> Option<bool> {
        self.next.get()
    }
}

impl Render for ToggleProbe {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let theme = ArtisanTheme::for_mode(ThemeMode::Light);
        let activations = self.activations.clone();
        let next = self.next.clone();
        let toggle = Toggle::new(
            "native-toggle",
            self.focus.clone(),
            theme,
            self.variant,
            self.size,
            self.content.clone(),
            self.pressed,
        )
        .expect("valid toggle content")
        .disabled(self.disabled)
        .focus_visibility(FocusVisibility::Visible)
        .debug_selector(self.selector.clone().expect("selector must be set"))
        .on_pressed_change(move |next_pressed, _, _, _| {
            activations.set(activations.get() + 1);
            next.set(Some(next_pressed));
        });

        let toggle = if self.refined {
            toggle
                .w(px(200.0))
                .h(px(50.0))
                .bg(theme.colors.accent.to_paint())
        } else {
            toggle
        };

        div().size_full().p(px(20.0)).child(toggle)
    }
}

#[test]
fn pure_toggle_policy_is_deterministic_and_disabled_guarded() {
    assert!(next_pressed(false, false));
    assert!(!next_pressed(true, false));
    assert!(!next_pressed(false, true));
    assert!(next_pressed(true, true));

    assert!(toggled(false));
    assert!(!toggled(true));

    // Determinism: repeated calls produce identical results.
    let first = next_pressed(false, false);
    let second = next_pressed(false, false);
    assert_eq!(first, second);
    assert!(!toggled(first));
}

#[test]
fn style_resolves_variant_size_pressed_and_theme_tokens() {
    let light = ArtisanTheme::for_mode(ThemeMode::Light);
    let dark = ArtisanTheme::for_mode(ThemeMode::Dark);

    let default_unpressed =
        ToggleStyle::resolve(light, ToggleVariant::Default, ToggleSize::Default, false);
    assert_eq!(default_unpressed.height, px(36.0));
    assert_eq!(default_unpressed.min_width, px(36.0));
    assert_eq!(default_unpressed.horizontal_padding, px(12.0));
    assert_eq!(default_unpressed.background, transparent_black());
    assert_eq!(default_unpressed.border, transparent_black());
    assert_eq!(
        default_unpressed.pressed_background,
        light.colors.muted.to_paint()
    );
    assert_eq!(
        default_unpressed.foreground,
        light.colors.foreground.to_paint()
    );
    assert_eq!(default_unpressed.focus_ring_width, px(3.0));
    assert_eq!(default_unpressed.corner_radius, px(26.0));
    assert_eq!(default_unpressed.content_gap, px(4.0));
    assert_eq!(default_unpressed.icon_size, px(16.0));
    assert_eq!(default_unpressed.disabled_opacity, 0.5);
    assert_eq!(default_unpressed.text_size, light.typography.control_text);

    let default_pressed =
        ToggleStyle::resolve(light, ToggleVariant::Default, ToggleSize::Default, true);
    assert_eq!(default_pressed.background, light.colors.muted.to_paint());

    let outline = ToggleStyle::resolve(light, ToggleVariant::Outline, ToggleSize::Default, false);
    assert_eq!(outline.border, light.colors.input.to_paint());

    let small = ToggleStyle::resolve(light, ToggleVariant::Default, ToggleSize::Small, false);
    assert_eq!(small.height, px(32.0));
    assert_eq!(small.min_width, px(32.0));

    let large = ToggleStyle::resolve(light, ToggleVariant::Default, ToggleSize::Large, false);
    assert_eq!(large.height, px(40.0));
    assert_eq!(large.min_width, px(40.0));
    assert_eq!(large.horizontal_padding, px(16.0));

    let dark_hover = ToggleStyle::resolve(dark, ToggleVariant::Default, ToggleSize::Default, false);
    assert_eq!(
        dark_hover.hover_background,
        dark.colors.muted.with_alpha(0.5).to_paint()
    );
    assert_eq!(dark_hover.focus_border, dark.colors.ring.to_paint());
    assert_eq!(
        dark_hover.hover_foreground,
        dark.colors.foreground.to_paint()
    );
}

#[test]
fn accessible_label_is_retained_for_all_content_variants() {
    let label = AccessibleLabel::new("Toggle formatting").expect("non-empty label");
    let icon_only = ToggleContent::icon_only(AssetId::TABLER_CHECK, label.clone());
    assert_eq!(icon_only.accessible_label(), "Toggle formatting");

    let text = ToggleContent::text("Bold");
    assert_eq!(text.accessible_label(), "Bold");

    let icon_text = ToggleContent::icon_text(AssetId::TABLER_CHECK, "Bold");
    assert_eq!(icon_text.accessible_label(), "Bold");
}

#[gpui::test]
fn toggle_construction_rejects_blank_visible_labels(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, cx| {
        ToggleProbe::new(
            cx,
            false,
            false,
            ToggleVariant::Default,
            ToggleSize::Default,
            ToggleContent::text("Bold"),
            false,
        )
    });
    let errors = cx.update(|_, app| {
        let focus = view.read(app).focus.clone();
        let theme = ArtisanTheme::for_mode(ThemeMode::Light);
        let blank_text = ToggleContent::text("   ");
        let err = Toggle::new(
            "blank",
            focus.clone(),
            theme,
            ToggleVariant::Default,
            ToggleSize::Default,
            blank_text,
            false,
        )
        .err();

        let blank_icon_text = ToggleContent::icon_text(AssetId::TABLER_CHECK, "  ");
        let err2 = Toggle::new(
            "blank2",
            focus,
            theme,
            ToggleVariant::Default,
            ToggleSize::Default,
            blank_icon_text,
            false,
        )
        .err();
        (err, err2)
    });
    assert_eq!(
        errors,
        (
            Some(ToggleConstructionError::EmptyVisibleLabel),
            Some(ToggleConstructionError::EmptyVisibleLabel),
        )
    );
}

#[gpui::test]
fn pressed_state_is_controlled_and_pointer_emits_next_value(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, cx| {
        ToggleProbe::new(
            cx,
            false,
            false,
            ToggleVariant::Default,
            ToggleSize::Default,
            ToggleContent::text("Bold"),
            false,
        )
    });

    cx.update(|_, app| assert!(!view.read(app).pressed));
    let bounds = cx
        .debug_bounds(TOGGLE_SELECTOR)
        .expect("toggle must expose debug bounds");
    cx.simulate_click(bounds.center(), Modifiers::none());

    cx.update(|_, app| {
        let probe = view.read(app);
        // Controlled: view's pressed has not changed until caller applies next.
        assert!(!probe.pressed);
        assert_eq!(probe.activations(), 1);
        assert_eq!(probe.next_value(), Some(true));
    });

    // Caller applies the next value;Toggle now renders pressed.
    cx.update(|_, app| {
        view.update(app, |probe, cx| {
            probe.pressed = probe.next_value().expect("next must be set");
            cx.notify();
        });
    });
    cx.run_until_parked();
    cx.update(|_, app| assert!(view.read(app).pressed));
    let pressed_bounds = cx
        .debug_bounds(TOGGLE_SELECTOR)
        .expect("pressed toggle must remain visible");
    assert_eq!(pressed_bounds.size.height, px(36.0));
}

#[gpui::test]
fn enter_and_space_activate_controlled_next_values(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|window, cx| {
        let probe = ToggleProbe::new(
            cx,
            false,
            false,
            ToggleVariant::Default,
            ToggleSize::Default,
            ToggleContent::text("Italic"),
            false,
        );
        window.focus(&probe.focus);
        probe
    });
    cx.run_until_parked();

    cx.simulate_event(KeyUpEvent {
        keystroke: Keystroke::parse("enter").expect("enter must parse"),
    });
    cx.run_until_parked();
    cx.update(|_, app| {
        assert_eq!(view.read(app).activations(), 1);
        assert_eq!(view.read(app).next_value(), Some(true));
    });

    let next = cx.update(|_, app| view.read(app).next_value().expect("next must be set"));
    cx.update(|_, app| {
        view.update(app, |probe, cx| {
            probe.pressed = next;
            cx.notify();
        });
    });
    cx.run_until_parked();

    cx.simulate_event(KeyUpEvent {
        keystroke: Keystroke::parse("space").expect("space must parse"),
    });
    cx.run_until_parked();
    cx.update(|window, app| {
        let probe = view.read(app);
        assert!(probe.focus.is_focused(window));
        assert_eq!(probe.activations(), 2);
        assert_eq!(probe.next_value(), Some(false));
    });
}

#[gpui::test]
fn disabled_toggle_is_half_opaque_and_suppresses_activation(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, cx| {
        ToggleProbe::new(
            cx,
            false,
            true,
            ToggleVariant::Outline,
            ToggleSize::Default,
            ToggleContent::text("Underline"),
            false,
        )
    });

    let bounds = cx
        .debug_bounds(TOGGLE_SELECTOR)
        .expect("disabled toggle must remain visible");
    cx.simulate_click(bounds.center(), Modifiers::none());
    cx.update(|window, app| {
        let focus = view.read(app).focus.clone();
        assert!(!focus.is_focused(window));
        window.focus(&focus);
        view.update(app, |_, cx| cx.notify());
    });
    cx.run_until_parked();

    for key in ["enter", "space"] {
        cx.simulate_event(KeyUpEvent {
            keystroke: Keystroke::parse(key).expect("activation key must parse"),
        });
    }
    cx.update(|_, app| {
        assert_eq!(view.read(app).activations(), 0);
        assert_eq!(view.read(app).next_value(), None);
    });
    assert_eq!(
        ToggleStyle::resolve(
            ArtisanTheme::for_mode(ThemeMode::Light),
            ToggleVariant::Default,
            ToggleSize::Default,
            false
        )
        .disabled_opacity,
        0.5
    );
}

#[gpui::test]
fn focus_ring_requires_actual_focus_and_visible_intent(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, cx| {
        ToggleProbe::new(
            cx,
            false,
            false,
            ToggleVariant::Default,
            ToggleSize::Default,
            ToggleContent::text("Code"),
            false,
        )
    });

    // Without focus, ring is not visible even though probe sets Visible.
    let is_ring_visible = |window: &Window, app: &gpui::App| {
        let theme = ArtisanTheme::for_mode(ThemeMode::Light);
        let toggle = Toggle::new(
            "probe",
            view.read(app).focus.clone(),
            theme,
            ToggleVariant::Default,
            ToggleSize::Default,
            ToggleContent::text("Code"),
            false,
        )
        .expect("valid")
        .focus_visibility(FocusVisibility::Visible);
        toggle.focus_ring_visible(window)
    };

    cx.update(|window, app| {
        assert!(!is_ring_visible(window, app));
        window.focus(&view.read(app).focus);
    });
    cx.run_until_parked();
    cx.update(|window, app| {
        assert!(is_ring_visible(window, app));
    });

    // Disabled toggles never show a focus ring even when focused.
    cx.update(|_, app| {
        view.update(app, |probe, cx| {
            probe.disabled = true;
            cx.notify();
        });
    });
    cx.run_until_parked();
    cx.update(|window, app| {
        // Build a disabled toggle and check focus_ring_visible directly.
        let theme = ArtisanTheme::for_mode(ThemeMode::Light);
        let toggle = Toggle::new(
            "probe2",
            view.read(app).focus.clone(),
            theme,
            ToggleVariant::Default,
            ToggleSize::Default,
            ToggleContent::text("Code"),
            false,
        )
        .expect("valid")
        .disabled(true)
        .focus_visibility(FocusVisibility::Visible);
        assert!(!toggle.focus_ring_visible(window));
    });
}

#[gpui::test]
fn identity_is_stable_and_icon_content_renders(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, cx| {
        let label = AccessibleLabel::new("Toggle bold").expect("non-empty");
        ToggleProbe::new(
            cx,
            true,
            false,
            ToggleVariant::Outline,
            ToggleSize::Small,
            ToggleContent::icon_only(AssetId::TABLER_CHECK, label),
            false,
        )
    });

    let bounds = cx
        .debug_bounds(TOGGLE_SELECTOR)
        .expect("icon-only toggle must expose debug bounds");
    assert_eq!(bounds.size.height, px(32.0));
    cx.simulate_click(bounds.center(), Modifiers::none());
    cx.update(|_, app| {
        assert_eq!(view.read(app).activations(), 1);
        assert_eq!(view.read(app).next_value(), Some(false));
        // Accessible label is retained.
        assert_eq!(view.read(app).content.accessible_label(), "Toggle bold");
        // Variant and size are retained.
        assert_eq!(view.read(app).variant, ToggleVariant::Outline);
        assert_eq!(view.read(app).size, ToggleSize::Small);
        assert!(view.read(app).pressed);
    });

    // Stable selector: second read of same selector yields same bounds.
    let second = cx
        .debug_bounds(TOGGLE_SELECTOR)
        .expect("selector must remain stable");
    assert_eq!(bounds, second);
}
