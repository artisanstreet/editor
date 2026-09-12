//! Observed visible-text throughput, measured at provider frame receipt.
//!
//! Uses the o200k reference encoding, not billing/hidden-reasoning counters.
//! Startup and the first frame of each interval are outside the sample.
//! Explicit activity/part boundaries split intervals; arbitrary gaps within
//! streaming remain included because their cause cannot be inferred safely.

use std::time::{Duration, Instant};

use crate::engine_owner::observation::TextDelta;

#[derive(Default)]
pub(super) struct StreamingSpeed {
    current: Option<Interval>,
    tokens: u64,
    elapsed: Duration,
}

struct Interval {
    part: Option<String>,
    first: Instant,
    last: Instant,
    first_bytes: usize,
    frames: u32,
    text: String,
}

impl StreamingSpeed {
    pub(super) fn push(&mut self, delta: &TextDelta) {
        self.observe(delta.part_id(), delta.delta(), delta.received_at());
    }

    fn observe(&mut self, part: Option<&str>, text: &str, received: Instant) {
        if text.is_empty() {
            return;
        }
        if self
            .current
            .as_ref()
            .is_some_and(|span| span.part.as_deref() != part)
        {
            self.end_interval();
        }
        let span = self.current.get_or_insert_with(|| Interval {
            part: part.map(str::to_owned),
            first: received,
            last: received,
            first_bytes: 0,
            frames: 1,
            text: String::new(),
        });
        // Match the durable assistant body ceiling; never retain an unbounded
        // second transcript, even if a malformed provider ignores its limit.
        if span.text.len().saturating_add(text.len()) > artisan_domain::AssistantBody::MAX_BYTES {
            self.current = None;
            return;
        }
        span.text.push_str(text);
        if received == span.first {
            // Artificial 4096-byte splits of one provider frame count once.
            span.first_bytes = span.text.len();
        }
        if received > span.last {
            span.frames = span.frames.saturating_add(1);
            span.last = received;
        }
    }

    pub(super) fn end_interval(&mut self) {
        if let Some(span) = self.current.take()
            && let Some((tokens, elapsed)) = span.sample()
        {
            self.tokens = self.tokens.saturating_add(tokens);
            self.elapsed = self.elapsed.saturating_add(elapsed);
        }
    }

    pub(super) fn rate(&self) -> Option<u32> {
        let (tokens, elapsed) = self
            .current
            .as_ref()
            .and_then(Interval::sample)
            .unwrap_or((0, Duration::ZERO));
        let tokens = self.tokens.checked_add(tokens)?;
        let elapsed = self.elapsed.checked_add(elapsed)?.as_micros();
        if elapsed == 0 || tokens == 0 {
            return None;
        }
        u32::try_from(u128::from(tokens).checked_mul(1_000_000_000)? / elapsed).ok()
    }
}

impl Interval {
    fn sample(&self) -> Option<(u64, Duration)> {
        let elapsed = self.last.checked_duration_since(self.first)?;
        if self.frames < 3 || elapsed < Duration::from_secs(1) {
            return None;
        }
        let bpe = tiktoken_rs::o200k_base_singleton();
        let tokens = bpe
            .encode_ordinary(&self.text)
            .len()
            .saturating_sub(bpe.encode_ordinary(&self.text[..self.first_bytes]).len());
        if tokens < 16 {
            return None;
        }
        Some((u64::try_from(tokens).ok()?, elapsed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(start: Instant) -> StreamingSpeed {
        let mut speed = StreamingSpeed::default();
        speed.observe(Some("a"), &"startup ".repeat(500), start);
        speed.observe(
            Some("a"),
            &"hello world ".repeat(20),
            start + Duration::from_secs(1),
        );
        speed.observe(
            Some("a"),
            &"hello world ".repeat(20),
            start + Duration::from_secs(2),
        );
        speed
    }

    #[test]
    fn startup_and_first_frame_are_excluded() {
        let now = Instant::now();
        assert_eq!(
            sample(now).rate(),
            sample(now + Duration::from_secs(90)).rate()
        );
        let rate = sample(now).rate().unwrap();
        assert!((39_000..42_000).contains(&rate), "{rate}");
    }

    #[test]
    fn tool_wait_does_not_lower_rate() {
        let now = Instant::now();
        let mut speed = sample(now);
        let first = speed.rate().unwrap();
        speed.end_interval();
        speed.observe(Some("b"), "first ", now + Duration::from_secs(90));
        speed.observe(
            Some("b"),
            &"hello world ".repeat(20),
            now + Duration::from_secs(91),
        );
        speed.observe(
            Some("b"),
            &"hello world ".repeat(20),
            now + Duration::from_secs(92),
        );
        assert!(speed.rate().unwrap().abs_diff(first) < 1000);
    }

    #[test]
    fn buffered_and_short_samples_stay_absent() {
        let now = Instant::now();
        let mut speed = StreamingSpeed::default();
        for _ in 0..10 {
            speed.observe(None, &"hello ".repeat(100), now);
        }
        assert_eq!(speed.rate(), None);
        let mut short = StreamingSpeed::default();
        for n in 0..3 {
            short.observe(None, " hi", now + Duration::from_secs(n));
        }
        assert_eq!(short.rate(), None);
    }

    #[test]
    fn a_new_part_starts_after_its_first_frame() {
        let now = Instant::now();
        let mut speed = sample(now);
        let first = speed.rate();
        speed.observe(
            Some("b"),
            &"buffered ".repeat(100),
            now + Duration::from_secs(90),
        );
        assert_eq!(speed.rate(), first);
    }
}
