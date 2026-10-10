//! Where a reader left a transcript, so reopening its thread puts them back.
//!
//! A thread switch retires the surface, and a fresh surface opens at the
//! transcript end. The application keeps one [`TranscriptReadingPosition`]
//! per thread instead: the turn under the viewport top, how far into it the
//! viewport sat, and the row heights that placed it there. Placeholders above
//! the reader stand in at those heights again, so the restored offset lands
//! on exactly the content the reader left. A reader at the end keeps no
//! position: the end is where a thread opens anyway.

use super::*;

/// The reader's place in a transcript they left.
#[derive(Clone, Debug, PartialEq)]
pub struct TranscriptReadingPosition {
    /// The turn under the viewport top.
    turn_id: TurnId,
    /// How far below that turn's top the viewport top sat, in px.
    depth: f64,
    /// Measured row heights by turn identity, as they were painted.
    heights: HashMap<String, f64>,
}

impl TranscriptReadingPosition {
    /// A position at the top of `turn_id` with nothing measured.
    #[cfg(test)]
    pub(crate) fn at_turn(turn_id: TurnId) -> Self {
        Self {
            turn_id,
            depth: 0.0,
            heights: HashMap::new(),
        }
    }
}

/// A restore waiting for the viewport's first laid-out frame.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct PendingReadingPosition {
    turn_id: TurnId,
    depth: f64,
}

impl ConversationSurface {
    /// Where the reader is, unless they are at the transcript end.
    ///
    /// A restore that has not reached the screen yet is still where the
    /// reader is: leaving again before it paints keeps it.
    #[must_use]
    pub fn reading_position(&self) -> Option<TranscriptReadingPosition> {
        let heights = || self.transcript_window.borrow().measured_heights();
        if let Some(pending) = &self.pending_reading_position {
            return Some(TranscriptReadingPosition {
                turn_id: pending.turn_id.clone(),
                depth: pending.depth,
                heights: heights(),
            });
        }
        let viewport_height = f64::from(self.scroll_handle.bounds().size.height);
        if viewport_height <= 0.0 {
            return None;
        }
        let scroll_top = -f64::from(self.scroll_handle.offset().y);
        let scroll_height = viewport_height + f64::from(self.natural_scroll_max());
        if conversation_is_following(scroll_top, scroll_height, viewport_height) {
            return None;
        }
        let (index, depth) = self
            .transcript_window
            .borrow()
            .turn_at(scroll_top - f64::from(TRANSCRIPT_PAD_TOP_PX))?;
        let turn = self.scene.turn_scenes().get(index)?;
        Some(TranscriptReadingPosition {
            turn_id: turn.turn_id.clone(),
            depth,
            heights: heights(),
        })
    }

    /// Puts the reader back where they left this transcript.
    ///
    /// Returns `false`, changing nothing, when the position's turn is not in
    /// the accepted scene. Otherwise the reader is detached from the end
    /// exactly as if they had scrolled there, any automatic move to the end
    /// is dropped, and the next laid-out frame writes the offset.
    pub fn restore_reading_position(
        &mut self,
        position: TranscriptReadingPosition,
        cx: &mut Context<Self>,
    ) -> bool {
        let known = self
            .scene
            .turn_scenes()
            .iter()
            .any(|turn| turn.turn_id == position.turn_id);
        if !known {
            return false;
        }
        self.transcript_window
            .borrow_mut()
            .seed(&self.scene, &position.heights);
        self.pending_reading_position = Some(PendingReadingPosition {
            turn_id: position.turn_id,
            depth: position.depth,
        });
        self.drop_automatic_scroll();
        let _ = self.observe_viewport(
            ViewportObservation {
                first_visible: None,
                last_visible: None,
                at_bottom: false,
            },
            cx,
        );
        cx.notify();
        true
    }

    /// Writes a pending restore's offset once the viewport has a size.
    ///
    /// The column hold keeps any offset inside the scroll range only once
    /// the viewport height is known; before that, GPUI would clamp the
    /// offset short of the reader's place.
    pub(super) fn apply_pending_reading_position(&mut self, window: &mut Window) {
        let Some(pending) = self.pending_reading_position.take() else {
            return;
        };
        if self.scroll_handle.bounds().size.height <= px(0.0) {
            self.pending_reading_position = Some(pending);
            window.request_animation_frame();
            return;
        }
        let Some(index) = self
            .scene
            .turn_scenes()
            .iter()
            .position(|turn| turn.turn_id == pending.turn_id)
        else {
            return;
        };
        let Some(top) = self.transcript_window.borrow().top_of(index) else {
            return;
        };
        let scroll_top = f64::from(TRANSCRIPT_PAD_TOP_PX) + top + pending.depth;
        #[expect(
            clippy::cast_possible_truncation,
            reason = "planned heights originate from GPUI's f32 pixel bounds"
        )]
        let offset = -(scroll_top.max(0.0) as f32);
        self.drop_automatic_scroll();
        let current = self.scroll_handle.offset();
        self.scroll_handle.set_offset(point(current.x, px(offset)));
        self.transcript_scroll.cancel_to(offset, -offset);
        // The reader is back where they left: the opening has landed.
        self.landing = None;
    }

    /// Drops every automatic move toward the end, and the anchor recorded
    /// against the previous offset.
    fn drop_automatic_scroll(&mut self) {
        self.follow_bottom_pending = false;
        self.smooth_bottom_pending = false;
        self.cancel_bottom_glide();
        self.scroll_anchor_row = None;
        let current = f32::from(self.scroll_handle.offset().y);
        let maximum = f32::from(self.scroll_handle.max_offset().y).max(0.0);
        self.transcript_scroll.cancel_to(current, maximum);
    }
}
