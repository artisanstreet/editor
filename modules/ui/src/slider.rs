//! Controlled native GPUI slider counterpart to
//! `modules/frontend/src/lib/components/ui/slider/slider.svelte`.
//!
//! The legacy wrapper re-exports `bits-ui`'s `SliderPrimitive.Root` with a
//! horizontal `data-orientation="horizontal"` default, a muted `bg-muted` track
//! (`rounded-4xl`, `h-3`/`w-3` by orientation), a `bg-primary` range that fills
//! the leading share, and a 16 px (`size-4`) white pill thumb with a primary
//! border and hover/focus ring (`ring-ring/50` at 3 px). The value is
//! two-way bound (`bind:value`) over an unconstrained numeric domain and
//! `min`/`max`/`step` control the snapped value. Disabled state halves opacity
//! and suppresses pointer/keyboard interaction (`data-disabled:opacity-50`).
//!
//! This native counterpart keeps the same product intent with an idiomatic
//! controlled GPUI recipe: callers own `value`, `min`, `max`, `step`, and
//! `disabled`; this module owns deterministic clamping/snap, theme-aware
//! rail/range/thumb paint, stable debug selectors, and keyboard/focus
//! semantics GPUI 0.2.2 can truthfully support. No platform accessibility tree,
//! drag pointer-position synthesis, or DOM shims are invented: keyboard moves
//! by `step`, `Home`/`End` jump to bounds, and `PageUp`/`PageDown` move by the
//! larger of `10 * step` and 10% of the range. All `f64` inputs are finite-
//! checked: non-finite or degenerate ranges fall back to a well-defined
//! interval instead of poisoning layout, and reversed ranges normalize to
//! `[low, high]` so a visible fraction remains defined.
//!
//! Rendering uses plain `Div`s: a fixed-thickness rounded track, an
//! `overflow_hidden` fill clip sized by `relative(fraction)`, and an absolute
//! thumb whose leading edge follows `relative(fraction)` with a half-thumb
//! centering offset. Further [`gpui::Styled`] refinements chain onto the
//! returned hitbox/track and later values win, mirroring `progress` and
//! `switch`. The component is a render recipe: a change creates a new `f64`
//! value for the caller without mutating retained state.

use std::rc::Rc;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, BoxShadow, ClickEvent, Div, ElementId, FocusHandle, Hsla, InteractiveElement, IntoElement,
    KeyDownEvent, ParentElement, Pixels, RenderOnce, SharedString, Stateful,
    StatefulInteractiveElement, StyleRefinement, Styled, Window, div, point, px, relative,
    transparent_black,
};

use crate::button::FocusVisibility;
use crate::theme::{ArtisanTheme, RadiusStep, RadiusTokens};

const TRACK_THICKNESS_PX: f32 = 12.0;
const THUMB_SIZE_PX: f32 = 16.0;
const THUMB_BORDER_WIDTH_PX: f32 = 1.0;
const DISABLED_OPACITY: f32 = 0.5;

/// Main-axis direction for the slider and its keyboard semantics.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum SliderOrientation {
    /// Left-to-right; `ArrowLeft`/`ArrowRight` move the thumb.
    #[default]
    Horizontal,
    /// Bottom-to-top; `ArrowUp`/`ArrowDown` move the thumb.
    Vertical,
}

/// Theme-resolved geometry and paint for one slider configuration.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SliderStyle {
    /// Track thickness (`h-3` / `w-3`, 12 px).
    pub track_thickness: Pixels,
    /// Track corner radius (`rounded-4xl`).
    pub track_corner_radius: Pixels,
    /// Thumb edge length (`size-4`, 16 px).
    pub thumb_size: Pixels,
    /// Thumb corner radius (pill, capped to half the thumb).
    pub thumb_corner_radius: Pixels,
    /// Thumb border width.
    pub thumb_border_width: Pixels,
    /// Track background (`--muted`).
    pub track_color: Hsla,
    /// Filled range background (`--primary`).
    pub fill_color: Hsla,
    /// Thumb fill (`--background`).
    pub thumb_color: Hsla,
    /// Thumb border (`--primary`).
    pub thumb_border_color: Hsla,
    /// Focus ring color (`ring/50`).
    pub focus_ring_color: Hsla,
    /// Focus border color (`--ring`).
    pub focus_border_color: Hsla,
    /// Focus ring spread (legacy `ring-[3px]`).
    pub focus_ring_width: Pixels,
    /// Disabled opacity (`0.5`).
    pub disabled_opacity: f32,
}

