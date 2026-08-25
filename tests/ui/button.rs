//! Behavioral coverage for the first-workflow native GPUI button.

use std::cell::Cell;
use std::rc::Rc;

use artisan_ui::AssetId;
use artisan_ui::button::{
    AccessibleLabel, AccessibleLabelError, Button, ButtonConstructionError, ButtonContent,
    ButtonSize, ButtonStyle, ButtonVariant, FocusVisibility,
};
use artisan_ui::motion::MotionPolicy;
use artisan_ui::theme::{ArtisanTheme, SurfaceStep, ThemeMode};
use gpui::{
    ClickEvent, Context, FocusHandle, IntoElement, KeyUpEvent, Keystroke, Modifiers, ParentElement,
    Render, Styled, TestAppContext, Window, div, point, transparent_black,
};

const BUTTON_SELECTOR: &str = "native-button-under-test";

#[derive(Clone)]
struct ButtonProbeState {
    pointer_activations: Rc<Cell<u32>>,
    keyboard_activations: Rc<Cell<u32>>,
    focus_ring_visible: Rc<Cell<bool>>,
}

impl ButtonProbeState {
    fn total_activations(&self) -> u32 {
        self.pointer_activations.get() + self.keyboard_activations.get()
    }
}

struct ButtonProbe {
    focus: FocusHandle,
    state: ButtonProbeState,
    disabled: bool,
    focus_visibility: FocusVisibility,
}

impl ButtonProbe {
    fn new(cx: &mut Context<Self>, disabled: bool, focus_visibility: FocusVisibility) -> Self {
        Self {
            focus: cx.focus_handle(),
            state: ButtonProbeState {
                pointer_activations: Rc::new(Cell::new(0)),
                keyboard_activations: Rc::new(Cell::new(0)),
                focus_ring_visible: Rc::new(Cell::new(false)),
            },
            disabled,
            focus_visibility,
        }
    }
}

impl Render for ButtonProbe {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let pointer_activations = self.state.pointer_activations.clone();
        let keyboard_activations = self.state.keyboard_activations.clone();
        let button = Button::new(
            "native-button",
            self.focus.clone(),
            ArtisanTheme::for_mode(ThemeMode::Light),
            MotionPolicy::Full,
            ButtonVariant::Default,
            ButtonSize::Small,
            ButtonContent::text("Send"),
        )
        .expect("text content and small size are compatible")
        .focus_visibility(self.focus_visibility)
        .disabled(self.disabled)
        .debug_selector(BUTTON_SELECTOR)
        .on_activate(move |event, _, _| match event {
            ClickEvent::Mouse(_) => {
                pointer_activations.set(pointer_activations.get() + 1);
            }
            ClickEvent::Keyboard(_) => {
                keyboard_activations.set(keyboard_activations.get() + 1);
            }
        });

        self.state
            .focus_ring_visible
            .set(button.focus_ring_visible(window));
        div().size_full().child(button)
    }
}

#[test]
fn accessible_label_and_size_content_contracts_are_typed() {
    assert_eq!(AccessibleLabel::new(" \t\n"), Err(AccessibleLabelError));

    let label = AccessibleLabel::new("Send message").expect("non-empty label");
    let icon = ButtonContent::icon_only(AssetId::TABLER_ARROW_UP, label.clone());
    assert_eq!(icon.accessible_label(), "Send message");
}

#[gpui::test]
fn icon_content_rejects_a_non_square_size(cx: &mut TestAppContext) {
    let label = AccessibleLabel::new("Send message").expect("non-empty label");
    let icon = ButtonContent::icon_only(AssetId::TABLER_ARROW_UP, label);
    let (view, cx) =
        cx.add_window_view(|_, cx| ButtonProbe::new(cx, false, FocusVisibility::Visible));
    let focus_error = cx.update(|_, app| {
        Button::new(
            "bad-icon-size",
            view.read(app).focus.clone(),
            ArtisanTheme::for_mode(ThemeMode::Light),
            MotionPolicy::Full,
            ButtonVariant::Ghost,
            ButtonSize::Small,
            icon,
        )
        .err()
    });
    assert_eq!(
        focus_error,
        Some(ButtonConstructionError::IconOnlyRequiresIconSize)
    );
}

