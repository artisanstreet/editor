//! A sealed value slider that owns track geometry, focus, keyboard and drag behavior.

use crate::theme::{ArtisanTheme, RadiusStep, RadiusTokens, TextStep, TypographyTokens};
use gpui::prelude::FluentBuilder;
use gpui::{
    App, Bounds, BoxShadow, ElementId, Entity, EventEmitter, FontWeight, InteractiveElement,
    IntoElement, MouseButton, MouseDownEvent, ParentElement as _, Pixels, RenderOnce, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, canvas, div, point, px, relative,
};
use std::{cell::Cell, rc::Rc};

/// A finite ordered range and positive increment. Invalid endpoints use 0..=1;
/// equal endpoints describe a fixed value. The default step is 1% of the range.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SliderRange {
    min: f64,
    max: f64,
    step: f64,
}
impl SliderRange {
    pub fn new(min: f64, max: f64, step: Option<f64>) -> Self {
        let (min, max) = if min.is_finite() && max.is_finite() && (max - min).is_finite() {
            (min.min(max), min.max(max))
        } else {
            (0.0, 1.0)
        };
        let span = max - min;
        let fallback = if span > 0.0 {
            (span / 100.0).max(f64::MIN_POSITIVE.min(span))
        } else {
            1.0
        };
        let step = step
            .filter(|step| step.is_finite() && *step > 0.0)
            .unwrap_or(fallback);
        Self { min, max, step }
    }
    pub fn snap(self, value: f64) -> f64 {
        let value = if value.is_finite() {
            value.clamp(self.min, self.max)
        } else {
            self.min
        };
        if value == self.min || value == self.max {
            return value;
        }
        let steps = ((value - self.min) / self.step).round();
        let snapped = self.min + steps * self.step;
        if snapped.is_finite() {
            snapped.clamp(self.min, self.max)
        } else {
            value
        }
    }
    fn command(self, value: f64, command: SliderCommand) -> f64 {
        match command {
            SliderCommand::Start => self.min,
            SliderCommand::End => self.max,
            SliderCommand::Previous => self.snap((value - self.step).max(self.min)),
            SliderCommand::Next => self.snap((value + self.step).min(self.max)),
        }
    }
}
#[derive(Clone, Copy)]
enum SliderCommand {
    Start,
    End,
    Previous,
    Next,
}
impl SliderCommand {
    fn from_key(key: &str) -> Option<Self> {
        match key {
            "home" => Some(Self::Start),
            "end" => Some(Self::End),
            "left" | "down" => Some(Self::Previous),
            "right" | "up" => Some(Self::Next),
            _ => None,
        }
    }
}

/// Subscribe to the slider entity for feature-owned value conversion or actions.
#[derive(Clone, Copy, Debug)]
pub struct SliderChanged(pub f64);

/// Retained interaction state. Feature code can read a value or reset its range;
/// track bounds, drag state and enabled state stay owned by the component.
pub struct SliderState {
    value: f64,
    range: SliderRange,
    dragging: bool,
    enabled: bool,
    track: Rc<Cell<Option<Bounds<Pixels>>>>,
}
impl EventEmitter<SliderChanged> for SliderState {}
impl SliderState {
    pub fn new(range: SliderRange, value: f64) -> Self {
        Self {
            value: range.snap(value),
            range,
            dragging: false,
            enabled: true,
            track: Rc::new(Cell::new(None)),
        }
    }
    pub fn value(&self) -> f64 {
        self.value
    }
    /// Reset when feature data changes; cancel an in-flight drag and keep geometry ownership.
    pub fn reset(&mut self, range: SliderRange, value: f64) {
        self.range = range;
        self.value = range.snap(value);
        self.dragging = false;
    }
    fn set_value(&mut self, value: f64) -> bool {
        let value = self.range.snap(value);
        let changed = value != self.value;
        self.value = value;
        changed
    }
    fn position(&self) -> f32 {
        let span = self.range.max - self.range.min;
        if span == 0.0 {
            return 0.0;
        }
        #[expect(
            clippy::cast_possible_truncation,
            reason = "bounded track fraction fits f32"
        )]
        let fraction = ((self.value - self.range.min) / span).clamp(0.0, 1.0) as f32;
        fraction
    }
    fn seek(&mut self, x: Pixels) -> bool {
        if !self.enabled {
            return false;
        }
        let Some(bounds) = self.track.get() else {
            return false;
        };
        let width = f32::from(bounds.size.width);
        if width <= 0.0 {
            return false;
        }
        let t = (f32::from(x - bounds.origin.x) / width).clamp(0.0, 1.0);
        self.set_value(self.range.min + f64::from(t) * (self.range.max - self.range.min))
    }
    pub fn keyed(
        window: &mut Window,
        cx: &mut App,
        id: impl Into<ElementId>,
        range: SliderRange,
        value: f64,
    ) -> Entity<Self> {
        window.use_keyed_state(id.into(), cx, |_, _| Self::new(range, value))
    }
}

