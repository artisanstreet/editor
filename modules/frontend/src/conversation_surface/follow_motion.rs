//! Programmatic transcript motion: the eased bottom glide, the scroll-range
//! hold, and scroll anchoring for rows above the reader.
//!
//! Three things used to move the viewport in single-frame steps. Follow and
//! the new-tail double snap jumped straight to the end during layout. The end
//! space below the last turn is measured in prepaint and applied a frame late,
//! so a last turn that shrank dropped the scroll range for one frame and GPUI
//! clamped the offset (a visible jump that follow then never undid). And rows
//! above the reader that changed measured height shifted everything under
//! them. This module owns the replacements:
//!
//! - every programmatic move to the end is one [`BottomGlide`] toward the
//!   *natural* end, re-read each frame;
//! - the transcript column never lays out shorter than the reader's current
//!   viewport bottom, so GPUI has nothing to clamp; a reader left past the
//!   natural end glides back instead;
//! - the column reads its rows' laid-out positions before prepaint and moves
//!   the offset by however far the reader's row moved, in the same frame.

use super::*;
use bottom_glide::BottomGlide;
use gpui::{
    App, Bounds, Element, GlobalElementId, InspectorElementId, LayoutId, Pixels, WeakEntity,
};
use std::panic;

/// Offsets further than this past the natural end count as "left in the
/// hold": sub-pixel differences are GPUI's two-decimal scroll rounding.
const HOLD_SLOP_PX: f32 = 0.5;

/// Anchor movement at or below this is layout rounding, never compensated, so
/// a steady frame never writes the offset.
const ANCHOR_SLOP_PX: f32 = 0.01;

/// Frames a landing may take before the transcript shows regardless: an end
/// that keeps moving (a reply streaming as the thread opens) is followed
/// with the normal glide from there.
const LANDING_FRAME_LIMIT: u8 = 8;

/// A surface opening onto its thread, before the transcript is in place.
///
/// A fresh surface lays out at the top, then moves to where the reader
/// belongs: the end, or the place they left. Gliding that distance shows the
/// whole transcript rushing past, so while landing every move to the end
/// snaps, and the transcript stays hidden until the first placement lands.
/// The end can still move for a frame or two as rows and the end space are
/// measured; landing ends once a snap finds it unchanged.
#[derive(Debug, Default)]
pub(super) struct Landing {
    /// Whether the first placement landed, so the transcript shows.
    shown: bool,
    /// The end the last snap landed on.
    last_end: Option<f32>,
    /// Frames rendered with content while landing.
    frames: u8,
}

/// The turn the reader's viewport top sits in, and where it was laid out.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct ScrollAnchorRow {
    /// Child index in the last prepaint; verified against `turn_id` first.
    pub(super) index: usize,
    pub(super) turn_id: TurnId,
    /// Top of the turn relative to the first transcript child, in px.
    pub(super) top: f32,
}

/// The natural scroll range once the latest end-space measurement lands.
///
/// `children_bounds` holds every transcript child with the end-space spacer
/// last; `end_space` is the spacer height the next frame lays out. The
/// column's minimum-height hold is deliberately absent: this is where the
/// reader's end really is, which the glide travels to and the controller
/// reports against. `None` without rows: the layout's own range stands.
#[must_use]
pub(super) fn natural_scroll_max(
    children_bounds: &[Bounds<Pixels>],
    viewport_height: f32,
    end_space: f32,
) -> Option<f32> {
    let (spacer, rows) = children_bounds.split_last()?;
    let first = rows.first()?;
    let content = f32::from(spacer.origin.y - first.origin.y) + TRANSCRIPT_PAD_TOP_PX + end_space;
    Some((content - viewport_height).max(0.0))
}

/// The first turn intersecting the viewport top, with its top relative to the
/// first turn. `None` when that is the first turn: nothing above it can move.
#[must_use]
pub(super) fn anchor_row(
    turn_bounds: &[Bounds<Pixels>],
    viewport_top: f32,
) -> Option<(usize, f32)> {
    let first = turn_bounds.first()?;
    let index = turn_bounds.partition_point(|bounds| f32::from(bounds.bottom()) <= viewport_top);
    let row = turn_bounds.get(index).filter(|_| index > 0)?;
    Some((index, f32::from(row.origin.y - first.origin.y)))
}

/// How far the anchored row moved since it was recorded, when that is more
/// than layout rounding.
#[must_use]
pub(super) fn anchor_delta(recorded_top: f32, laid_out_top: f32) -> Option<f32> {
    let delta = laid_out_top - recorded_top;
    (delta.abs() > ANCHOR_SLOP_PX).then_some(delta)
}