impl SliderStyle {
    /// Resolves the slider recipe from shared theme tokens.
    #[must_use]
    pub fn resolve(theme: ArtisanTheme) -> Self {
        Self {
            track_thickness: px(TRACK_THICKNESS_PX),
            track_corner_radius: RadiusTokens::value(RadiusStep::X4l),
            thumb_size: px(THUMB_SIZE_PX),
            thumb_corner_radius: px(9999.0),
            thumb_border_width: px(THUMB_BORDER_WIDTH_PX),
            track_color: theme.colors.muted.to_paint(),
            fill_color: theme.colors.primary.to_paint(),
            thumb_color: theme.colors.background.to_paint(),
            thumb_border_color: theme.colors.primary.to_paint(),
            focus_ring_color: theme.interaction.focus_ring_color.to_paint(),
            focus_border_color: theme.colors.ring.to_paint(),
            focus_ring_width: theme.interaction.focus_ring_width,
            disabled_opacity: DISABLED_OPACITY,
        }
    }
}

/// Returns the effective ordered bounds for a slider.
///
/// Non-finite `min` or `max` fall back to `0.0..=100.0` so layout never
/// receives `NaN` or infinity. Reversed ranges are reordered so
/// `low <= high`; degenerate `min == max` is preserved so the range can be
/// detected as a single point.
#[must_use]
pub fn effective_slider_bounds(min: f64, max: f64) -> (f64, f64) {
    if !min.is_finite() || !max.is_finite() {
        return (0.0, 100.0);
    }
    if min <= max { (min, max) } else { (max, min) }
}

/// Normalizes `step` into a strictly positive finite stride.
///
/// Returns `None` when `step` is non-finite, `NaN`, zero, or negative so
/// callers can skip snapping and use pure clamping.
#[must_use]
pub fn normalized_slider_step(step: f64) -> Option<f64> {
    if step.is_finite() && step > 0.0 {
        Some(step)
    } else {
        None
    }
}

/// Clamps and snaps `value` into `[min, max]` with deterministic step
/// normalization.
///
/// - Non-finite `value` becomes `low`.
/// - Non-finite `min`/`max` fall back to `0.0..=100.0` and reversed ranges
///   are ordered.
/// - Degenerate `low == high` returns `low` without division.
/// - Finite positive `step` snaps to the nearest step from `low` and then
///   re-clamps.
#[must_use]
pub fn normalize_slider_value(value: f64, min: f64, max: f64, step: f64) -> f64 {
    let (low, high) = effective_slider_bounds(min, max);
    if (low - high).abs() < f64::EPSILON {
        return low;
    }
    let clamped = if value.is_finite() {
        value.clamp(low, high)
    } else {
        low
    };
    let Some(step) = normalized_slider_step(step) else {
        return clamped;
    };
    let steps = ((clamped - low) / step).round();
    let snapped = low + steps * step;
    if snapped.is_finite() {
        snapped.clamp(low, high)
    } else {
        clamped
    }
}

/// Computes the fill/thumb fraction for `value` inside `[min, max]`.
///
/// Returns `0.0` for degenerate or non-finite bounds. The returned share is
/// clamped to `0.0..=1.0` and is `floor`-stable: `low` maps to `0.0` and
/// `high` maps to `1.0`.
#[must_use]
pub fn slider_fraction(value: f64, min: f64, max: f64) -> f32 {
    let (low, high) = effective_slider_bounds(min, max);
    if !low.is_finite() || !high.is_finite() {
        return 0.0;
    }
    let span = high - low;
    if span.abs() < f64::EPSILON {
        return 0.0;
    }
    let normalized = normalize_slider_value(value, min, max, f64::NAN);
    let frac = (normalized - low) / span;
    if !frac.is_finite() {
        return 0.0;
    }
    f32::from(Pixels::from(frac)).clamp(0.0, 1.0)
}

