//! The picker surfaces' motion (`native_picker_motion` and the paint helpers
//! of `native_model_selector/render`): the shared hover pill that flies from
//! row to row, and the open and close of a popover that stays mounted until
//! its exit has played.
//!
//! Geometry and lifecycle live apart from the paint closures. A pill's
//! interrupted flight starts from the rectangle it displays, and a popover
//! reversed mid-flight starts from the opacity and offset it shows, never
//! from a fixed first frame.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use gpui::{
    Animation, AnimationExt as _, AnyElement, Div, ElementId, InteractiveElement as _, IntoElement,
    Pixels, Point, Stateful, Styled as _, div, px,
};

use crate::gradient::hover_fill_gradient;
use crate::hover_card::hover_highlight_shadow;
use crate::motion::MotionCurve;
use crate::theme::ArtisanTheme;

/// The pill's row-to-row flight.
pub const HOVER_FLIGHT: Duration = Duration::from_millis(250);
/// `--dropdown-pre-scale`: where an opening dropdown starts.
const DROPDOWN_PRE_SCALE: f32 = 0.97;
/// `--dropdown-closing-scale`: where a closing dropdown ends.
const DROPDOWN_CLOSING_SCALE: f32 = 0.99;
/// How far a hidden popover sits from its place.
const HIDDEN_OFFSET: f32 = 8.0;

/// `cubic-bezier(0.22, 1, 0.36, 1)` as a GPUI easing.
#[must_use]
pub fn smooth_out(progress: f32) -> f32 {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the curve samples in f64 and feeds f32 paint values"
    )]
    let value = MotionCurve::SmoothOut.sample(f64::from(progress)) as f32;
    value
}

/// A measured pill rectangle in its surface's coordinates.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HoverRect {
    pub left: f32,
    pub top: f32,
    pub width: f32,
    pub height: f32,
}

impl HoverRect {
    /// Every component moved by the same progress.
    #[must_use]
    pub fn lerp(self, target: Self, progress: f32) -> Self {
        let progress = progress.clamp(0.0, 1.0);
        Self {
            left: self.left + (target.left - self.left) * progress,
            top: self.top + (target.top - self.top) * progress,
            width: self.width + (target.width - self.width) * progress,
            height: self.height + (target.height - self.height) * progress,
        }
    }

    /// `bounds` relative to the surface at `surface`.
    #[must_use]
    pub fn within(bounds: gpui::Bounds<Pixels>, surface: gpui::Bounds<Pixels>) -> Self {
        Self {
            left: f32::from(bounds.left() - surface.left()),
            top: f32::from(bounds.top() - surface.top()),
            width: f32::from(bounds.size.width),
            height: f32::from(bounds.size.height),
        }
    }
}

/// One pill flight.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HoverTransition {
    pub from: HoverRect,
    pub to: HoverRect,
    /// Retires the paint closure of an interrupted flight.
    pub generation: u64,
}

/// One shared hover pill.
#[derive(Clone, Debug, Default)]
pub struct SlidingHoverState {
    active_id: Option<String>,
    target_id: Option<String>,
    current: HoverRect,
    from: HoverRect,
    target: HoverRect,
    visible: bool,
    has_geometry: bool,
    animating: bool,
    generation: u64,
}

impl SlidingHoverState {
    #[must_use]
    pub fn active_id(&self) -> Option<&str> {
        self.active_id.as_deref()
    }

    #[must_use]
    pub fn visual_rect(&self) -> HoverRect {
        self.current
    }

    #[must_use]
    pub const fn visible(&self) -> bool {
        self.visible
    }

    /// Makes `id` the row the pill goes to; its probe supplies the place.
    pub fn set_active(&mut self, id: String) {
        if self.active_id.as_deref() != Some(id.as_str()) {
            self.active_id = Some(id);
        }
        self.visible = true;
    }

    /// Drops the target; the next one is placed, not flown to.
    pub fn clear(&mut self) {
        self.active_id = None;
        self.target_id = None;
        self.visible = false;
        self.has_geometry = false;
        self.animating = false;
        self.generation = self.generation.wrapping_add(1);
    }

    /// Hides the pill but keeps its place, so the next row flies from it.
    pub fn hide(&mut self) {
        self.active_id = None;
        self.visible = false;
    }

    /// Clears a target that left the list.
    pub fn clear_if_missing(&mut self, visible_ids: &[String]) {
        if self
            .active_id
            .as_ref()
            .is_some_and(|active| !visible_ids.iter().any(|id| id == active))
        {
            self.clear();
        }
    }

