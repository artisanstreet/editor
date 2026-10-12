//! Following the live tail with an eased glide, as master's `BottomGlide`
//! did, instead of GPUI's tail mode, which snaps on every layout.
//!
//! The end keeps moving while a reply streams, so the glide is additive:
//! every change of the end adds one eased segment covering just that change,
//! from the moment it was seen. A segment starts at zero displacement, so
//! position never jumps, and the sum of all segments is exactly the latest
//! end, so the glide settles on it. Distances are in the list's scroll-top
//! space (pixels from the top, growing downward).

use editor_ui::motion::{MotionCurve, MotionDuration};
use gpui::{ListState, px};
use std::time::{Duration, Instant};

/// Ends closer than this are the same end (GPUI's two-decimal rounding).
const SAME_END_PX: f32 = 0.01;
/// The offset moved further than this from the glide's own last write:
/// someone else moved it (the reader, a jump), so the glide starts over.
const EXTERNAL_WRITE_PX: f32 = 0.5;
/// Past the bound, a change folds into the youngest segment.
const MAX_SEGMENTS: usize = 64;

#[derive(Clone, Copy, Debug)]
struct Segment {
    started: Instant,
    distance: f32,
}

#[derive(Debug, Default)]
pub(crate) struct Follow {
    /// What retired segments already contributed.
    base: f32,
    segments: Vec<Segment>,
    /// The scroll top this glide last wrote, while one runs.
    written: Option<f32>,
    /// The next move to the end lands at once: the thread just opened.
    snap: bool,
}

impl Follow {
    /// The next placement at the end snaps rather than glides: a thread
    /// opening onto its end shouldn't show the transcript rushing past.
    pub(crate) fn snap_next(&mut self) {
        self.snap = true;
        self.stop();
    }

    /// Whether this move to the end snaps; it clears the request.
    pub(crate) fn take_snap(&mut self) -> bool {
        std::mem::take(&mut self.snap)
    }

    /// No glide is running.
    pub(crate) fn idle(&self) -> bool {
        self.segments.is_empty()
    }

    pub(crate) fn stop(&mut self) {
        self.segments.clear();
        self.written = None;
    }

    fn end(&self) -> f32 {
        self.base + self.segments.iter().map(|s| s.distance).sum::<f32>()
    }

    /// Where the glide is at `now`, retiring finished segments into `base`.
    fn sample(&mut self, now: Instant, duration: Duration) -> f32 {
        let mut position = self.base;
        let mut retired = 0.0;
        self.segments.retain(|segment| {
            let elapsed = now.saturating_duration_since(segment.started);
            if duration.is_zero() || elapsed >= duration {
                retired += segment.distance;
                return false;
            }
            let progress = elapsed.as_secs_f64() / duration.as_secs_f64();
            #[expect(
                clippy::cast_possible_truncation,
                reason = "the curve samples in f64 and feeds f32 offsets"
            )]
            let eased = MotionCurve::SmoothOut.sample(progress) as f32;
            position += segment.distance * eased;
            true
        });
        self.base += retired;
        position + retired
    }

    /// Moves the list one frame toward its end while the reader follows;
    /// returns whether another frame is needed. Run after the list's layout
    /// (in paint), so the end includes this frame's growth.
    pub(crate) fn drive(&mut self, list: &ListState, following: bool, reduce_motion: bool) -> bool {
        if !following {
            self.stop();
            return false;
        }
        let end = f32::from(list.max_offset_for_scrollbar().y).max(0.0);
        let raw = -f32::from(list.scroll_px_offset_for_scrollbar().y);
        if raw > end + EXTERNAL_WRITE_PX {
            // The opening snap (`scroll_to_end`) leaves the top past the
            // end; pin it there as a concrete offset to glide on from.
            list.scroll_by(px(end - raw));
            self.stop();
            return false;
        }
        let current = raw.max(0.0);
        let now = Instant::now();
        if self
            .written
            .is_some_and(|written| (written - current).abs() > EXTERNAL_WRITE_PX)
        {
            self.stop();
        }
        if self.written.is_none() {
            if (end - current).abs() <= EXTERNAL_WRITE_PX {
                return false;
            }
            self.base = current;
        }
        let change = end - self.end();
        if change.abs() > SAME_END_PX {
            if self.segments.len() >= MAX_SEGMENTS
                && let Some(last) = self.segments.last_mut()
            {
                last.distance += change;
            } else {
                self.segments.push(Segment {
                    started: now,
                    distance: change,
                });
            }
        }
        let duration = if reduce_motion {
            Duration::ZERO
        } else {
            MotionDuration::Fast.as_duration()
        };
        let next = self.sample(now, duration).clamp(0.0, end);
        list.scroll_by(px(next - current));
        if self.segments.is_empty() {
            self.stop();
            return false;
        }
        self.written = Some(next);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_moving_end_glides_continuously_and_settles_on_the_latest_end() {
        let mut follow = Follow::default();
        let start = Instant::now();
        let duration = Duration::from_millis(250);
        follow.base = 0.0;
        follow.segments.push(Segment {
            started: start,
            distance: 100.0,
        });
        let early = follow.sample(start + Duration::from_millis(50), duration);
        assert!(early > 0.0 && early < 100.0, "it eases, it doesn't jump");
        // The reply grows mid-glide: one more segment, no jump back.
        follow.segments.push(Segment {
            started: start + Duration::from_millis(50),
            distance: 40.0,
        });
        let later = follow.sample(start + Duration::from_millis(60), duration);
        assert!(later >= early);
        let settled = follow.sample(start + Duration::from_secs(1), duration);
        assert!((settled - 140.0).abs() < 1e-3);
        assert!(follow.segments.is_empty());
    }
}
