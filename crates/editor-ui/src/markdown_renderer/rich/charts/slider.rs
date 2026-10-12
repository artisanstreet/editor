//! A small value slider: a track, a filled part, and a thumb, snapped to a
//! step. Pressing the track sets the value; dragging follows the pointer
//! anywhere over the block, through [`follow_drag`] on the block's root.

use gpui::{
    App, Div, ElementId, Entity, FontWeight, InteractiveElement, MouseButton, MouseDownEvent,
    ParentElement as _, Pixels, SharedString, Styled as _, Window, div, px, relative,
};

use super::super::kit;
use super::scale::Domain;
use super::scene::{Probe, plot_canvas, probe};
use crate::theme::ArtisanTheme;

/// A slider's value and drag, kept in the window.
pub(super) struct SliderState {
    pub(super) value: f64,
    range: Domain,
    step: f64,
    dragging: bool,
    track: Probe,
}

impl SliderState {
    /// A slider over `min..=max` stepping by `step` (a hundredth of the range
    /// when absent or not positive), starting at `value` snapped.
    pub(super) fn new(min: f64, max: f64, step: Option<f64>, value: f64) -> Self {
        let range = Domain::new(min, max);
        let step = step
            .filter(|step| step.is_finite() && *step > 0.0)
            .unwrap_or(range.span() / 100.0);
        let mut state = Self {
            value: range.lo,
            range,
            step,
            dragging: false,
            track: probe(),
        };
        state.value = state.snap(value);
        state
    }

    /// `value` clamped to the range and snapped to the nearest step.
    pub(super) fn snap(&self, value: f64) -> f64 {
        let value = if value.is_finite() {
            value
        } else {
            self.range.lo
        };
        let steps = ((value - self.range.lo) / self.step).round();
        (self.range.lo + steps * self.step).clamp(self.range.lo, self.range.hi)
    }

    /// Where the value sits along the track, 0..=1.
    pub(super) fn position(&self) -> f32 {
        self.range.fraction(self.value).clamp(0.0, 1.0)
    }

    /// Moves the value under the pointer at window x `x`; true if it changed.
    fn seek(&mut self, x: Pixels) -> bool {
        let Some(bounds) = self.track.get() else {
            return false;
        };
        let width = f32::from(bounds.size.width);
        if width <= 0.0 {
            return false;
        }
        let t = (f32::from(x - bounds.origin.x) / width).clamp(0.0, 1.0);
        let value = self.snap(self.range.value_at(t));
        let changed = (value - self.value).abs() > self.step * 1e-6;
        self.value = value;
        changed
    }
}

/// The slider row: label, track, and the readout beside it.
pub(super) fn slider(
    theme: &ArtisanTheme,
    id: SharedString,
    label: SharedString,
    readout: SharedString,
    state: &Entity<SliderState>,
    cx: &App,
) -> Div {
    let current = state.read(cx);
    let t = current.position();
    let track_probe = current.track.clone();
    let accent = kit::Tone::Warning.color(theme);
    let press = state.clone();
    let track = div()
        .id(ElementId::Name(id))
        .relative()
        .flex_1()
        .min_w(px(80.0))
        .h(px(20.0))
        .cursor_ew_resize()
        .on_mouse_down(MouseButton::Left, move |event: &MouseDownEvent, _, cx| {
            press.update(cx, |state, cx| {
                state.dragging = true;
                state.seek(event.position.x);
                cx.notify();
            });
        })
        .child(
            div()
                .absolute()
                .left_0()
                .right_0()
                .top(px(9.0))
                .h(px(2.0))
                .rounded(px(1.0))
                .bg(kit::hairline(theme)),
        )
        .child(
            div()
                .absolute()
                .left_0()
                .top(px(9.0))
                .h(px(2.0))
                .w(relative(t))
                .rounded(px(1.0))
                .bg(accent.to_paint()),
        )
        .child(
            div()
                .absolute()
                .top(px(4.0))
                .left(relative(t))
                .ml(px(-6.0))
                .size(px(12.0))
                .rounded(px(6.0))
                .border_2()
                .border_color(accent.to_paint())
                .bg(theme.colors.background.to_paint()),
        )
        .child(plot_canvas(Vec::new(), track_probe));
    div()
        .w_full()
        .flex()
        .items_center()
        .gap(px(12.0))
        .child(
            div()
                .flex_shrink_0()
                .max_w(px(160.0))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_size(px(12.0))
                .line_height(px(16.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.colors.muted_foreground.to_paint())
                .child(label),
        )
        .child(track)
        .child(
            div()
                .flex_shrink_0()
                .whitespace_nowrap()
                .text_size(px(12.0))
                .line_height(px(16.0))
                .text_color(theme.colors.foreground.to_paint())
                .child(readout),
        )
}

/// Lets `root` carry slider drags: moves over it seek any dragging slider,
/// and releasing the button anywhere ends the drag.
pub(super) fn follow_drag<E: InteractiveElement>(root: E, sliders: &[Entity<SliderState>]) -> E {
    if sliders.is_empty() {
        return root;
    }
    let moving = sliders.to_vec();
    let releasing = sliders.to_vec();
    let releasing_out = sliders.to_vec();
    root.on_mouse_move(move |event, _, cx| {
        for slider in &moving {
            if !slider.read(cx).dragging {
                continue;
            }
            if event.pressed_button != Some(MouseButton::Left) {
                slider.update(cx, |state, _| state.dragging = false);
                continue;
            }
            slider.update(cx, |state, cx| {
                if state.seek(event.position.x) {
                    cx.notify();
                }
            });
        }
    })
    .on_mouse_up(MouseButton::Left, move |_, _, cx| release(&releasing, cx))
    .on_mouse_up_out(MouseButton::Left, move |_, _, cx| {
        release(&releasing_out, cx);
    })
}

fn release(sliders: &[Entity<SliderState>], cx: &mut App) {
    for slider in sliders {
        if slider.read(cx).dragging {
            slider.update(cx, |state, _| state.dragging = false);
        }
    }
}

/// The slider entity keyed under `id`, made from `init` on first use.
pub(super) fn keyed_slider(
    window: &mut Window,
    cx: &mut App,
    id: SharedString,
    init: impl FnOnce() -> SliderState,
) -> Entity<SliderState> {
    kit::keyed(window, cx, ElementId::Name(id), init)
}