/// The column height that keeps a `scroll_top` offset inside the scroll range
/// of a `viewport_height` viewport.
#[must_use]
pub(super) fn scroll_hold_height(scroll_top: f32, viewport_height: f32) -> f32 {
    (scroll_top + viewport_height).max(0.0)
}

impl ConversationSurface {
    /// The scroll range the reader's end really has, without the hold.
    pub(super) fn natural_scroll_max(&self) -> f32 {
        self.natural_max_offset
            .unwrap_or_else(|| f32::from(self.scroll_handle.max_offset().y))
            .max(0.0)
    }

    /// Whether the reduced-motion preference (stored or system) applies to
    /// programmatic scrolling.
    pub(super) fn scroll_motion_reduced(&self, cx: &App) -> bool {
        effective_status_motion(self.status_motion, cx.reduce_motion()) == MotionPolicy::Reduced
    }

    /// Starts a glide to the natural end, or keeps the one in flight: it
    /// already re-reads the end every frame, so a second request has nothing
    /// to add and must not restart the motion.
    pub(super) fn start_bottom_glide(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.bottom_glide.is_none() {
            let current = f32::from(self.scroll_handle.offset().y);
            // A zero-length glide is a snap: a landing surface jumps.
            let snap = self.scroll_motion_reduced(cx) || self.landing.is_some();
            self.bottom_glide = Some(BottomGlide::new(current, snap));
        }
        self.schedule_bottom_glide_frame(window, cx);
    }