#[gpui::test]
fn square_icon_size_rejects_text_and_visible_labels_must_not_be_blank(cx: &mut TestAppContext) {
    let (view, cx) =
        cx.add_window_view(|_, cx| ButtonProbe::new(cx, false, FocusVisibility::Visible));
    let errors = cx.update(|_, app| {
        let focus = view.read(app).focus.clone();
        let make = |id, size, content| {
            Button::new(
                id,
                focus.clone(),
                ArtisanTheme::for_mode(ThemeMode::Light),
                MotionPolicy::Full,
                ButtonVariant::Default,
                size,
                content,
            )
            .err()
        };
        (
            make(
                "bad-text-size",
                ButtonSize::IconSmall,
                ButtonContent::text("Send"),
            ),
            make(
                "blank-text",
                ButtonSize::Small,
                ButtonContent::text(" \t\n"),
            ),
            make(
                "blank-icon-text",
                ButtonSize::Small,
                ButtonContent::icon_text(AssetId::TABLER_ARROW_UP, "  "),
            ),
        )
    });
    assert_eq!(
        errors,
        (
            Some(ButtonConstructionError::IconSizeRequiresIconOnly),
            Some(ButtonConstructionError::EmptyVisibleLabel),
            Some(ButtonConstructionError::EmptyVisibleLabel),
        )
    );
}

#[test]
fn recipes_use_theme_tokens_and_reduced_motion_removes_press_offset() {
    let light = ArtisanTheme::for_mode(ThemeMode::Light);
    let full = ButtonStyle::resolve(
        light,
        ButtonVariant::Default,
        ButtonSize::Small,
        MotionPolicy::Full,
    );
    assert_eq!(full.height, light.density.control_sm);
    assert_eq!(full.background, light.colors.primary.to_paint());
    assert_eq!(full.foreground, light.colors.primary_foreground.to_paint());
    assert_eq!(
        full.focus_ring,
        light.interaction.focus_ring_color.to_paint()
    );
    assert_eq!(full.focus_border, light.colors.ring.to_paint());
    assert!(full.pressed_offset_y.is_some());

    let reduced = ButtonStyle::resolve(
        light,
        ButtonVariant::Default,
        ButtonSize::Small,
        MotionPolicy::Reduced,
    );
    assert_eq!(reduced.pressed_offset_y, None);

    let icon = ButtonStyle::resolve(
        light,
        ButtonVariant::Ghost,
        ButtonSize::IconSmall,
        MotionPolicy::Full,
    );
    assert_eq!(icon.width, Some(light.density.control_sm));
    assert_eq!(icon.height, light.density.control_sm);
    assert_eq!(icon.horizontal_padding, gpui::px(0.0));
    assert_eq!(icon.content_gap, gpui::px(0.0));

    for variant in [
        ButtonVariant::Default,
        ButtonVariant::Outline,
        ButtonVariant::Secondary,
        ButtonVariant::Ghost,
    ] {
        let style = ButtonStyle::resolve(light, variant, ButtonSize::Small, MotionPolicy::Full);
        assert_eq!(style.height, light.density.control_sm);
        assert_eq!(style.icon_size, gpui::px(16.0));
        assert_eq!(style.disabled_opacity.to_bits(), 0.5_f32.to_bits());
    }
}

#[test]
fn variant_recipes_map_exact_light_and_dark_theme_tokens() {
    let light = ArtisanTheme::for_mode(ThemeMode::Light);
    let dark = ArtisanTheme::for_mode(ThemeMode::Dark);

    let default = ButtonStyle::resolve(
        light,
        ButtonVariant::Default,
        ButtonSize::Small,
        MotionPolicy::Full,
    );
    assert_eq!(default.background, light.colors.primary.to_paint());
    assert_eq!(
        default.hover_background,
        light.colors.primary.with_alpha(0.8).to_paint()
    );

    let outline_light = ButtonStyle::resolve(
        light,
        ButtonVariant::Outline,
        ButtonSize::Small,
        MotionPolicy::Full,
    );
    assert_eq!(
        outline_light.background,
        light.surfaces.value(SurfaceStep::S100).to_paint()
    );
    assert_eq!(outline_light.border, light.colors.border.to_paint());
    assert_eq!(
        outline_light.hover_background,
        light.colors.input.with_alpha(0.5).to_paint()
    );
    let outline_dark = ButtonStyle::resolve(
        dark,
        ButtonVariant::Outline,
        ButtonSize::Small,
        MotionPolicy::Full,
    );
    assert_eq!(
        outline_dark.background,
        dark.surfaces.value(SurfaceStep::S900).to_paint()
    );

    let secondary = ButtonStyle::resolve(
        light,
        ButtonVariant::Secondary,
        ButtonSize::Small,
        MotionPolicy::Full,
    );
    assert_eq!(secondary.background, light.colors.secondary.to_paint());
    assert_eq!(
        secondary.hover_background,
        light.colors.secondary.with_alpha(0.8).to_paint()
    );

    let ghost_light = ButtonStyle::resolve(
        light,
        ButtonVariant::Ghost,
        ButtonSize::Small,
        MotionPolicy::Full,
    );
    let ghost_dark = ButtonStyle::resolve(
        dark,
        ButtonVariant::Ghost,
        ButtonSize::Small,
        MotionPolicy::Full,
    );
    assert_eq!(ghost_light.background, transparent_black());
    assert_eq!(ghost_light.hover_background, light.colors.muted.to_paint());
    assert_eq!(
        ghost_dark.hover_background,
        dark.colors.muted.with_alpha(0.5).to_paint()
    );
}

