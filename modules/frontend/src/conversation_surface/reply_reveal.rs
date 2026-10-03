//! Paced, shift-free reveal of a turn's streaming reply.
//!
//! The transport delivers reply text in arbitrary chunks. A row instead
//! shows a revealed prefix that only ever ends where nothing on screen can
//! move ([`stable_reveal_end`]): whole words, whole inline constructs, and
//! whole fences and tables.
//!
//! The prefix advances in segments: at most every [`SEGMENT_INTERVAL`],
//! what became stable since the last segment is revealed together. A
//! segment fades in mostly as one piece, but its short runs start a frame
//! apart from left to right, so it sweeps into place instead of spawning;
//! the sweep is capped at [`SWEEP_SPAN`], well inside the fade, so it never
//! reads as letters streaming. A large backlog (a reply arriving in one
//! piece, or its remainder when the turn ends) is drained in capped
//! segments instead of appearing at once.
//!
//! Opacity is the only animated property: glyph ranges cannot rise or blur
//! without moving layout, and layout must stay still. Reduced motion shows
//! every stable unit at once with no fade.

use std::time::{Duration, Instant};

use artisan_ui::markdown_reveal::{RevealFade, RevealSteps, stable_reveal_end};
use artisan_ui::motion::{MotionCurve, MotionDuration, MotionPolicy};

/// One run's fade-in (`--duration-medium`): long enough to read as a fade,
/// short enough that at most a couple of segments are mid-fade.
const UNIT_FADE: Duration = MotionDuration::Medium.as_duration();

/// Visible characters per run of a segment's sweep.
const SWEEP_RUN: usize = 6;

/// Delay between neighbouring runs: one display frame.
const SWEEP_STAGGER: Duration = Duration::from_millis(16);

/// The longest a segment's sweep may take from its first run to its last.
const SWEEP_SPAN: Duration = Duration::from_millis(160);

/// The fewest words one segment reveals while draining a backlog; larger
/// backlogs drain in about six segments.
const DRAIN_MIN_WORDS: usize = 12;

/// The shortest gap between two segments (`--duration-quick`). Text that
/// becomes stable within it joins the next segment.
const SEGMENT_INTERVAL: Duration = MotionDuration::Quick.as_duration();

/// The reveal state of one streaming prose body, retained by its turn row.
pub(super) struct ReplyReveal {
    /// The revealed source prefix, kept to detect a non-append replacement.
    revealed: String,
    /// The renderer's visible length of `revealed` at its last render.
    visible_len: usize,
    /// When set, the next render only re-measures `visible_len`: the prefix
    /// jumped without a fade, so there is no segment to mark.
    resync: bool,
    /// Fade start per revealed run, by visible offset, oldest first.
    marks: Vec<(usize, Instant)>,
    /// The newest segment's start, until its render reports where it ends
    /// and it can be split into runs.
    pending: Option<(usize, Instant)>,
    /// Earliest instant the next pacing tick may reveal more.
    next_tick: Instant,
    /// The fade resolved by the latest [`Self::advance`].
    pub(super) fade: RevealFade,
}

impl ReplyReveal {
    /// Starts revealing prose that arrived while its row was on screen.
    pub(super) fn begin(now: Instant) -> Self {
        Self {
            revealed: String::new(),
            visible_len: 0,
            resync: false,
            marks: Vec::new(),
            pending: None,
            next_tick: now,
            fade: RevealFade::default(),
        }
    }

    /// Resumes revealing prose already mid-stream when its row first
    /// painted: what arrived shows at once, only later text streams in.
    pub(super) fn resume(body: &str, now: Instant) -> Self {
        Self {
            revealed: body[..stable_reveal_end(body)].to_owned(),
            resync: true,
            ..Self::begin(now)
        }
    }

    /// The revealed source prefix.
    pub(super) fn revealed(&self) -> &str {
        &self.revealed
    }

    /// Reveals at most one segment and resolves this frame's fade.
    ///
    /// `streaming` is the reply's own liveness: a settled reply drains to
    /// its full length. Returns whether the reveal still animates, so the
    /// row keeps requesting frames only while it does.
    pub(super) fn advance(
        &mut self,
        body: &str,
        streaming: bool,
        motion: MotionPolicy,
        now: Instant,
    ) -> bool {
        let stable = if streaming {
            stable_reveal_end(body)
        } else {
            body.len()
        };
        if !body.starts_with(self.revealed.as_str()) {
            // A replacement rather than an append: the shown prefix is gone,
            // so show the new stable prefix outright instead of fading it.
            body[..stable].clone_into(&mut self.revealed);
            self.marks.clear();
            self.pending = None;
            self.resync = true;
        }
        let start = self.revealed.len();
        if start < stable {
            match motion {
                MotionPolicy::Reduced => body[..stable].clone_into(&mut self.revealed),
                MotionPolicy::Full if now >= self.next_tick => {
                    // A normal stream reveals everything stable; a backlog
                    // drains in capped segments rather than all at once.
                    let steps = RevealSteps::new(&body[..stable]);
                    let cap = DRAIN_MIN_WORDS.max(steps.pending_words(start).div_ceil(6));
                    let mut end = start;
                    for _ in 0..cap {
                        end = steps.next(end);
                        if end >= stable {
                            break;
                        }
                    }
                    self.revealed.push_str(&body[start..end]);
                    self.next_tick = now + SEGMENT_INTERVAL;
                }
                MotionPolicy::Full => {}
            }
        }
        if self.revealed.len() > start && !self.resync && motion == MotionPolicy::Full {
            self.pending = Some((self.visible_len, now));
        }
        self.fade = self.resolve_fade(now);
        self.revealed.len() < stable || !self.marks.is_empty() || self.pending.is_some()
    }