    /// Samples the glide now and writes its offset, notifying so the write
    /// gets this frame's render. Returns whether it is still travelling.
    pub(super) fn step_bottom_glide(&mut self, cx: &mut Context<Self>) -> bool {
        let target = -self.natural_scroll_max();
        let Some(glide) = self.bottom_glide.as_mut() else {
            return false;
        };
        let offset = self.scroll_handle.offset();
        let current = f32::from(offset.y);
        let frame = glide.sample(Instant::now(), current, target);
        #[expect(
            clippy::float_cmp,
            reason = "an unchanged offset is skipped exactly; an epsilon would drop the glide's last sub-pixel steps"
        )]
        let moved = frame.offset != current;
        if moved {
            self.scroll_handle
                .set_offset(point(offset.x, px(frame.offset)));
            cx.notify();
        }
        if !frame.settled {
            return true;
        }
        self.bottom_glide = None;
        if let Some(landing) = self.landing.as_mut() {
            landing.shown = true;
            if landing.last_end == Some(target) {
                self.landing = None;
            } else {
                // The end may still move as the first rows and the end
                // space are measured: snap again next frame until a snap
                // finds it where the last one left it.
                landing.last_end = Some(target);
                self.bottom_glide = Some(BottomGlide::new(frame.offset, true));
                cx.notify();
                return true;
            }
        }
        if self.smooth_bottom_active {
            // The jump settled at the end. Say so regardless of the last
            // report: the controller is in its scrolling state until it
            // hears the reader arrive, and a geometry observation may never
            // differ from what was reported before the jump began.
            self.smooth_bottom_active = false;
            self.report_reached_end(cx);
        }
        false
    }

    /// Schedules one glide frame while a glide is travelling.
    pub(super) fn schedule_bottom_glide_frame(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.bottom_glide.is_none() || self.bottom_glide_frame_scheduled {
            return;
        }
        self.bottom_glide_frame_scheduled = true;
        cx.on_next_frame(window, |surface, window, cx| {
            surface.advance_bottom_glide(window, cx);
        });
    }

    /// Applies one display frame of the glide, before that frame renders.
    pub(super) fn advance_bottom_glide(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.bottom_glide_frame_scheduled = false;
        if self.step_bottom_glide(cx) {
            self.schedule_bottom_glide_frame(window, cx);
        }
    }

    /// Stops an automatic glide because the reader took over.
    ///
    /// Returns whether it was a jump-to-latest, which the controller must
    /// hear was interrupted; a follow glide just ends.
    pub(super) fn cancel_bottom_glide(&mut self) -> bool {
        self.bottom_glide = None;
        self.landing = None;
        std::mem::take(&mut self.smooth_bottom_active)
    }

    /// Records this frame's natural end and glides a reader left past it.
    ///
    /// The column hold keeps the offset in range while content shrinks, so
    /// a reader can sit past the natural end for a frame. Nothing else
    /// would bring them back while they are not following, so the same
    /// glide does (or snaps, with reduced motion). A wheel glide in flight
    /// is the reader's own motion and finishes first.
    pub(super) fn observe_natural_end(
        &mut self,
        children_bounds: &[Bounds<Pixels>],
        end_space: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let viewport_height = f32::from(self.scroll_handle.bounds().size.height);
        self.natural_max_offset = natural_scroll_max(children_bounds, viewport_height, end_space);
        // Nothing overflows: the top is the end, so the opening is in place.
        if self.landing.is_some() && self.natural_max_offset == Some(0.0) {
            self.landing = None;
        }
        let scroll_top = -f32::from(self.scroll_handle.offset().y);
        if scroll_top > self.natural_scroll_max() + HOLD_SLOP_PX
            && self.bottom_glide.is_none()
            && !self.transcript_scroll.active()
        {
            self.start_bottom_glide(window, cx);
        }
    }

    /// Counts one rendered frame against the landing; returns whether the
    /// transcript stays hidden this frame.
    pub(super) fn note_landing_frame(&mut self) -> bool {
        if self.scene.turn_scenes().is_empty() {
            return false;
        }
        let Some(landing) = self.landing.as_mut() else {
            return false;
        };
        landing.frames = landing.frames.saturating_add(1);
        if landing.frames > LANDING_FRAME_LIMIT {
            self.landing = None;
            return false;
        }
        !landing.shown
    }

    /// Records the row under the viewport top for the next frame's anchoring.
    pub(super) fn record_scroll_anchor_row(&mut self, children_bounds: &[Bounds<Pixels>]) {
        let turns = self.scene.turn_scenes();
        let turn_bounds = &children_bounds[..turns.len().min(children_bounds.len())];
        let viewport_top = f32::from(self.scroll_handle.bounds().origin.y);
        let Some((index, top)) = anchor_row(turn_bounds, viewport_top) else {
            self.scroll_anchor_row = None;
            return;
        };
        let turn_id = &turns[index].turn_id;
        // A steady reader keeps the same row frame after frame: update its
        // position in place instead of cloning its identity every frame.
        match self.scroll_anchor_row.as_mut() {
            Some(row) if row.index == index && &row.turn_id == turn_id => row.top = top,
            _ => {
                self.scroll_anchor_row = Some(ScrollAnchorRow {
                    index,
                    turn_id: turn_id.clone(),
                    top,
                });
            }
        }
    }

    /// The current child index of the recorded anchor row, if it is still in
    /// the scene and is not the first row.
    pub(super) fn scroll_anchor_index(&self) -> Option<usize> {
        let row = self.scroll_anchor_row.as_ref()?;
        let turns = self.scene.turn_scenes();
        let index = if turns
            .get(row.index)
            .is_some_and(|turn| turn.turn_id == row.turn_id)
        {
            row.index
        } else {
            turns.iter().position(|turn| turn.turn_id == row.turn_id)?
        };
        (index > 0).then_some(index)
    }

    /// Moves the offset by however far the reader's row moved, returning the
    /// distance actually applied after clamping to the laid-out range.
    ///
    /// Glides in flight move with it: the content they travel over moved by
    /// the same amount, so their next frame continues from what is painted.
    pub(super) fn hold_scroll_anchor(&mut self, delta: f32) -> f32 {
        let offset = self.scroll_handle.offset();
        let maximum = f32::from(self.scroll_handle.max_offset().y).max(0.0);
        let current = f32::from(offset.y);
        let next = (current - delta).clamp(-maximum, 0.0);
        let applied = current - next;
        if applied.abs() <= ANCHOR_SLOP_PX {
            return 0.0;
        }
        self.scroll_handle.set_offset(point(offset.x, px(next)));
        if let Some(glide) = self.bottom_glide.as_mut() {
            glide.shift(-applied);
        }
        self.transcript_scroll.shift(-applied);
        // The prepaint listener re-measures both right after this; shifting
        // them too keeps a frame whose listener is skipped (a stale paint
        // token) from compensating the same movement twice.
        if let Some(natural) = self.natural_max_offset.as_mut() {
            *natural += applied;
        }
        if let Some(row) = self.scroll_anchor_row.as_mut() {
            row.top += applied;
        }
        applied
    }
}

/// Records one transcript child's layout node for the anchoring column.
///
/// Pass-through in every phase: the child keeps its own layout node, so
/// sibling geometry and every prepaint listener see exactly the same tree.
pub(super) struct LayoutProbe {
    child: AnyElement,
    slot: Rc<Cell<Option<LayoutId>>>,
}

impl LayoutProbe {
    pub(super) fn new(child: AnyElement, slot: Rc<Cell<Option<LayoutId>>>) -> Self {
        Self { child, slot }
    }
}

impl Element for LayoutProbe {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let layout_id = self.child.request_layout(window, cx);
        self.slot.set(Some(layout_id));
        (layout_id, ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        self.child.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.paint(window, cx);
    }
}

