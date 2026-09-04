#![allow(clippy::float_cmp)]

use std::cell::Cell;
use std::rc::Rc;

use artisan_ui::button::FocusVisibility;
use artisan_ui::switch::{Switch, SwitchSize, SwitchStyle};
use artisan_ui::theme::{ArtisanTheme, ThemeMode};
use gpui::{
    Context, FocusHandle, IntoElement, KeyUpEvent, Keystroke, Modifiers, ParentElement, Render,
    Styled, TestAppContext, Window, div, point, px, transparent_black,
};

const SWITCH_SELECTOR: &str = "native-switch-under-test";
const DEFAULT_TRACK_WIDTH: f32 = 32.0;
const DEFAULT_TRACK_HEIGHT: f32 = 18.4;
const DEFAULT_RENDERED_TRACK_HEIGHT: f32 = 18.5;
const DEFAULT_THUMB_SIZE: f32 = 16.0;
const DEFAULT_TRAVEL: f32 = 14.0;
const SMALL_TRACK_WIDTH: f32 = 24.0;
const SMALL_TRACK_HEIGHT: f32 = 14.0;
const SMALL_THUMB_SIZE: f32 = 12.0;
const SMALL_TRAVEL: f32 = 10.0;
const GEOMETRY_SELECTORS: [&str; 4] = [
    "geometry-switch-0",
    "geometry-switch-1",
    "geometry-switch-2",
    "geometry-switch-3",
];
const GEOMETRY_TRACK_SELECTORS: [&str; 4] = [
    "geometry-switch-0-track",
    "geometry-switch-1-track",
    "geometry-switch-2-track",
    "geometry-switch-3-track",
];
const GEOMETRY_THUMB_SELECTORS: [&str; 4] = [
    "geometry-switch-0-thumb",
    "geometry-switch-1-thumb",
    "geometry-switch-2-thumb",
    "geometry-switch-3-thumb",
];

#[derive(Clone)]
struct ActivationState {
    activations: Rc<Cell<u32>>,
    next: Rc<Cell<Option<bool>>>,
}

impl ActivationState {
    fn new() -> Self {
        Self {
            activations: Rc::new(Cell::new(0)),
            next: Rc::new(Cell::new(None)),
        }
    }

    fn record(&self, next: bool) {
        self.activations.set(self.activations.get() + 1);
        self.next.set(Some(next));
    }
}

struct SwitchProbe {
    focus: FocusHandle,
    state: ActivationState,
    checked: bool,
    disabled: bool,
    size: SwitchSize,
    refined: bool,
}

impl SwitchProbe {
    fn new(
        cx: &mut Context<Self>,
        state: ActivationState,
        checked: bool,
        disabled: bool,
        size: SwitchSize,
        refined: bool,
    ) -> Self {
        Self {
            focus: cx.focus_handle(),
            state,
            checked,
            disabled,
            size,
            refined,
        }
    }
}

impl Render for SwitchProbe {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.state.clone();
        let theme = ArtisanTheme::for_mode(ThemeMode::Light);
        let switch = Switch::new(
            "native-switch",
            self.focus.clone(),
            theme,
            self.size,
            self.checked,
        )
        .disabled(self.disabled)
        .focus_visibility(FocusVisibility::Visible)
        .debug_selector(SWITCH_SELECTOR)
        .on_change(move |next, _, _, _| state.record(next));
        let switch = if self.refined {
            switch
                .w(px(40.0))
                .h(px(20.0))
                .bg(theme.colors.accent.to_paint())
        } else {
            switch
        };

        div().w(px(200.0)).h(px(80.0)).p(px(20.0)).child(switch)
    }
}

struct GeometryProbe {
    focus: FocusHandle,
}

impl GeometryProbe {
    fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus: cx.focus_handle(),
        }
    }
}