    /// Records a row's measured place. A new row flies from the displayed
    /// rectangle; the same row measured again (resize, scroll) moves at
    /// once. Returns whether anything changed.
    pub fn measure(&mut self, id: &str, rect: HoverRect) -> bool {
        if self.active_id.as_deref() != Some(id) {
            return false;
        }
        if self.target_id.as_deref() == Some(id) && self.target == rect && self.visible {
            return false;
        }
        let switch_target = self.target_id.as_deref() != Some(id);
        let animate = self.visible && self.has_geometry && switch_target;
        self.target_id = Some(id.to_owned());
        self.visible = true;
        self.has_geometry = true;
        self.from = self.current;
        self.target = rect;
        self.generation = self.generation.wrapping_add(1);
        if animate {
            self.animating = true;
        } else {
            self.current = rect;
            self.animating = false;
        }
        true
    }

    #[must_use]
    pub fn transition(&self) -> Option<HoverTransition> {
        self.animating.then_some(HoverTransition {
            from: self.from,
            to: self.target,
            generation: self.generation,
        })
    }

    /// Applies one sample, ignoring an interrupted flight's closure.
    pub fn apply_progress(&mut self, generation: u64, progress: f32) {
        if !self.animating || generation != self.generation {
            return;
        }
        self.current = self.from.lerp(self.target, progress);
        if progress >= 1.0 {
            self.current = self.target;
            self.animating = false;
        }
    }
}

/// Paints the shared pill behind a surface's rows: the hover fill, with the
/// highlight shadow when `shadow`, flying over [`HOVER_FLIGHT`] on the
/// smooth-out curve. Reduced motion jumps to the target.
#[must_use]
pub fn sliding_hover_pill(
    id: &str,
    theme: &ArtisanTheme,
    hover: &Rc<RefCell<SlidingHoverState>>,
    radius: Pixels,
    shadow: bool,
    reduce_motion: bool,
) -> AnyElement {
    let (mut rect, visible, transition) = {
        let hover = hover.borrow();
        (hover.visual_rect(), hover.visible(), hover.transition())
    };
    if reduce_motion && let Some(transition) = transition {
        rect = transition.to;
        hover
            .borrow_mut()
            .apply_progress(transition.generation, 1.0);
    }
    let mut pill = div()
        .id(ElementId::Name(id.to_owned().into()))
        .absolute()
        .left(px(rect.left))
        .top(px(rect.top))
        .w(px(rect.width))
        .h(px(rect.height))
        .rounded(radius)
        .bg(hover_fill_gradient(*theme))
        .opacity(if visible { 1.0 } else { 0.0 });
    if shadow {
        pill = pill.shadow(hover_highlight_shadow(theme));
    }
    if !reduce_motion && let Some(transition) = transition {
        let motion = Rc::clone(hover);
        let HoverTransition {
            from,
            to,
            generation,
        } = transition;
        return pill
            .with_animation(
                ElementId::Name(format!("{id}-{generation}").into()),
                Animation::new(HOVER_FLIGHT).with_easing(smooth_out),
                move |pill, progress| {
                    let rect = from.lerp(to, progress);
                    motion.borrow_mut().apply_progress(generation, progress);
                    pill.left(px(rect.left))
                        .top(px(rect.top))
                        .w(px(rect.width))
                        .h(px(rect.height))
                },
            )
            .into_any_element();
    }
    pill.into_any_element()
}

/// A popover's lifecycle. `Closing` stays painted until its exit settles.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum MenuPhase {
    #[default]
    Hidden,
    Opening,
    Open,
    Closing,
}

/// A popover's retained opacity and offset.
#[derive(Clone, Copy, Debug)]
pub struct MenuMotion {
    phase: MenuPhase,
    current_opacity: f32,
    current_offset: f32,
    from_opacity: f32,
    from_offset: f32,
    to_opacity: f32,
    to_offset: f32,
    generation: u64,
}

impl Default for MenuMotion {
    fn default() -> Self {
        Self {
            phase: MenuPhase::Hidden,
            current_opacity: 0.0,
            current_offset: HIDDEN_OFFSET,
            from_opacity: 0.0,
            from_offset: HIDDEN_OFFSET,
            to_opacity: 0.0,
            to_offset: HIDDEN_OFFSET,
            generation: 0,
        }
    }
}

impl MenuMotion {
    #[must_use]
    pub const fn phase(self) -> MenuPhase {
        self.phase
    }

    /// Whether the popover is painted (open, or playing its entrance or
    /// exit).
    #[must_use]
    pub fn painted(self) -> bool {
        self.phase != MenuPhase::Hidden
    }

