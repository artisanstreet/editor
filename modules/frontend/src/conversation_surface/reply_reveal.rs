//! Paced, shift-free reveal of a turn's streaming reply.
//!
//! The transport delivers reply text in arbitrary chunks. A row instead
//! shows a revealed prefix that only ever ends where nothing on screen can
//! move ([`stable_reveal_end`]): whole words, whole inline constructs, and
//! whole fences and tables.
//!
//! The prefix advances at a steady, capped pace rather than at the
//! transport's: a word budget accrues at a rate that eases toward clearing
//! the backlog within [`CATCH_UP_WINDOW_SECONDS`], never faster than
//! [`MAX_WORDS_PER_MINUTE`], and segments spend it at most every
//! [`SEGMENT_INTERVAL`]. A segment's short runs start left to right,
//! spread across [`SWEEP_SPAN`] (one segment interval), and each segment's
//! sweep picks up where the previous one's front is, so the fade front
//! glides through the text instead of arriving in bursts. Every run's long
//! soft fade overlaps its neighbours', so it never reads as letters
//! streaming. A reply arriving in one piece, or its remainder when the turn
//! ends, drains at the same capped pace.
//!
//! Opacity is the only animated property: glyph ranges cannot rise or blur
//! without moving layout, and layout must stay still. Reduced motion shows
//! every stable unit at once with no fade.

use std::time::{Duration, Instant};

use artisan_ui::markdown_reveal::{RevealFade, RevealSteps, stable_reveal_end};
use artisan_ui::motion::{MotionCurve, MotionDuration, MotionPolicy};

/// One run's fade-in (`--duration-very-slow`, texts reveal): long against
/// the sweep, so a soft gradient trails the front.
const UNIT_FADE: Duration = Duration::from_millis(400);

/// The fade's curve: a gentle start, so the front has no hard edge.
const UNIT_FADE_CURVE: MotionCurve = MotionCurve::EaseOut;

/// Visible characters per run of a segment's sweep: fine enough that the
/// gradient reads as one front, not as stepping blocks.
const SWEEP_RUN: usize = 3;

/// How long a segment's sweep takes from its first run to its last: the
/// gap until the next segment may start, so consecutive sweeps join into
/// one continuous front.
const SWEEP_SPAN: Duration = SEGMENT_INTERVAL;

/// The fastest prose ever reveals, in words per minute. The transport's
/// bursts and stalls never show through above this; a long reply may trail
/// the model while it streams and catches up at this pace afterwards.
pub(super) const MAX_WORDS_PER_MINUTE: f64 = 900.0;

/// The slowest prose reveals while words wait, so the last words of a
/// stream never trickle in.
pub(super) const MIN_WORDS_PER_MINUTE: f64 = 300.0;

/// The reveal aims to clear its backlog within this window, so it speeds up
/// gently behind a burst and slows as it catches up.
const CATCH_UP_WINDOW_SECONDS: f64 = 1.5;

/// How quickly the reveal rate follows its target: the time constant of an
/// exponential ease, so speed changes glide instead of stepping.
const RATE_EASE_SECONDS: f64 = 0.5;

/// The most words the budget may bank, so a pause never ends in a burst.
const MAX_BANKED_WORDS: f64 = 6.0;