impl Render for GeometryProbe {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let theme = ArtisanTheme::for_mode(ThemeMode::Light);
        let cases = [
            (SwitchSize::Default, false),
            (SwitchSize::Default, true),
            (SwitchSize::Small, false),
            (SwitchSize::Small, true),
        ];
        let mut root = div().size_full().flex().flex_col();
        for (index, (size, checked)) in cases.into_iter().enumerate() {
            let switch = Switch::new(index, self.focus.clone(), theme, size, checked)
                .debug_selector(GEOMETRY_SELECTORS[index]);
            root = root.child(switch);
        }
        root
    }
}

#[test]
fn style_resolves_shared_light_and_dark_paints() {
    let light = ArtisanTheme::for_mode(ThemeMode::Light);
    let dark = ArtisanTheme::for_mode(ThemeMode::Dark);

    let light_unchecked = SwitchStyle::resolve(light, SwitchSize::Default, false);
    assert_eq!(light_unchecked.track_color, light.colors.input.to_paint());
    assert_eq!(
        light_unchecked.thumb_color,
        light.colors.background.to_paint()
    );

    let light_checked = SwitchStyle::resolve(light, SwitchSize::Default, true);
    assert_eq!(light_checked.track_color, light.colors.primary.to_paint());
    assert_eq!(
        light_checked.thumb_color,
        light.colors.background.to_paint()
    );

    let dark_unchecked = SwitchStyle::resolve(dark, SwitchSize::Default, false);
    assert_eq!(
        dark_unchecked.track_color,
        dark.colors.input.with_alpha(0.8).to_paint()
    );
    assert_eq!(
        dark_unchecked.thumb_color,
        dark.colors.foreground.to_paint()
    );

    let dark_checked = SwitchStyle::resolve(dark, SwitchSize::Default, true);
    assert_eq!(dark_checked.track_color, dark.colors.primary.to_paint());
    assert_eq!(
        dark_checked.thumb_color,
        dark.colors.primary_foreground.to_paint()
    );
    assert_eq!(dark_checked.border_color, transparent_black());
    assert_eq!(dark_checked.border_width, px(1.0));
    assert_eq!(dark_checked.disabled_opacity, 0.5);
}

#[gpui::test]
fn visible_geometry_is_exact_for_both_sizes_and_states(cx: &mut TestAppContext) {
    let (_view, cx) = cx.add_window_view(|_, cx| GeometryProbe::new(cx));
    let expected = [
        (
            SwitchSize::Default,
            false,
            DEFAULT_TRACK_WIDTH,
            DEFAULT_TRACK_HEIGHT,
            DEFAULT_RENDERED_TRACK_HEIGHT,
            DEFAULT_THUMB_SIZE,
            DEFAULT_TRAVEL,
        ),
        (
            SwitchSize::Default,
            true,
            DEFAULT_TRACK_WIDTH,
            DEFAULT_TRACK_HEIGHT,
            DEFAULT_RENDERED_TRACK_HEIGHT,
            DEFAULT_THUMB_SIZE,
            DEFAULT_TRAVEL,
        ),
        (
            SwitchSize::Small,
            false,
            SMALL_TRACK_WIDTH,
            SMALL_TRACK_HEIGHT,
            SMALL_TRACK_HEIGHT,
            SMALL_THUMB_SIZE,
            SMALL_TRAVEL,
        ),
        (
            SwitchSize::Small,
            true,
            SMALL_TRACK_WIDTH,
            SMALL_TRACK_HEIGHT,
            SMALL_TRACK_HEIGHT,
            SMALL_THUMB_SIZE,
            SMALL_TRAVEL,
        ),
    ];

    for (index, (size, checked, width, height, rendered_height, thumb_size, travel)) in
        expected.into_iter().enumerate()
    {
        let style = SwitchStyle::resolve(ArtisanTheme::for_mode(ThemeMode::Light), size, checked);
        let track = cx
            .debug_bounds(GEOMETRY_TRACK_SELECTORS[index])
            .expect("visible track must expose debug bounds");
        let thumb = cx
            .debug_bounds(GEOMETRY_THUMB_SELECTORS[index])
            .expect("thumb must expose debug bounds");

        assert_eq!(style.track_width, px(width));
        assert_eq!(style.track_height, px(height));
        assert_eq!(track.size.width, px(width));
        assert_eq!(track.size.height, px(rendered_height));
        assert_eq!(thumb.size.width, px(thumb_size));
        assert_eq!(thumb.size.height, px(thumb_size));
        assert_eq!(style.checked_travel, px(travel));
        assert_eq!(
            thumb.origin.x - track.origin.x,
            px(1.0) + if checked { px(travel) } else { px(0.0) }
        );
        let top_inset = thumb.origin.y - track.origin.y;
        let bottom_inset =
            track.origin.y + track.size.height - (thumb.origin.y + thumb.size.height);
        assert!(f32::from((top_inset - bottom_inset).abs()) <= 0.5);
    }
}