    /// `(from_opacity, from_offset, to_opacity, to_offset, generation)`
    /// while a flight runs.
    #[must_use]
    pub const fn transition(self) -> Option<(f32, f32, f32, f32, u64)> {
        match self.phase {
            MenuPhase::Opening | MenuPhase::Closing => Some((
                self.from_opacity,
                self.from_offset,
                self.to_opacity,
                self.to_offset,
                self.generation,
            )),
            MenuPhase::Hidden | MenuPhase::Open => None,
        }
    }

    /// Opens, or reverses a close, from the displayed frame.
    pub fn begin_open(&mut self) -> u64 {
        self.from_opacity = self.current_opacity;
        self.from_offset = self.current_offset;
        self.to_opacity = 1.0;
        self.to_offset = 0.0;
        self.phase = MenuPhase::Opening;
        self.generation = self.generation.wrapping_add(1);
        self.generation
    }

    /// Closes, or reverses an open, from the displayed frame.
    pub fn begin_close(&mut self) -> u64 {
        self.from_opacity = self.current_opacity;
        self.from_offset = self.current_offset;
        self.to_opacity = 0.0;
        self.to_offset = HIDDEN_OFFSET;
        self.phase = MenuPhase::Closing;
        self.generation = self.generation.wrapping_add(1);
        self.generation
    }

    pub fn apply_progress(&mut self, generation: u64, progress: f32) {
        if generation != self.generation {
            return;
        }
        let progress = progress.clamp(0.0, 1.0);
        self.current_opacity = self.from_opacity + (self.to_opacity - self.from_opacity) * progress;
        self.current_offset = self.from_offset + (self.to_offset - self.from_offset) * progress;
    }

    /// Settles an open nothing newer replaced.
    pub fn finish_open(&mut self, generation: u64) -> bool {
        if self.phase != MenuPhase::Opening || generation != self.generation {
            return false;
        }
        self.current_opacity = 1.0;
        self.current_offset = 0.0;
        self.phase = MenuPhase::Open;
        true
    }

    /// Drops a closed popover nothing newer replaced.
    pub fn finish_close(&mut self, generation: u64) -> bool {
        if self.phase != MenuPhase::Closing || generation != self.generation {
            return false;
        }
        self.current_opacity = 0.0;
        self.current_offset = HIDDEN_OFFSET;
        self.phase = MenuPhase::Hidden;
        true
    }

    /// Forgets the popover at once.
    pub fn hide(&mut self) {
        *self = Self {
            generation: self.generation.wrapping_add(1),
            ..Self::default()
        };
    }
}

/// How a popover moves while it fades.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MenuEntrance {
    /// Slides 8 px toward its trigger (the picker's own recipe).
    Slide,
    /// The transitions-dev dropdown: grows from 97% about `origin` (a
    /// fraction of its bounds) on open and settles to 99% on close.
    Scale { origin: Point<f32> },
}

/// Paints `panel` at its motion's frame (`animate_picker_menu_timed`): the
/// flight on the smooth-out curve over `open` or `close`, or at rest.
#[must_use]
pub fn animate_menu(
    panel: Stateful<Div>,
    motion: &Rc<RefCell<MenuMotion>>,
    surface: &str,
    entrance: MenuEntrance,
    open: Duration,
    close: Duration,
) -> AnyElement {
    let snapshot = *motion.borrow();
    let Some((from_opacity, from_offset, to_opacity, to_offset, generation)) =
        snapshot.transition()
    else {
        return panel.into_any_element();
    };
    let phase = snapshot.phase();
    let closing = phase == MenuPhase::Closing;
    let place = move |panel: Stateful<Div>, offset: f32, opacity: f32| match entrance {
        MenuEntrance::Slide => panel.top(px(offset)).opacity(opacity),
        MenuEntrance::Scale { origin } => {
            let presence = 1.0 - (offset / HIDDEN_OFFSET).clamp(0.0, 1.0);
            let rest = if closing {
                DROPDOWN_CLOSING_SCALE
            } else {
                DROPDOWN_PRE_SCALE
            };
            panel.opacity(opacity).transform(
                gpui::ElementTransform::scale(rest + (1.0 - rest) * presence).with_origin(origin),
            )
        }
    };
    let motion = Rc::clone(motion);
    place(panel, from_offset, from_opacity)
        .with_animation(
            ElementId::Name(
                format!(
                    "{surface}-{}-{generation}",
                    if closing { "closing" } else { "opening" }
                )
                .into(),
            ),
            Animation::new(if closing { close } else { open }).with_easing(smooth_out),
            move |panel, progress| {
                motion.borrow_mut().apply_progress(generation, progress);
                place(
                    panel,
                    from_offset + (to_offset - from_offset) * progress,
                    from_opacity + (to_opacity - from_opacity) * progress,
                )
            },
        )
        .into_any_element()
}