#[gpui::test]
fn pointer_click_uses_the_mouse_activation_path_and_focuses(cx: &mut TestAppContext) {
    let (view, cx) =
        cx.add_window_view(|_, cx| ButtonProbe::new(cx, false, FocusVisibility::Visible));

    let bounds = cx
        .debug_bounds(BUTTON_SELECTOR)
        .expect("button must paint inspectable bounds");
    let center = point(
        bounds.origin.x + bounds.size.width / 2.0,
        bounds.origin.y + bounds.size.height / 2.0,
    );
    cx.simulate_click(center, Modifiers::none());
    cx.update(|window, app| {
        let focus = view.read(app).focus.clone();
        assert!(focus.is_focused(window));
        let probe = view.read(app);
        assert_eq!(probe.state.pointer_activations.get(), 1);
        assert_eq!(probe.state.keyboard_activations.get(), 0);
    });
}

#[gpui::test]
fn enter_and_space_activate_from_keyboard_focus_without_a_pointer_click(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|window, cx| {
        let probe = ButtonProbe::new(cx, false, FocusVisibility::Visible);
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
        let probe = view.read(app);
        assert!(probe.focus.is_focused(window));
        assert_eq!(probe.state.pointer_activations.get(), 0);
        assert_eq!(probe.state.keyboard_activations.get(), 2);
    });
}

#[gpui::test]
fn disabled_button_suppresses_pointer_keyboard_and_focus_tracking(cx: &mut TestAppContext) {
    let (view, cx) =
        cx.add_window_view(|_, cx| ButtonProbe::new(cx, true, FocusVisibility::Visible));

    let bounds = cx
        .debug_bounds(BUTTON_SELECTOR)
        .expect("disabled button must remain visible");
    let center = point(
        bounds.origin.x + bounds.size.width / 2.0,
        bounds.origin.y + bounds.size.height / 2.0,
    );
    cx.simulate_click(center, Modifiers::none());
    cx.update(|window, app| {
        let focus = view.read(app).focus.clone();
        assert!(!focus.is_focused(window));
        window.focus(&focus);
        view.update(app, |_, cx| cx.notify());
    });
    cx.run_until_parked();
    for key in ["enter", "space"] {
        cx.simulate_event(KeyUpEvent {
            keystroke: Keystroke::parse(key).expect("known keyboard activation key"),
        });
    }

    cx.update(|window, app| {
        let probe = view.read(app);
        assert!(probe.focus.is_focused(window));
        assert!(!probe.state.focus_ring_visible.get());
        assert_eq!(probe.state.total_activations(), 0);
    });
}

#[gpui::test]
fn focus_ring_requires_actual_focus_and_visible_focus_intent(cx: &mut TestAppContext) {
    let (view, cx) =
        cx.add_window_view(|_, cx| ButtonProbe::new(cx, false, FocusVisibility::Visible));
    cx.update(|_, app| {
        assert!(!view.read(app).state.focus_ring_visible.get());
    });

    cx.update(|window, app| {
        let focus = view.read(app).focus.clone();
        window.focus(&focus);
    });
    cx.run_until_parked();
    cx.update(|_, app| {
        assert!(view.read(app).state.focus_ring_visible.get());
    });

    cx.update(|_, app| {
        view.update(app, |probe, cx| {
            probe.focus_visibility = FocusVisibility::Hidden;
            cx.notify();
        });
    });
    cx.run_until_parked();
    cx.update(|_, app| {
        assert!(!view.read(app).state.focus_ring_visible.get());
    });
}
