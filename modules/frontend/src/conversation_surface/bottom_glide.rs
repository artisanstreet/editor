//! Retargetable eased glide of the transcript offset toward its end.
//!
//! Automatic follow, jump-to-latest and the settle after content shrank under
//! the reader all move the viewport programmatically. That is a position
//! change, so it travels on the motion scale's page-slide tokens: 250 ms
//! ([`MotionDuration::Fast`]) on `cubic-bezier(0.22, 1, 0.36, 1)`
//! ([`MotionCurve::SmoothOut`]).
//!
//! The end keeps moving while a reply streams, so a plain tween would either
//! restart every frame (a stutter of re-accelerations) or arrive somewhere
//! stale. The glide is additive instead, the way interruptible native motion
//! composes: every change of the end adds one more eased segment covering just
//! that change, starting at the moment it was seen. A segment starts at zero
//! displacement, so position never jumps; its starting velocity is
//! proportional to its own small distance, so a streaming retarget reads as one
//! continuous glide. The sum of all segments is exactly the latest end, so the
//! glide settles on it with no residue.

use std::time::{Duration, Instant};

use artisan_ui::motion::{MotionCurve, MotionDuration};

/// Ends closer than this are the same end: measured geometry carries GPUI's
/// two-decimal rounding, and a sub-hundredth retarget is not motion.
const SAME_END_PX: f32 = 0.01;

/// An offset further than this from the glide's own last write was moved by
/// someone else (a clamp, a prepend hold, anchoring); smaller differences are
/// GPUI's pixel snapping and stay owned by the glide.
const EXTERNAL_WRITE_PX: f32 = 0.5;

/// Upper bound on concurrently easing segments.
///
/// Segments retire after one duration, so a retarget per display frame keeps
/// at most a frame rate's quarter-second worth alive; the bound only guards a
/// pathological refresh rate. Past it a change folds into the youngest
/// segment, which is still near its start, so the merge is invisible.
const MAX_SEGMENTS: usize = 64;

/// One eased share of the travel: the end moved by `distance` at `started`.
#[derive(Clone, Copy, Debug, PartialEq)]
struct GlideSegment {
    started: Instant,
    distance: f32,
}

/// What one sampled frame of the glide asks the viewport to do.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct GlideFrame {
    /// The offset to paint this frame.
    pub(super) offset: f32,
    /// The glide reached its end and the end held still for a whole frame:
    /// nothing more will move unless a new glide starts.
    pub(super) settled: bool,
}

/// Additive eased glide of one scroll offset toward a moving end.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct BottomGlide {
    /// The offset every retired segment has already contributed to.
    base: f32,
    /// Segments still easing, oldest first.
    segments: Vec<GlideSegment>,
    duration: Duration,
    /// The offset this glide last asked the viewport to paint.
    last_written: f32,
    /// When the glide was asked for, which its first segment starts at.
    ///
    /// A glide is requested in a render and first sampled on the display
    /// frame after it; starting the clock at the request keeps that first
    /// frame moving instead of spending it at zero progress.
    requested: Option<Instant>,
}

impl BottomGlide {
    /// Starts a glide from the painted offset on the page-slide clock, or a
    /// zero-length clock that snaps when motion is reduced.
    pub(super) fn new(current: f32, reduce_motion: bool) -> Self {
        let duration = if reduce_motion {
            Duration::ZERO
        } else {
            MotionDuration::Fast.as_duration()
        };
        Self {
            requested: Some(Instant::now()),
            ..Self::with_duration(current, duration)
        }
    }

    /// A glide on an explicit clock whose first segment starts when it is
    /// first sampled.
    pub(super) fn with_duration(current: f32, duration: Duration) -> Self {
        Self {
            base: current,
            segments: Vec::new(),
            duration,
            last_written: current,
            requested: None,
        }
    }

    /// The end the glide is currently travelling to.
    pub(super) fn end(&self) -> f32 {
        self.base
            + self
                .segments
                .iter()
                .map(|segment| segment.distance)
                .sum::<f32>()
    }

    /// Moves the whole glide by an external offset change, keeping its end.
    ///
    /// Content inserted or measured above the reader moves the painted offset
    /// and the end by the same amount; shifting here keeps the next frame
    /// continuous with what is on screen instead of easing the reader back.
    pub(super) fn shift(&mut self, delta: f32) {
        self.base += delta;
        self.last_written += delta;
    }

