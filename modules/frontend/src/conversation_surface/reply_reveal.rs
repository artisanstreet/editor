//! Paced, shift-free reveal of a turn's streaming reply.
//!
//! The transport delivers reply text in arbitrary chunks. A row instead
//! shows a revealed prefix that only ever ends where nothing on screen can
//! move ([`stable_reveal_end`]): whole words, whole inline constructs, and
//! whole fences and tables.
//!
//! The prefix advances in segments: at most every [`SEGMENT_INTERVAL`],
//! everything that became stable since the last segment is revealed at
//! once and fades in as one piece, one opacity across the whole segment.
//! Revealing word by word with long overlapping fades read as a
//! left-to-right wipe across letters; a segment instead appears together.
//! Opacity is the only animated property: glyph ranges cannot rise or blur
//! without moving layout, and layout must stay still. Reduced motion shows
//! every stable unit at once with no fade.

use std::time::{Duration, Instant};

use artisan_ui::markdown_reveal::{RevealFade, RevealSteps, stable_reveal_end};
use artisan_ui::motion::{MotionCurve, MotionDuration, MotionPolicy};

/// One segment's fade-in (`--duration-medium`): long enough to read as a
/// fade, short enough that at most a couple of segments are mid-fade.
const UNIT_FADE: Duration = MotionDuration::Medium.as_duration();

/// The shortest gap between two segments (`--duration-quick`). Text that
/// becomes stable within it joins the next segment.
const SEGMENT_INTERVAL: Duration = MotionDuration::Quick.as_duration();

/// A reply first seen with more words pending than this shows them at once:
/// replaying text that already arrived would read as a replay, not a stream.
const SNAP_BACKLOG_WORDS: usize = 96;

/// The reveal state of one streaming prose body, retained by its turn row.
pub(super) struct ReplyReveal {
    /// The revealed source prefix, kept to detect a non-append replacement.
    revealed: String,
    /// The renderer's visible length of `revealed` at its last render.
    visible_len: usize,
    /// When set, the next render only re-measures `visible_len`: the prefix
    /// jumped without a fade, so there is no segment to mark.
    resync: bool,
    /// Fade start per revealed segment, by visible offset, oldest first.
    marks: Vec<(usize, Instant)>,
    /// Earliest instant the next pacing tick may reveal more.
    next_tick: Instant,
    /// The fade resolved by the latest [`Self::advance`].
    pub(super) fade: RevealFade,
}

impl ReplyReveal {
    /// Starts revealing prose first seen while it streams.
    pub(super) fn begin(body: &str, now: Instant) -> Self {
        let stable = &body[..stable_reveal_end(body)];
        let backlog = RevealSteps::new(stable).pending_words(0);
        let snap = backlog > SNAP_BACKLOG_WORDS;
        Self {
            revealed: if snap {
                stable.to_owned()
            } else {
                String::new()
            },
            visible_len: 0,
            resync: snap,
            marks: Vec::new(),
            next_tick: now,
            fade: RevealFade::default(),
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
            self.resync = true;
        }
        let start = self.revealed.len();
        if start < stable {
            match motion {
                MotionPolicy::Reduced => body[..stable].clone_into(&mut self.revealed),
                MotionPolicy::Full if now >= self.next_tick => {
                    self.revealed.push_str(&body[start..stable]);
                    self.next_tick = now + SEGMENT_INTERVAL;
                }
                MotionPolicy::Full => {}
            }
        }
        if self.revealed.len() > start && !self.resync && motion == MotionPolicy::Full {
            self.marks.push((self.visible_len, now));
        }
        self.fade = self.resolve_fade(now);
        self.revealed.len() < stable || !self.marks.is_empty()
    }

    /// Records the visible length the renderer reported for `revealed`.
    pub(super) fn rendered(&mut self, visible_len: usize) {
        self.visible_len = visible_len;
        self.resync = false;
    }

    /// Whether the reveal has nothing left to show or fade for `body`.
    pub(super) fn is_complete(&self, body: &str) -> bool {
        self.revealed.len() == body.len() && self.marks.is_empty()
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
            let eased = MotionCurve::EaseInOut.sample(progress) as f32;
            eased
        };
        let opaque = self
            .marks
            .iter()
            .take_while(|(_, started)| alpha(*started) >= 1.0)
            .count();
        self.marks.drain(..opaque);
        RevealFade::new(
            self.marks
                .iter()
                .map(|(offset, started)| (*offset, alpha(*started)))
                .collect(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reveals_each_stable_stretch_as_one_segment_with_one_opacity() {
        let start = Instant::now();
        let mut reveal = ReplyReveal::begin("Hello brave new", start);
        assert_eq!(reveal.revealed(), "");
        assert!(reveal.advance("Hello brave new", true, MotionPolicy::Full, start));
        // Every complete word shows at once; the trailing `new` may grow.
        assert_eq!(reveal.revealed(), "Hello brave ");
        reveal.rendered(11);
        let mid = start + UNIT_FADE / 2;
        reveal.advance("Hello brave new", true, MotionPolicy::Full, mid);
        // One segment, one opacity: the first and last letters match.
        assert!(reveal.fade.alpha_at(0) < 1.0);
        assert!((reveal.fade.alpha_at(0) - reveal.fade.alpha_at(10)).abs() < f32::EPSILON);

        // Text stable within the interval waits for the next segment.
        let soon = start + SEGMENT_INTERVAL / 2;
        reveal.advance("Hello brave new world ", true, MotionPolicy::Full, soon);
        assert_eq!(reveal.revealed(), "Hello brave ");
        let next = start + SEGMENT_INTERVAL;
        reveal.advance("Hello brave new world ", true, MotionPolicy::Full, next);
        assert_eq!(reveal.revealed(), "Hello brave new world ");
        reveal.rendered(21);

        let done = next + UNIT_FADE * 2;
        assert!(!reveal.advance("Hello brave new world ", false, MotionPolicy::Full, done));
        assert!(reveal.is_complete("Hello brave new world "));
    }

    #[test]
    fn reduced_motion_shows_the_stable_prefix_without_fading() {
        let start = Instant::now();
        let mut reveal = ReplyReveal::begin("", start);
        let animating = reveal.advance("One two three four", true, MotionPolicy::Reduced, start);
        assert_eq!(reveal.revealed(), "One two three ");
        assert!(!animating);
        assert!(reveal.fade.is_opaque());
    }

    #[test]
    fn a_replaced_body_snaps_without_replaying() {
        let start = Instant::now();
        let mut reveal = ReplyReveal::begin("", start);
        reveal.advance("Alpha beta ", true, MotionPolicy::Full, start);
        reveal.rendered(5);
        reveal.advance("Gamma delta ", true, MotionPolicy::Full, start);
        assert_eq!(reveal.revealed(), "Gamma delta ");
    }

    #[test]
    fn a_long_reply_first_seen_mid_stream_shows_at_once() {
        let body = "word ".repeat(SNAP_BACKLOG_WORDS + 1);
        let reveal = ReplyReveal::begin(&body, Instant::now());
        assert_eq!(reveal.revealed(), body);
    }
}