    /// Records the visible length the renderer reported for `revealed`, and
    /// splits a new segment into runs whose fades start left to right.
    pub(super) fn rendered(&mut self, visible_len: usize) {
        if let Some((start, began)) = self.pending.take()
            && visible_len > start
        {
            let runs = (visible_len - start).div_ceil(SWEEP_RUN);
            let stagger = SWEEP_STAGGER.min(SWEEP_SPAN / u32::try_from(runs).unwrap_or(u32::MAX));
            for run in 0..runs {
                let delay = stagger * u32::try_from(run).unwrap_or(u32::MAX);
                self.marks.push((start + run * SWEEP_RUN, began + delay));
            }
        }
        self.visible_len = visible_len;
        self.resync = false;
    }

    /// Whether the reveal has nothing left to show or fade for `body`.
    pub(super) fn is_complete(&self, body: &str) -> bool {
        self.revealed.len() == body.len() && self.marks.is_empty() && self.pending.is_none()
    }

    /// Resolves each segment's opacity and drops the fully opaque oldest
    /// marks; fades end oldest first, so those form a prefix.
    fn resolve_fade(&mut self, now: Instant) -> RevealFade {
        let alpha = |started: Instant| {
            let progress =
                now.saturating_duration_since(started).as_secs_f64() / UNIT_FADE.as_secs_f64();
            #[expect(
                clippy::cast_possible_truncation,
                reason = "the shared easing curve samples in f64 and feeds an f32 opacity; the narrowing is the intended precision"
            )]
            let eased = MotionCurve::SmoothOut.sample(progress) as f32;
            eased
        };
        let opaque = self
            .marks
            .iter()
            .take_while(|(_, started)| alpha(*started) >= 1.0)
            .count();
        self.marks.drain(..opaque);
        // A segment awaiting its runs paints transparent: it was revealed
        // this frame, so its fade has not begun.
        RevealFade::new(
            self.marks
                .iter()
                .map(|(offset, started)| (*offset, alpha(*started)))
                .chain(self.pending.map(|(offset, _)| (offset, 0.0)))
                .collect(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_segment_sweeps_in_left_to_right_and_settles() {
        let start = Instant::now();
        let mut reveal = ReplyReveal::begin(start);
        assert!(reveal.advance("Hello brave new", true, MotionPolicy::Full, start));
        // Every complete word shows at once; the trailing `new` may grow.
        assert_eq!(reveal.revealed(), "Hello brave ");
        // Revealed this frame: transparent until its runs are known.
        assert!(reveal.fade.alpha_at(0) <= f32::EPSILON);
        reveal.rendered(11);
        let mid = start + UNIT_FADE / 3;
        reveal.advance("Hello brave new", true, MotionPolicy::Full, mid);
        let (left, right) = (reveal.fade.alpha_at(0), reveal.fade.alpha_at(10));
        assert!(right < left, "the left run leads the right one");
        assert!(left < 1.0 && right > 0.0, "one segment, fading together");

        // Text stable within the interval waits for the next segment.
        let soon = start + SEGMENT_INTERVAL / 2;
        reveal.advance("Hello brave new world ", true, MotionPolicy::Full, soon);
        assert_eq!(reveal.revealed(), "Hello brave ");
        let next = start + SEGMENT_INTERVAL;
        reveal.advance("Hello brave new world ", true, MotionPolicy::Full, next);
        assert_eq!(reveal.revealed(), "Hello brave new world ");
        reveal.rendered(21);

        let done = next + SWEEP_SPAN + UNIT_FADE * 2;
        assert!(!reveal.advance("Hello brave new world ", false, MotionPolicy::Full, done));
        assert!(reveal.is_complete("Hello brave new world "));
    }

    #[test]
    fn a_backlog_drains_in_capped_segments_instead_of_snapping() {
        // A reply that arrives whole, as at the end of a turn.
        let body = "word ".repeat(120);
        let start = Instant::now();
        let mut reveal = ReplyReveal::begin(start);
        reveal.advance(&body, false, MotionPolicy::Full, start);
        let first = reveal.revealed().split_whitespace().count();
        assert_eq!(first, 20, "a sixth of the backlog per segment");
        reveal.rendered(first * 4);
        let mut at = start;
        for _ in 0..12 {
            at += SEGMENT_INTERVAL;
            reveal.advance(&body, false, MotionPolicy::Full, at);
            reveal.rendered(reveal.revealed().len());
        }
        assert_eq!(reveal.revealed(), body, "the whole backlog drains");
    }

    #[test]
    fn reduced_motion_shows_the_stable_prefix_without_fading() {
        let start = Instant::now();
        let mut reveal = ReplyReveal::begin(start);
        let animating = reveal.advance("One two three four", true, MotionPolicy::Reduced, start);
        assert_eq!(reveal.revealed(), "One two three ");
        assert!(!animating);
        assert!(reveal.fade.is_opaque());
    }

    #[test]
    fn a_replaced_body_snaps_without_replaying() {
        let start = Instant::now();
        let mut reveal = ReplyReveal::begin(start);
        reveal.advance("Alpha beta ", true, MotionPolicy::Full, start);
        reveal.rendered(5);
        reveal.advance("Gamma delta ", true, MotionPolicy::Full, start);
        assert_eq!(reveal.revealed(), "Gamma delta ");
    }
}