    /// Samples one frame at `now` toward `target`, given the painted
    /// `current` offset.
    pub(super) fn sample(&mut self, now: Instant, current: f32, target: f32) -> GlideFrame {
        // Someone else moved the viewport (a clamp, a hold release): adopt
        // the painted position so this frame continues from it. The end is
        // re-derived below, so the target is still reached exactly.
        if (current - self.last_written).abs() > EXTERNAL_WRITE_PX {
            self.base += current - self.last_written;
        }
        // Only the first sample may start at the request: a change seen
        // later is new motion and starts when it is seen.
        let requested = self.requested.take();
        let duration = self.duration;
        let mut retired = 0.0_f32;
        self.segments.retain(|segment| {
            let done = now.saturating_duration_since(segment.started) >= duration;
            if done {
                retired += segment.distance;
            }
            !done
        });
        self.base += retired;
        let change = target - self.end();
        let retargeted = change.abs() > SAME_END_PX;
        if retargeted {
            if duration.is_zero() {
                self.base += change;
            } else if self.segments.len() >= MAX_SEGMENTS {
                if let Some(youngest) = self.segments.last_mut() {
                    youngest.distance += change;
                }
            } else {
                let started = requested.map_or(now, |requested| requested.min(now));
                self.segments.push(GlideSegment {
                    started,
                    distance: change,
                });
            }
        }
        if self.segments.is_empty() {
            // Land exactly on the end: accumulated float error stays below
            // the same-end tolerance and is dropped here, never painted.
            self.base = target;
            self.last_written = target;
            return GlideFrame {
                offset: target,
                settled: !retargeted,
            };
        }
        let mut offset = self.base;
        for segment in &self.segments {
            let progress = now.saturating_duration_since(segment.started).as_secs_f64()
                / duration.as_secs_f64();
            #[expect(
                clippy::cast_possible_truncation,
                reason = "motion curves feed f32 layout values"
            )]
            let eased = MotionCurve::SmoothOut.sample(progress) as f32;
            offset += segment.distance * eased;
        }
        self.last_written = offset;
        GlideFrame {
            offset,
            settled: false,
        }
    }
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::float_cmp,
        reason = "settling is asserted bit-exact: the glide must land on the target, not near it"
    )]
    use super::*;

    const FAST: Duration = Duration::from_millis(250);

    fn frames(start: Instant, count: u32) -> impl Iterator<Item = Instant> {
        (0..=count).map(move |frame| start + Duration::from_millis(u64::from(frame) * 16))
    }

    #[test]
    fn page_slide_tokens_drive_full_motion() {
        let glide = BottomGlide::new(0.0, false);
        assert_eq!(glide.duration, FAST);
        assert_eq!(BottomGlide::new(0.0, true).duration, Duration::ZERO);
    }

    #[test]
    fn the_first_frame_after_the_request_already_moves() {
        let mut glide = BottomGlide::new(0.0, false);
        let requested = glide.requested.expect("a requested glide records when");
        let first = glide.sample(requested + Duration::from_millis(16), 0.0, -300.0);
        assert!(
            first.offset < 0.0 && first.offset > -300.0,
            "the frame after the request is already under way, got {}",
            first.offset
        );
    }

    #[test]
    fn a_fixed_end_is_reached_monotonically_and_exactly() {
        let start = Instant::now();
        let mut glide = BottomGlide::with_duration(0.0, FAST);
        let mut current = 0.0;
        let mut settled_at = None;
        for (frame, now) in frames(start, 40).enumerate() {
            let step = glide.sample(now, current, -600.0);
            assert!(step.offset <= current, "the glide never backs up");
            assert!(step.offset >= -600.0, "the glide never overshoots");
            current = step.offset;
            if step.settled {
                settled_at = Some(frame);
                break;
            }
        }
        assert_eq!(current, -600.0, "the glide lands exactly on its end");
        let settled_at = settled_at.expect("a fixed end settles");
        assert!(
            (16..=18).contains(&settled_at),
            "250 ms of 16 ms frames plus the confirming frame, got {settled_at}"
        );
    }

    #[test]
    fn the_first_frame_moves_and_smooth_out_front_loads_travel() {
        let start = Instant::now();
        let mut glide = BottomGlide::with_duration(0.0, FAST);
        assert_eq!(glide.sample(start, 0.0, -100.0).offset, 0.0);
        let half = glide.sample(start + FAST / 2, 0.0, -100.0).offset;
        assert!(
            half < -80.0,
            "smooth-out covers most of the travel by half time, got {half}"
        );
    }

    #[test]
    fn retargeting_mid_flight_never_jumps_and_lands_on_the_new_end() {
        let start = Instant::now();
        let mut glide = BottomGlide::with_duration(0.0, FAST);
        let mut current = glide.sample(start, 0.0, -300.0).offset;
        let mid = start + Duration::from_millis(96);
        current = glide.sample(mid, current, -300.0).offset;
        // Streaming growth moves the end; at the instant it is seen the
        // painted position is unchanged.
        let retargeted = glide.sample(mid, current, -420.0).offset;
        assert_eq!(retargeted, current, "a retarget is position-continuous");
        // The next frame keeps moving the same way (no restart from rest).
        let next = glide
            .sample(mid + Duration::from_millis(16), current, -420.0)
            .offset;
        assert!(next < current, "the glide keeps travelling toward the end");
        let mut position = next;
        for now in frames(mid + Duration::from_millis(32), 30) {
            let step = glide.sample(now, position, -420.0);
            position = step.offset;
            if step.settled {
                break;
            }
        }
        assert_eq!(position, -420.0);
    }

    #[test]
    fn per_frame_streaming_growth_stays_one_monotonic_glide() {
        let start = Instant::now();
        let mut glide = BottomGlide::with_duration(0.0, FAST);
        let mut current = 0.0_f32;
        let mut target = -200.0_f32;
        let mut late_steps = Vec::new();
        for (frame, now) in frames(start, 60).enumerate() {
            if frame < 30 {
                target -= 12.0;
            }
            let step = glide.sample(now, current, target);
            let moved = current - step.offset;
            assert!(moved >= 0.0, "frame {frame} moved backwards");
            assert!(step.offset >= target, "frame {frame} passed the end");
            if (20..30).contains(&frame) {
                late_steps.push(moved);
            }
            current = step.offset;
            if step.settled {
                break;
            }
        }
        assert_eq!(current, target, "the glide settles on the final end");
        // Once the initial travel is absorbed, steady growth is followed at
        // the growth's own steady speed: no per-frame re-acceleration.
        for moved in late_steps {
            assert!(
                (moved - 12.0).abs() < 1.0,
                "steady growth must glide at a steady speed, got {moved}"
            );
        }
    }

    #[test]
    fn a_shrinking_end_glides_back_up_without_overshoot() {
        let start = Instant::now();
        let mut glide = BottomGlide::with_duration(-900.0, FAST);
        let mut current = -900.0;
        for now in frames(start, 30) {
            let step = glide.sample(now, current, -860.0);
            assert!(step.offset >= current && step.offset <= -860.0);
            current = step.offset;
            if step.settled {
                break;
            }
        }
        assert_eq!(current, -860.0);
    }

    #[test]
    fn reduced_motion_snaps_then_confirms_the_end_held() {
        let start = Instant::now();
        let mut glide = BottomGlide::new(0.0, true);
        let first = glide.sample(start, 0.0, -500.0);
        assert_eq!(first.offset, -500.0, "reduced motion snaps");
        assert!(
            !first.settled,
            "a fresh end waits one frame to be confirmed"
        );
        // The end space under a new tail lands a frame late: the next frame
        // snaps again to the corrected end.
        let corrected = glide.sample(start, -500.0, -540.0);
        assert_eq!(corrected.offset, -540.0);
        assert!(!corrected.settled);
        let confirmed = glide.sample(start, -540.0, -540.0);
        assert!(confirmed.settled);
    }

    #[test]
    fn an_external_move_is_adopted_and_the_end_still_reached() {
        let start = Instant::now();
        let mut glide = BottomGlide::with_duration(0.0, FAST);
        let early = glide.sample(start, 0.0, -400.0).offset;
        let early = glide
            .sample(start + Duration::from_millis(32), early, -400.0)
            .offset;
        // Something clamps the viewport 40 px away from the glide's write.
        let clamped = early + 40.0;
        let next = glide.sample(start + Duration::from_millis(48), clamped, -400.0);
        // The next frame is the painted offset plus the curve's own progress
        // over that frame: no lurch back to where the glide had written.
        #[expect(
            clippy::cast_possible_truncation,
            reason = "motion curves feed f32 layout values"
        )]
        let progress = (MotionCurve::SmoothOut.sample(48.0 / 250.0)
            - MotionCurve::SmoothOut.sample(32.0 / 250.0)) as f32;
        assert!(
            (next.offset - (clamped - 400.0 * progress)).abs() < 0.01,
            "the glide continues from the painted offset, not its own write"
        );
        let mut position = next.offset;
        for now in frames(start + Duration::from_millis(64), 40) {
            let step = glide.sample(now, position, -400.0);
            position = step.offset;
            if step.settled {
                break;
            }
        }
        assert_eq!(position, -400.0);
    }

    #[test]
    fn shift_moves_position_and_end_together() {
        let start = Instant::now();
        let mut glide = BottomGlide::with_duration(-100.0, FAST);
        let before = glide
            .sample(start + Duration::from_millis(64), -100.0, -300.0)
            .offset;
        glide.shift(-50.0);
        assert_eq!(glide.end(), -350.0);
        // Same instant after the shift: exactly the shifted position, with
        // the target shifted alike (no segment added for the shift).
        let after = glide.sample(start + Duration::from_millis(64), before - 50.0, -350.0);
        assert_eq!(after.offset, before - 50.0);
    }
}
