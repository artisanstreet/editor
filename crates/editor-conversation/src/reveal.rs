//! Paced, shift-free reveal of a streaming reply (ported from the old
//! `reply_reveal`). The shown prefix only ever ends where nothing painted can
//! move ([`stable_reveal_end`]): whole words, whole inline constructs, whole
//! fences and tables. It advances at a steady capped pace, and each new
//! segment fades in left to right. Opacity is the only animated property.

use editor_ui::markdown_reveal::{RevealFade, RevealSteps, stable_reveal_end};
use editor_ui::motion::{MotionCurve, MotionDuration, MotionPolicy};
use std::time::{Duration, Instant};

const UNIT_FADE: Duration = Duration::from_millis(400);
const SWEEP_RUN: usize = 3;
const SEGMENT_INTERVAL: Duration = MotionDuration::Quick.as_duration();
const MAX_WORDS_PER_MINUTE: f64 = 900.0;
const MIN_WORDS_PER_MINUTE: f64 = 300.0;
const CATCH_UP_WINDOW_SECONDS: f64 = 1.5;
const RATE_EASE_SECONDS: f64 = 0.5;
const MAX_BANKED_WORDS: f64 = 6.0;
const MAX_UNIT_COST: f64 = 6.0;

/// Reveal progress of one reply, kept in that item's UI state.
pub(crate) struct Reveal {
    revealed: String,
    visible_len: usize,
    resync: bool,
    marks: Vec<(usize, Instant)>,
    pending: Option<(usize, Instant)>,
    next_tick: Instant,
    rate: f64,
    budget: f64,
    paced_at: Instant,
    pub fade: RevealFade,
}

impl Reveal {
    /// Starts revealing a reply that began while the thread was open.
    pub(crate) fn begin(now: Instant) -> Self {
        Self {
            revealed: String::new(),
            visible_len: 0,
            resync: false,
            marks: Vec::new(),
            pending: None,
            next_tick: now,
            rate: MIN_WORDS_PER_MINUTE / 60.0,
            budget: 1.0,
            paced_at: now,
            fade: RevealFade::default(),
        }
    }

    /// Resumes a reply already mid-stream when first shown: what arrived
    /// shows at once.
    pub(crate) fn resume(body: &str, now: Instant) -> Self {
        Self {
            revealed: body[..stable_reveal_end(body)].to_owned(),
            resync: true,
            ..Self::begin(now)
        }
    }

    pub(crate) fn revealed(&self) -> &str {
        &self.revealed
    }

    /// Reveals at most one segment; returns whether it still animates.
    pub(crate) fn advance(
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
            // Replaced rather than appended: show the new text outright.
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

    /// Holds the reveal where it is while blocks above it settle: nothing
    /// new reveals, the fades on screen finish, and no pace banks up.
    /// Returns whether a fade still runs.
    pub(crate) fn wait(&mut self, now: Instant) -> bool {
        self.paced_at = now;
        self.fade = self.resolve_fade(now);
        !self.marks.is_empty() || self.pending.is_some()
    }

    /// Whether this prose still holds the blocks after it: words left to
    /// reveal, or its newest words not yet halfway faded in.
    pub(crate) fn settling(&self, body: &str, streaming: bool, now: Instant) -> bool {
        let stable = if streaming {
            stable_reveal_end(body)
        } else {
            body.len()
        };
        let half = UNIT_FADE / 2;
        self.revealed.len() < stable
            || self.pending.is_some()
            || self
                .marks
                .last()
                .is_some_and(|(_, started)| now.saturating_duration_since(*started) < half)
    }

    /// Records the renderer's visible length and splits the newest segment
    /// into runs whose fades start left to right.
    pub(crate) fn rendered(&mut self, visible_len: usize) {
        if let Some((start, began)) = self.pending.take()
            && visible_len > start
        {
            let runs = (visible_len - start).div_ceil(SWEEP_RUN);
            let stagger = SEGMENT_INTERVAL / u32::try_from(runs).unwrap_or(u32::MAX);
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

    fn resolve_fade(&mut self, now: Instant) -> RevealFade {
        #[allow(clippy::cast_possible_truncation)]
        let alpha = |started: Instant| {
            let progress =
                now.saturating_duration_since(started).as_secs_f64() / UNIT_FADE.as_secs_f64();
            MotionCurve::EaseOut.sample(progress) as f32
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
                .chain(self.pending.map(|(offset, _)| (offset, 0.0)))
                .collect(),
        )
    }
}

fn unit_cost(unit: &str) -> f64 {
    let words = f64::from(u32::try_from(unit.split_whitespace().count()).unwrap_or(u32::MAX));
    words.clamp(1.0, MAX_UNIT_COST)
}