/// Computes the next controlled value for a non-modified keyboard action.
///
/// Returns `None` for inert keys. `Arrow*` moves by `step` (or `1.0` when
/// `step` is invalid), `Home`/`End` jump to bounds, and `PageUp`/`PageDown`
/// move by the larger of `10 * step` and 10% of the range.
#[must_use]
pub fn next_slider_value_for_key(
    key: &str,
    value: f64,
    min: f64,
    max: f64,
    step: f64,
) -> Option<f64> {
    let (low, high) = effective_slider_bounds(min, max);
    if (low - high).abs() < f64::EPSILON {
        return None;
    }
    let normalized = normalize_slider_value(value, min, max, step);
    let valid_step = normalized_slider_step(step).unwrap_or(1.0);
    let range = (high - low).abs();
    let page_step = (valid_step * 10.0).max(range / 10.0);
    let next = match key {
        "arrowright" | "right" | "arrowup" | "up" => normalized + valid_step,
        "arrowleft" | "left" | "arrowdown" | "down" => normalized - valid_step,
        "home" | "pagehome" => low,
        "end" | "pageend" => high,
        "pageup" => normalized + page_step,
        "pagedown" => normalized - page_step,
        _ => return None,
    };
    Some(normalize_slider_value(next, min, max, step))
}

type ChangeHandler = Rc<dyn Fn(f64, &ClickEvent, &mut Window, &mut App)>;

/// A controlled numeric slider with a single thumb.
///
/// The selected value remains owned by the caller. Keyboard interaction
/// synthesizes a new `f64` and invokes the change handler without mutating
/// the retained value.
#[derive(IntoElement)]
pub struct Slider {
    id: ElementId,
    focus: FocusHandle,
    theme: ArtisanTheme,
    min: f64,
    max: f64,
    step: f64,
    value: f64,
    orientation: SliderOrientation,
    disabled: bool,
    focus_visibility: FocusVisibility,
    on_change: Option<ChangeHandler>,
    debug_selector: Option<SharedString>,
    track: Div,
    fill: Div,
    thumb: Div,
}

impl Slider {
    /// Constructs a slider with the caller-owned `value` and standard
    /// `0.0..=100.0` bounds at `1.0` step.
    #[must_use]
    pub fn new(
        id: impl Into<ElementId>,
        focus: FocusHandle,
        theme: ArtisanTheme,
        value: f64,
    ) -> Self {
        Self::new_with_bounds(id, focus, theme, 0.0, 100.0, 1.0, value)
    }

    /// Constructs a slider with explicit bounds.
    #[must_use]
    pub fn new_with_bounds(
        id: impl Into<ElementId>,
        focus: FocusHandle,
        theme: ArtisanTheme,
        min: f64,
        max: f64,
        step: f64,
        value: f64,
    ) -> Self {
        let style = SliderStyle::resolve(theme);
        let track = div()
            .flex()
            .items_center()
            .overflow_hidden()
            .bg(style.track_color)
            .rounded(style.track_corner_radius);
        let fill = div().h_full().bg(style.fill_color);
        let thumb = div()
            .flex_shrink_0()
            .w(style.thumb_size)
            .h(style.thumb_size)
            .rounded(style.thumb_corner_radius)
            .border_1()
            .border_color(style.thumb_border_color)
            .bg(style.thumb_color);

        Self {
            id: id.into(),
            focus,
            theme,
            min,
            max,
            step,
            value,
            orientation: SliderOrientation::Horizontal,
            disabled: false,
            focus_visibility: FocusVisibility::Hidden,
            on_change: None,
            debug_selector: None,
            track,
            fill,
            thumb,
        }
    }