/// A labelled slider row. Callers supply labels and value conversion, and may
/// constrain the row from its parent. Internal sizes and focus are sealed.
#[derive(IntoElement)]
pub struct Slider {
    id: ElementId,
    theme: ArtisanTheme,
    label: SharedString,
    readout: SharedString,
    state: Entity<SliderState>,
    disabled: bool,
    debug_selector: Option<SharedString>,
}
impl Slider {
    pub fn new(
        id: impl Into<ElementId>,
        theme: ArtisanTheme,
        label: impl Into<SharedString>,
        readout: impl Into<SharedString>,
        state: Entity<SliderState>,
    ) -> Self {
        Self {
            id: id.into(),
            theme,
            label: label.into(),
            readout: readout.into(),
            state,
            disabled: false,
            debug_selector: None,
        }
    }
    #[must_use]
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
    #[must_use]
    pub fn debug_selector(mut self, selector: impl Into<SharedString>) -> Self {
        self.debug_selector = Some(selector.into());
        self
    }
    /// Attach to the containing block so drags continue outside the track and
    /// release outside the block cancels them. Every feature uses this same owner.
    pub fn follow_drag<E: InteractiveElement>(root: E, sliders: &[Entity<SliderState>]) -> E {
        if sliders.is_empty() {
            return root;
        }
        let moving = sliders.to_vec();
        let releasing = sliders.to_vec();
        let releasing_out = sliders.to_vec();
        root.on_mouse_move(move |event, _, cx| {
            for slider in &moving {
                if !slider.read(cx).dragging || !slider.read(cx).enabled {
                    continue;
                }
                if event.pressed_button != Some(MouseButton::Left) {
                    slider.update(cx, |state, _| state.dragging = false);
                    continue;
                }
                slider.update(cx, |state, cx| {
                    if state.seek(event.position.x) {
                        cx.emit(SliderChanged(state.value));
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
}

impl RenderOnce for Slider {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let Self {
            id,
            theme,
            label,
            readout,
            state,
            disabled,
            debug_selector,
        } = self;
        state.update(cx, |state, _| {
            state.enabled = !disabled;
            if disabled {
                state.dragging = false;
            }
        });
        let current = state.read(cx);
        let t = current.position();
        let track_probe = current.track.clone();
        let accent = theme.colors.banner_warning;
        let press = state.clone();
        let keys = state.clone();
        let focus_theme = theme;
        let track = div()
            .id(id)
            .when(!disabled, |track| track.focusable().tab_index(0))
            .when(disabled, |track| {
                track.opacity(0.5).aria_description("Unavailable")
            })
            .when_some(debug_selector, |track, selector| {
                track.debug_selector(move || selector.to_string())
            })
            .role(gpui::Role::Slider)
            .aria_label(label.clone())
            .aria_value(readout.clone())
            .focus_visible(move |style| {
                style.shadow(vec![BoxShadow {
                    color: focus_theme.interaction.focus_ring_color.to_paint(),
                    offset: point(px(0.0), px(0.0)),
                    blur_radius: px(0.0),
                    spread_radius: focus_theme.interaction.focus_ring_width,
                    inset: false,
                }])
            })
            .on_key_down(move |event, _, cx| {
                if disabled {
                    return;
                }
                let Some(command) = SliderCommand::from_key(event.keystroke.key.as_str()) else {
                    return;
                };
                keys.update(cx, |state, cx| {
                    let wanted = state.range.command(state.value, command);
                    if state.set_value(wanted) {
                        cx.emit(SliderChanged(state.value));
                        cx.notify();
                    }
                });
                cx.stop_propagation();
            })
            .relative()
            .flex_1()
            .min_w(px(80.0))
            .h(px(20.0))
            .rounded(RadiusTokens::value(RadiusStep::Sm))
            .border_1()
            .border_color(gpui::transparent_black())
            .when(!disabled, |track| track.cursor_ew_resize())
            .on_mouse_down(MouseButton::Left, move |event: &MouseDownEvent, _, cx| {
                if disabled {
                    return;
                }
                press.update(cx, |state, cx| {
                    state.dragging = true;
                    if state.seek(event.position.x) {
                        cx.emit(SliderChanged(state.value));
                    }
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
                    .rounded(RadiusTokens::value(RadiusStep::X3s))
                    .bg(theme.colors.foreground.with_alpha(0.1).to_paint()),
            )
            .child(
                div()
                    .absolute()
                    .left_0()
                    .top(px(9.0))
                    .h(px(2.0))
                    .w(relative(t))
                    .rounded(RadiusTokens::value(RadiusStep::X3s))
                    .bg(accent.to_paint()),
            )
            .child(
                div()
                    .absolute()
                    .top(px(4.0))
                    .left(relative(t))
                    .ml(px(-6.0))
                    .size(px(12.0))
                    .rounded(RadiusTokens::value(RadiusStep::Sm))
                    .border_2()
                    .border_color(accent.to_paint())
                    .bg(theme.colors.background.to_paint()),
            )
            .child(
                canvas(
                    move |bounds, _, _| track_probe.set(Some(bounds)),
                    |_, (), _, _| {},
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full(),
            );
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
                    .text_size(TypographyTokens::size(TextStep::Xs))
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
                    .text_size(TypographyTokens::size(TextStep::Xs))
                    .line_height(px(16.0))
                    .text_color(theme.colors.foreground.to_paint())
                    .child(readout),
            )
    }
}

fn release(sliders: &[Entity<SliderState>], cx: &mut App) {
    for slider in sliders {
        if slider.read(cx).dragging {
            slider.update(cx, |state, _| state.dragging = false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ranges_keep_endpoints_finite_and_reachable() {
        let range = SliderRange::new(1.0, 0.0, Some(0.3));
        assert_eq!(range.snap(0.4), 0.3);
        assert_eq!(range.command(0.3, SliderCommand::End), 1.0);
        assert_eq!(range.command(1.0, SliderCommand::Next), 1.0);
        assert_eq!(range.command(1.0, SliderCommand::Start), 0.0);
        assert_eq!(range.snap(f64::NAN), 0.0);
        for range in [
            SliderRange::new(f64::NEG_INFINITY, f64::INFINITY, None),
            SliderRange::new(-f64::MAX, f64::MAX, Some(f64::NAN)),
            SliderRange::new(0.0, 1.0, Some(f64::from_bits(1))),
            SliderRange::new(0.0, f64::from_bits(1), None),
            SliderRange::new(7.0, 7.0, Some(0.0)),
        ] {
            assert!(range.step.is_finite() && range.step > 0.0);
            for value in [f64::NAN, f64::MAX, -f64::MAX, 0.5] {
                let value = range.snap(value);
                assert!(value.is_finite() && (range.min..=range.max).contains(&value));
            }
        }
        assert_eq!(
            SliderState::new(SliderRange::new(7.0, 7.0, None), 9.0).position(),
            0.0
        );
    }
    #[test]
    fn disabled_and_reset_cancel_pointer_interaction() {
        let mut state = SliderState::new(SliderRange::new(0.0, 1.0, Some(0.1)), 0.0);
        state.track.set(Some(Bounds {
            origin: point(px(10.0), px(0.0)),
            size: gpui::size(px(100.0), px(20.0)),
        }));
        assert!(state.seek(px(60.0)));
        assert_eq!(state.value(), 0.5);
        state.enabled = false;
        assert!(!state.seek(px(110.0)));
        assert_eq!(state.value(), 0.5);
        state.dragging = true;
        state.reset(SliderRange::new(0.0, 10.0, Some(1.0)), 4.3);
        assert!(!state.dragging);
        assert_eq!(state.value(), 4.0);
    }
}