/// What one unit costs at most: a fence or table appears whole, but should
/// not stall the reveal for as long as reading all of its words.
const MAX_UNIT_COST: f64 = 6.0;

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
    /// The smoothed reveal rate in words per second.
    rate: f64,
    /// Words the rate has earned but not yet revealed.
    budget: f64,
    /// When the rate and budget last advanced.
    paced_at: Instant,
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
            rate: MIN_WORDS_PER_MINUTE / 60.0,
            // The first word shows at once; the pace builds from there.
            budget: 1.0,
            paced_at: now,
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
                MotionPolicy::Full => {
                    let steps = RevealSteps::new(&body[..stable]);
                    self.pace(steps.pending_words(start), now);
                    if now >= self.next_tick && self.budget >= 1.0 {
                        // Spend the budget on whole units; one unit may run
                        // the budget negative, which the pace then repays.
                        let mut end = start;
                        while end < stable && self.budget >= 1.0 {
                            let next = steps.next(end);
                            self.budget -= unit_cost(&body[end..next]);
                            end = next;
                        }
                        self.revealed.push_str(&body[start..end]);
                        self.next_tick = now + SEGMENT_INTERVAL;
                    }
                }
            }
        }
        if self.revealed.len() > start && !self.resync && motion == MotionPolicy::Full {
            self.pending = Some((self.visible_len, now));
        }
        self.fade = self.resolve_fade(now);
        self.revealed.len() < stable || !self.marks.is_empty() || self.pending.is_some()
    }

    /// Eases the rate toward clearing `backlog` words within the catch-up
    /// window, between [`MIN_WORDS_PER_MINUTE`] and [`MAX_WORDS_PER_MINUTE`],
    /// and earns budget at it.
    fn pace(&mut self, backlog: usize, now: Instant) {
        let elapsed = now.saturating_duration_since(self.paced_at).as_secs_f64();
        self.paced_at = now;
        let backlog = f64::from(u32::try_from(backlog).unwrap_or(u32::MAX));
        let target = (backlog / CATCH_UP_WINDOW_SECONDS)
            .clamp(MIN_WORDS_PER_MINUTE / 60.0, MAX_WORDS_PER_MINUTE / 60.0);
        let blend = 1.0 - (-elapsed / RATE_EASE_SECONDS).exp();
        self.rate += (target - self.rate) * blend;
        self.budget = (self.budget + self.rate * elapsed).min(MAX_BANKED_WORDS);
    }

    /// Records the visible length the renderer reported for `revealed`, and
    /// splits a new segment into runs whose fades start left to right.
    pub(super) fn rendered(&mut self, visible_len: usize) {
        if let Some((start, began)) = self.pending.take()
            && visible_len > start
        {
            let runs = (visible_len - start).div_ceil(SWEEP_RUN);
            let stagger = SWEEP_SPAN / u32::try_from(runs).unwrap_or(u32::MAX);
            // The front carries on from the previous segment's last run, so
            // a segment arriving mid-sweep queues behind it; starts stay in
            // order, which the fade's prefix drain relies on.
            let front = self
                .marks
                .last()
                .map_or(began, |(_, last)| began.max(*last + stagger));
            for run in 0..runs {
                let delay = stagger * u32::try_from(run).unwrap_or(u32::MAX);
                self.marks.push((start + run * SWEEP_RUN, front + delay));
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
            let eased = UNIT_FADE_CURVE.sample(progress) as f32;
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

/// What revealing one unit costs the budget: its words, capped so a whole
/// fence or table does not stall the prose after it.
fn unit_cost(unit: &str) -> f64 {
    let words = f64::from(u32::try_from(unit.split_whitespace().count()).unwrap_or(u32::MAX));
    words.clamp(1.0, MAX_UNIT_COST)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Advances one reveal at 60 fps for `frames`, rendering each frame;
    /// returns the largest segment it revealed, in words.
    fn run(
        reveal: &mut ReplyReveal,
        body: &str,
        streaming: bool,
        at: &mut Instant,
        frames: u32,
    ) -> usize {
        let mut largest = 0;
        for _ in 0..frames {
            *at += Duration::from_millis(16);
            let before = reveal.revealed().split_whitespace().count();
            reveal.advance(body, streaming, MotionPolicy::Full, *at);
            let after = reveal.revealed().split_whitespace().count();
            largest = largest.max(after - before);
            reveal.rendered(reveal.revealed().len());
        }
        largest
    }

    #[test]
    fn a_segment_sweeps_in_left_to_right() {
        let start = Instant::now();
        let mut reveal = ReplyReveal::begin(start);
        // A 20-character segment revealed this frame splits into runs.
        reveal.pending = Some((0, start));
        reveal.rendered(20);
        let mid = start + UNIT_FADE / 3;
        reveal.fade = reveal.resolve_fade(mid);
        let (left, right) = (reveal.fade.alpha_at(0), reveal.fade.alpha_at(19));
        assert!(right < left, "the left run leads the right one");
        assert!(left < 1.0 && right > 0.0, "one segment, fading together");
    }

    #[test]
    fn prose_reveals_at_a_steady_capped_pace() {
        // A reply that lands whole, as at the end of a turn: no snap, no
        // burst, never faster than the cap, and it still drains.
        let body = "word ".repeat(120);
        let start = Instant::now();
        let mut at = start;
        let mut reveal = ReplyReveal::begin(start);
        let largest = run(&mut reveal, &body, false, &mut at, 120);
        let shown = reveal.revealed().split_whitespace().count();
        let cap = (MAX_WORDS_PER_MINUTE / 60.0 * 2.0) as usize + MAX_BANKED_WORDS as usize + 1;
        assert!(shown > 10, "it moves: {shown} words in two seconds");
        assert!(shown <= cap, "{shown} words in two seconds exceeds the cap");
        assert!(
            largest <= MAX_BANKED_WORDS as usize + 1,
            "no burst: {largest} words at once"
        );
        run(&mut reveal, &body, false, &mut at, 60 * 10);
        assert_eq!(reveal.revealed(), body, "the whole reply drains");
    }

    #[test]
    fn complete_words_reveal_and_a_growing_word_waits() {
        let start = Instant::now();
        let mut at = start;
        let mut reveal = ReplyReveal::begin(start);
        run(&mut reveal, "Hello brave new", true, &mut at, 60);
        // Every complete word shows; the trailing `new` may still grow.
        assert_eq!(reveal.revealed(), "Hello brave ");
        run(&mut reveal, "Hello brave new", false, &mut at, 60);
        assert_eq!(reveal.revealed(), "Hello brave new");
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