    /// Sets the minimum bound.
    #[must_use]
    pub const fn min(mut self, min: f64) -> Self {
        self.min = min;
        self
    }

    /// Sets the maximum bound.
    #[must_use]
    pub const fn max(mut self, max: f64) -> Self {
        self.max = max;
        self
    }

    /// Sets both bounds at once.
    #[must_use]
    pub const fn bounds(mut self, min: f64, max: f64) -> Self {
        self.min = min;
        self.max = max;
        self
    }

    /// Sets the snapping step. Non-finite or non-positive steps are treated
    /// as no snapping (pure clamp) at render and normalization time.
    #[must_use]
    pub const fn step(mut self, step: f64) -> Self {
        self.step = step;
        self
    }

    /// Sets the controlled value.
    #[must_use]
    pub const fn value(mut self, value: f64) -> Self {
        self.value = value;
        self
    }

    /// Selects the layout/orientation.
    #[must_use]
    pub const fn orientation(mut self, orientation: SliderOrientation) -> Self {
        self.orientation = orientation;
        self
    }

    /// Selects the disabled presentation and suppresses every interaction.
    #[must_use]
    pub const fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Selects whether actual focus should receive a visible ring.
    #[must_use]
    pub const fn focus_visibility(mut self, visibility: FocusVisibility) -> Self {
        self.focus_visibility = visibility;
        self
    }