#[gpui::test]
fn default_unchecked_and_controlled_checked_rendering(cx: &mut TestAppContext) {
    let state = ActivationState::new();
    let state_for_view = state.clone();
    let (view, cx) = cx.add_window_view(move |_, cx| {
        SwitchProbe::new(cx, state_for_view, false, false, SwitchSize::Default, false)
    });

    cx.update(|_, app| assert!(!view.read(app).checked));
    let unchecked = cx
        .debug_bounds("native-switch-under-test-thumb")
        .expect("default unchecked thumb must paint");

    cx.update(|_, app| {
        view.update(app, |probe, cx| {
            probe.checked = true;
            cx.notify();
        });
    });
    cx.run_until_parked();

    cx.update(|_, app| assert!(view.read(app).checked));
    let checked = cx
        .debug_bounds("native-switch-under-test-thumb")
        .expect("controlled checked thumb must paint");
    assert_eq!(
        checked.origin.x - unchecked.origin.x,
        SwitchStyle::resolve(
            ArtisanTheme::for_mode(ThemeMode::Light),
            SwitchSize::Default,
            true,
        )
        .checked_travel
    );
    assert_eq!(state.activations.get(), 0);
}

#[gpui::test]
fn pointer_activation_emits_one_next_checked_value_and_focuses(cx: &mut TestAppContext) {
    let state = ActivationState::new();
    let state_for_view = state.clone();
    let (view, cx) = cx.add_window_view(move |_, cx| {
        SwitchProbe::new(cx, state_for_view, false, false, SwitchSize::Default, false)
    });
    let track = cx
        .debug_bounds("native-switch-under-test-track")
        .expect("switch track must paint");

    cx.simulate_click(track.center(), Modifiers::none());
    cx.update(|window, app| {
        let probe = view.read(app);
        assert!(probe.focus.is_focused(window));
        assert_eq!(probe.state.activations.get(), 1);
        assert_eq!(probe.state.next.get(), Some(true));
    });
}

#[gpui::test]
fn enter_and_space_activate_controlled_next_values(cx: &mut TestAppContext) {
    let state = ActivationState::new();
    let state_for_view = state.clone();
    let (view, cx) = cx.add_window_view(move |window, cx| {
        let probe = SwitchProbe::new(cx, state_for_view, false, false, SwitchSize::Default, false);
        window.focus(&probe.focus, cx);
        probe
    });
    cx.run_until_parked();

    cx.simulate_event(KeyUpEvent {
        keystroke: Keystroke::parse("enter").expect("known keyboard activation key"),
    });
    cx.run_until_parked();
    assert_eq!(state.activations.get(), 1);
    assert_eq!(state.next.get(), Some(true));

    let next = state.next.get().expect("Enter must produce a next value");
    cx.update(|_, app| {
        view.update(app, |probe, cx| {
            probe.checked = next;
            cx.notify();
        });
    });
    cx.run_until_parked();

    cx.simulate_event(KeyUpEvent {
        keystroke: Keystroke::parse("space").expect("known keyboard activation key"),
    });
    cx.run_until_parked();
    cx.update(|window, app| {
        let probe = view.read(app);
        assert!(probe.focus.is_focused(window));
        assert_eq!(probe.state.activations.get(), 2);
        assert_eq!(probe.state.next.get(), Some(false));
    });
}