impl IntoElement for LayoutProbe {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

/// The probes one frame's anchoring reads, and where the anchor row was.
pub(super) struct AnchorProbes {
    pub(super) first: Rc<Cell<Option<LayoutId>>>,
    pub(super) anchor: Rc<Cell<Option<LayoutId>>>,
    pub(super) recorded_top: f32,
}

/// The transcript column with same-frame scroll anchoring.
///
/// Layout of the whole window is computed before anything is prepainted, so
/// this element (the scroll viewport's only child) can read where the
/// reader's row was laid out before its own children are positioned. When
/// rows above it changed height, it moves the scroll offset by that amount
/// and prepaints the column under the matching extra offset: the frame that
/// changed the heights already paints the reader's content where it was.
pub(super) struct AnchoredTranscript {
    child: AnyElement,
    surface: WeakEntity<ConversationSurface>,
    probes: Option<AnchorProbes>,
}

impl AnchoredTranscript {
    pub(super) fn new(
        child: AnyElement,
        surface: WeakEntity<ConversationSurface>,
        probes: Option<AnchorProbes>,
    ) -> Self {
        Self {
            child,
            surface,
            probes,
        }
    }
}

impl Element for AnchoredTranscript {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        (self.child.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        // Both probes resolve under the same element offset, so their
        // difference is a pure layout distance, independent of scrolling.
        let delta = self.probes.as_ref().and_then(|probes| {
            let first = window.layout_bounds(probes.first.get()?).origin.y;
            let anchor = window.layout_bounds(probes.anchor.get()?).origin.y;
            anchor_delta(probes.recorded_top, f32::from(anchor - first))
        });
        let applied = delta
            .and_then(|delta| {
                self.surface
                    .update(cx, |surface, _| surface.hold_scroll_anchor(delta))
                    .ok()
            })
            .unwrap_or(0.0);
        window.with_element_offset(point(px(0.0), px(-applied)), |window| {
            self.child.prepaint(window, cx);
        });
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.paint(window, cx);
    }
}

impl IntoElement for AnchoredTranscript {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::float_cmp,
        reason = "the geometry under test is exact pixel arithmetic"
    )]
    use super::*;
    use gpui::{point, size};

    fn row(top: f32, height: f32) -> Bounds<Pixels> {
        Bounds::new(point(px(0.0), px(top)), size(px(720.0), px(height)))
    }

    #[test]
    fn natural_range_uses_the_next_end_space_not_the_painted_one() {
        // Rows 40..640 then the spacer at 640 painted with a stale 192 px;
        // the next frame lays out 300 px.
        let children = [row(40.0, 300.0), row(372.0, 268.0), row(640.0, 192.0)];
        let max = natural_scroll_max(&children, 480.0, 300.0).expect("rows measure");
        // 600 px of rows + 40 px top pad + 300 px end space - 480 px viewport.
        assert_eq!(max, 460.0);
        assert_eq!(natural_scroll_max(&children[2..], 480.0, 300.0), None);
        assert_eq!(natural_scroll_max(&children, 4_000.0, 300.0), Some(0.0));
    }

    #[test]
    fn the_anchor_is_the_first_row_under_the_viewport_top() {
        let rows = [row(0.0, 100.0), row(132.0, 400.0), row(564.0, 80.0)];
        assert_eq!(
            anchor_row(&rows, 50.0),
            None,
            "the first row has nothing above it"
        );
        assert_eq!(
            anchor_row(&rows, 100.0),
            Some((1, 132.0)),
            "a row ending at the top is above it"
        );
        assert_eq!(anchor_row(&rows, 600.0), Some((2, 564.0)));
        assert_eq!(
            anchor_row(&rows, 9_000.0),
            None,
            "past every row nothing anchors"
        );
        assert_eq!(anchor_row(&[], 0.0), None);
    }

    #[test]
    fn anchor_delta_is_the_row_movement_beyond_rounding() {
        // A placeholder estimated at 160 px above the reader measured 260 px:
        // the reader's row moved down 100 px and the offset must follow.
        assert_eq!(anchor_delta(1_000.0, 1_100.0), Some(100.0));
        // A fold above the reader moved it up.
        assert_eq!(anchor_delta(1_000.0, 880.0), Some(-120.0));
        assert_eq!(anchor_delta(1_000.0, 1_000.005), None);
    }

    #[test]
    fn the_hold_keeps_the_current_offset_in_range() {
        // A reader at 900 px of a 480 px viewport keeps a 1380 px column:
        // GPUI's range is then at least 900 px and nothing is clamped when
        // the last turn shrinks this frame.
        assert_eq!(scroll_hold_height(900.0, 480.0), 1_380.0);
        assert_eq!(scroll_hold_height(0.0, 480.0), 480.0);
    }
}