    /// Installs the callback invoked with the next controlled value.
    #[must_use]
    pub fn on_change(
        mut self,
        handler: impl Fn(f64, &ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_change = Some(Rc::new(handler));
        self
    }

    /// Adds a stable selector to the interactive hitbox.
    #[must_use]
    pub fn debug_selector(mut self, selector: impl Into<SharedString>) -> Self {
        self.debug_selector = Some(selector.into());
        self
    }

    /// Returns the raw controlled value retained by this render recipe.
    #[must_use]
    pub const fn value_ref(&self) -> f64 {
        self.value
    }

    /// Returns the raw `min` bound.
    #[must_use]
    pub const fn min_value(&self) -> f64 {
        self.min
    }

    /// Returns the raw `max` bound.
    #[must_use]
    pub const fn max_value(&self) -> f64 {
        self.max
    }

    /// Returns the raw `step`.
    #[must_use]
    pub const fn step_value(&self) -> f64 {
        self.step
    }

    /// Returns the effective ordered bounds after finite/reversed handling.
    #[must_use]
    pub fn effective_bounds(&self) -> (f64, f64) {
        effective_slider_bounds(self.min, self.max)
    }

    /// Returns the clamped and snapped value that will actually render.
    #[must_use]
    pub fn normalized_value(&self) -> f64 {
        normalize_slider_value(self.value, self.min, self.max, self.step)
    }

    /// Returns the fill/thumb fraction `0.0..=1.0` for the normalized value.
    #[must_use]
    pub fn fraction(&self) -> f32 {
        slider_fraction(self.normalized_value(), self.min, self.max)
    }

    /// Returns the next keyboard value for `key`, if the key is bound.
    #[must_use]
    pub fn next_value_for_key(&self, key: &str) -> Option<f64> {
        next_slider_value_for_key(key, self.value, self.min, self.max, self.step)
    }

    /// Returns whether this slider is disabled.
    #[must_use]
    pub const fn is_disabled(&self) -> bool {
        self.disabled
    }

    /// Returns the configured orientation.
    #[must_use]
    pub const fn orientation_value(&self) -> SliderOrientation {
        self.orientation
    }

    /// Returns the resolved theme and geometry recipe.
    #[must_use]
    pub fn visual_style(&self) -> SliderStyle {
        SliderStyle::resolve(self.theme)
    }

    /// Whether this slider should paint its focus ring now.
    #[must_use]
    pub fn focus_ring_visible(&self, window: &Window) -> bool {
        !self.disabled
            && self.focus_visibility == FocusVisibility::Visible
            && self.focus.is_focused(window)
    }
}

impl Styled for Slider {
    fn style(&mut self) -> &mut StyleRefinement {
        self.track.style()
    }
}

/// Shared interaction inputs for the slider hitbox wiring.
struct SliderInteraction {
    focus: FocusHandle,
    on_change: Option<ChangeHandler>,
    current_value: f64,
    min: f64,
    max: f64,
    step: f64,
    normalized: f64,
}

/// Wires keyboard and click activation onto the stateful hitbox.
fn wire_slider_interactions(
    hitbox: Stateful<Div>,
    interaction: &SliderInteraction,
) -> Stateful<Div> {
    // Keyboard handling: non-modified keys move by step/page/home/end.
    // The handler synthesizes a `ClickEvent` so the controlled callback
    // shape stays uniform with `switch`/`toggle_group`.
    let focus_for_key = interaction.focus.clone();
    let on_change_for_key = interaction.on_change.clone();
    let SliderInteraction {
        current_value,
        min,
        max,
        step,
        normalized,
        ..
    } = *interaction;
    let hitbox = hitbox.on_key_down(move |event: &KeyDownEvent, window, cx| {
        if event.keystroke.modifiers.modified() {
            return;
        }
        // `KeyDownEvent.keystroke.key` is the physical key name; match
        // lowercase to accept both `"ArrowRight"` and `"arrowright"`.
        let key = event.keystroke.key.to_ascii_lowercase();
        let next = next_slider_value_for_key(&key, current_value, min, max, step);
        let Some(next) = next else {
            return;
        };
        // Ensure the slider actually holds focus before emitting.
        if !focus_for_key.is_focused(window) {
            window.focus(&focus_for_key, cx);
        }
        window.prevent_default();
        cx.stop_propagation();
        if let Some(handler) = on_change_for_key.as_ref() {
            handler(next, &ClickEvent::default(), window, cx);
        }
    });

    // Pointer activation on the hitbox: treat a click as step-forward for
    // deterministic testing when the caller has not configured a drag
    // source. Disabled suppression is covered above; this just gives a
    // stable click-to-change path that the tests can assert is blocked
    // when disabled. Precise pixel-ratio dragging is deliberately not
    // synthesized here; callers that need it can compute
    // `normalize_slider_value` from their own geometry.
    let Some(handler) = interaction.on_change.clone() else {
        return hitbox;
    };
    let handler_for_click = Rc::clone(&handler);
    hitbox.on_click(move |event, window, cx| {
        // Advance by one step on click so the interaction is
        // observable in the controlled harness without inventing
        // layout-dependent pointer math.
        let next = next_slider_value_for_key("arrowright", current_value, min, max, step)
            .unwrap_or(normalized);
        handler_for_click(next, event, window, cx);
    })
}

/// Positions the thumb at the fractional leading edge with half-thumb centering.
fn position_slider_thumb(
    thumb: Div,
    orientation: SliderOrientation,
    fraction: f32,
    style: &SliderStyle,
) -> Div {
    // Thumb positioning: absolute leading edge at `fraction` with half-thumb
    // centering. For zero-span the thumb stays at the start.
    let thumb_half = px(f32::from(style.thumb_size) / 2.0);
    match orientation {
        SliderOrientation::Horizontal => thumb
            .absolute()
            .left(relative(fraction))
            .top(px(-(f32::from(style.thumb_size)
                - f32::from(style.track_thickness))
                / 2.0))
            .ml(-thumb_half),
        SliderOrientation::Vertical => thumb
            .absolute()
            .bottom(relative(fraction))
            .left(px(-(f32::from(style.thumb_size)
                - f32::from(style.track_thickness))
                / 2.0))
            .mb(-thumb_half),
    }
}

/// Applies the focus ring plumbing to the assembled track.
fn apply_slider_focus_ring(track: Div, focus: &FocusHandle, style: &SliderStyle) -> Div {
    let focus_border = style.focus_border_color;
    let focus_ring = style.focus_ring_color;
    let focus_ring_width = style.focus_ring_width;
    track
        .focus(move |focused| {
            focused.border_color(focus_border).shadow(vec![BoxShadow {
                color: focus_ring,
                offset: point(px(0.0), px(0.0)),
                blur_radius: px(0.0),
                spread_radius: focus_ring_width,
                inset: false,
            }])
        })
        .border_1()
        .border_color(transparent_black())
        // Keep the ring plumbing attached to the same focus handle the
        // hitbox tracks.
        .track_focus(focus)
}

impl RenderOnce for Slider {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let normalized = normalize_slider_value(self.value, self.min, self.max, self.step);
        let fraction = slider_fraction(normalized, self.min, self.max);
        let style = SliderStyle::resolve(self.theme);
        let disabled = self.disabled;
        let focus_visibility = self.focus_visibility;
        let focus = self.focus.clone();
        let on_change = self.on_change;
        let orientation = self.orientation;
        let min = self.min;
        let max = self.max;
        let step = self.step;
        let current_value = self.value;