#[gpui::test]
fn disabled_switch_is_half_opaque_and_suppresses_pointer_keyboard(cx: &mut TestAppContext) {
    let state = ActivationState::new();
    let state_for_view = state.clone();
    let (view, cx) = cx.add_window_view(move |_, cx| {
        SwitchProbe::new(cx, state_for_view, false, true, SwitchSize::Default, false)
    });
    let track = cx
        .debug_bounds("native-switch-under-test-track")
        .expect("disabled switch must remain visible");
    cx.simulate_click(track.center(), Modifiers::none());
    cx.update(|window, app| {
        let focus = view.read(app).focus.clone();
        assert!(!focus.is_focused(window));
        window.focus(&focus, app);
        view.update(app, |_, cx| cx.notify());
    });
    cx.run_until_parked();

    for key in ["enter", "space"] {
        cx.simulate_event(KeyUpEvent {
            keystroke: Keystroke::parse(key).expect("known keyboard activation key"),
        });
    }
    cx.update(|_, app| {
        assert_eq!(view.read(app).state.activations.get(), 0);
        assert_eq!(view.read(app).state.next.get(), None);
    });
    assert_eq!(
        SwitchStyle::resolve(
            ArtisanTheme::for_mode(ThemeMode::Light),
            SwitchSize::Default,
            false,
        )
        .disabled_opacity,
        0.5
    );
}

#[gpui::test]
fn expanded_hit_target_preserves_visible_track_bounds(cx: &mut TestAppContext) {
    let state = ActivationState::new();
    let state_for_view = state.clone();
    let (_view, cx) = cx.add_window_view(move |_, cx| {
        SwitchProbe::new(cx, state_for_view, false, false, SwitchSize::Default, false)
    });
    let hitbox = cx
        .debug_bounds(SWITCH_SELECTOR)
        .expect("switch hitbox must paint debug bounds");
    let track = cx
        .debug_bounds("native-switch-under-test-track")
        .expect("switch track must paint debug bounds");
    assert_eq!(hitbox.size.width, track.size.width + px(24.0));
    assert_eq!(hitbox.size.height, track.size.height + px(16.0));

    let expanded_point = point(track.origin.x - px(6.0), track.origin.y - px(4.0));
    assert!(expanded_point.x < track.origin.x);
    assert!(expanded_point.y < track.origin.y);
    assert!(expanded_point.x >= hitbox.origin.x);
    assert!(expanded_point.y >= hitbox.origin.y);
    cx.simulate_click(expanded_point, Modifiers::none());
    assert_eq!(state.activations.get(), 1);
    assert_eq!(state.next.get(), Some(true));

    let after = cx
        .debug_bounds("native-switch-under-test-track")
        .expect("track must remain inspectable after hitbox click");
    assert_eq!(after, track);
}

#[gpui::test]
fn caller_style_refinements_override_the_base_track_recipe(cx: &mut TestAppContext) {
    let state = ActivationState::new();
    let state_for_view = state.clone();
    let (_view, cx) = cx.add_window_view(move |_, cx| {
        SwitchProbe::new(cx, state_for_view, false, false, SwitchSize::Default, true)
    });
    let track = cx
        .debug_bounds("native-switch-under-test-track")
        .expect("refined switch track must paint");
    let hitbox = cx
        .debug_bounds(SWITCH_SELECTOR)
        .expect("refined switch hitbox must paint");
    assert_eq!(track.size.width, px(40.0));
    assert_eq!(track.size.height, px(20.0));
    assert_eq!(hitbox.size.width, px(64.0));
    assert_eq!(hitbox.size.height, px(36.0));
}