        let mut track = self.track;
        let mut fill = self.fill;
        let mut thumb = self.thumb;

        // Apply orientation sizing: horizontal is full-width thin, vertical is
        // tall thin. Styled refinements on `track` may override these defaults
        // later via `StyleRefinement`.
        match orientation {
            SliderOrientation::Horizontal => {
                track = track.w_full().h(style.track_thickness);
                fill = fill.w(relative(fraction));
            }
            SliderOrientation::Vertical => {
                track = track.h_full().w(style.track_thickness).flex_col();
                fill = fill.w_full().h(relative(fraction));
            }
        }

        // Stable debug selectors for the sub-elements.
        if let Some(selector) = self.debug_selector.as_ref() {
            let selector = selector.clone();
            let track_selector = SharedString::from(format!("{selector}-track"));
            let range_selector = SharedString::from(format!("{selector}-range"));
            let thumb_selector = SharedString::from(format!("{selector}-thumb"));
            let track_sel = track_selector.clone();
            let range_sel = range_selector.clone();
            let thumb_sel = thumb_selector.clone();
            track = track.debug_selector(move || track_sel.to_string());
            fill = fill.debug_selector(move || range_sel.to_string());
            thumb = thumb.debug_selector(move || thumb_sel.to_string());
        }

        // Thumb positioning: absolute leading edge at `fraction` with half-thumb
        // centering. For zero-span the thumb stays at the start.
        thumb = position_slider_thumb(thumb, orientation, fraction, &style);

        // Track contains the fill clip and the absolute thumb. The fill is
        // clipped via `overflow_hidden` on the track itself.
        track = track.relative().child(fill).child(thumb);

        // Focus ring on the track when keyboard focus is visible.
        if !disabled && focus_visibility == FocusVisibility::Visible {
            track = apply_slider_focus_ring(track, &focus, &style);
        }

        let mut hitbox = div()
            .id(self.id)
            .flex()
            .items_center()
            .justify_center()
            .relative()
            .when(orientation == SliderOrientation::Horizontal, |e| {
                e.flex_row()
                    .w_full()
                    .h(style.thumb_size + px(8.0))
                    .py(px(4.0))
            })
            .when(orientation == SliderOrientation::Vertical, |e| {
                e.flex_col()
                    .h_full()
                    .w(style.thumb_size + px(8.0))
                    .px(px(4.0))
            });

        if let Some(selector) = self.debug_selector.clone() {
            let sel = selector.clone();
            hitbox = hitbox.debug_selector(move || sel.to_string());
        }

        if disabled {
            return hitbox
                .opacity(style.disabled_opacity)
                .child(track)
                .track_focus(&focus);
        }

        hitbox = hitbox.track_focus(&focus);

        let interaction = SliderInteraction {
            focus,
            on_change,
            current_value,
            min,
            max,
            step,
            normalized,
        };
        let hitbox = wire_slider_interactions(hitbox, &interaction);

        hitbox.child(track)
    }
}
