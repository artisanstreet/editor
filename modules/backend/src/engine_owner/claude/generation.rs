//! Root-model generation speed for one Claude turn.
//!
//! Claude does not stream text at the pace it generates it: after thinking,
//! a whole answer can arrive as one burst of deltas within milliseconds, and
//! tool runs pause the stream between messages. Frame receipt times therefore
//! say nothing about how fast the model writes. Each root message instead
//! reports its own output tokens (`message_delta` usage), and the time it
//! spent generating them runs from its first content frame to its last. The
//! speed is those tokens over that time, summed across the turn's messages.
//! Subagent frames route away before decoding and never reach this clock.

use std::time::{Duration, Instant};

/// Shortest generation time that yields a speed.
const MIN_ELAPSED: Duration = Duration::from_secs(1);
/// Fewest output tokens that yield a speed.
const MIN_TOKENS: u64 = 16;

/// Generation windows of the turn's root messages.
#[derive(Debug, Default)]
pub(crate) struct ClaudeGenerationClock {
    open: Option<Window>,
    tokens: u64,
    elapsed: Duration,
}

/// The message generating now.
#[derive(Debug, Default)]
struct Window {
    first: Option<Instant>,
    last: Option<Instant>,
    tokens: Option<u64>,
}

impl ClaudeGenerationClock {
    /// A new root message opened; the previous one is complete.
    pub(crate) fn message_started(&mut self) {
        self.close();
        self.open = Some(Window::default());
    }

    /// One content frame of the current message arrived at `now`.
    pub(crate) fn content(&mut self, now: Instant) {
        if let Some(window) = &mut self.open {
            window.first.get_or_insert(now);
            window.last = Some(now);
        }
    }

    /// The current message reported its output tokens at `now`: the model
    /// finished generating it.
    pub(crate) fn output_tokens(&mut self, tokens: u64, now: Instant) {
        if let Some(window) = &mut self.open {
            window.tokens = Some(tokens);
            if window.first.is_some() {
                window.last = Some(now);
            }
        }
    }

    /// Closes the current message; it counts only once it reported tokens
    /// and generated across more than one frame.
    pub(crate) fn close(&mut self) {
        let Some(window) = self.open.take() else {
            return;
        };
        if let (Some(first), Some(last), Some(tokens)) = (window.first, window.last, window.tokens)
            && last > first
        {
            self.tokens = self.tokens.saturating_add(tokens);
            self.elapsed = self.elapsed.saturating_add(last - first);
        }
    }

    /// Output tokens per second across the closed messages, multiplied by
    /// 1000; `None` until enough generation was measured.
    pub(crate) fn rate(&self) -> Option<u32> {
        if self.elapsed < MIN_ELAPSED || self.tokens < MIN_TOKENS {
            return None;
        }
        let micros = self.elapsed.as_micros();
        u32::try_from(u128::from(self.tokens).checked_mul(1_000_000_000)? / micros).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_burst_after_thinking_counts_the_whole_generation() {
        // Thinking from 0 s to 3.8 s, then the whole answer in 10 ms.
        let start = Instant::now();
        let mut clock = ClaudeGenerationClock::default();
        clock.message_started();
        clock.content(start);
        for millis in 0..75 {
            clock.content(start + Duration::from_millis(3_800 + millis / 8));
        }
        clock.output_tokens(266, start + Duration::from_millis(3_810));
        clock.close();
        let rate = clock.rate().expect("rate");
        assert!((69_000..71_000).contains(&rate), "{rate}");
    }

    #[test]
    fn tool_runs_between_messages_are_not_generation() {
        let start = Instant::now();
        let mut clock = ClaudeGenerationClock::default();
        clock.message_started();
        clock.content(start);
        clock.output_tokens(70, start + Duration::from_secs(1));
        // A 60 s tool run, then the next message.
        clock.message_started();
        clock.content(start + Duration::from_secs(61));
        clock.output_tokens(140, start + Duration::from_secs(63));
        clock.close();
        assert_eq!(clock.rate(), Some(70_000));
    }

    #[test]
    fn single_frame_and_unreported_messages_stay_out() {
        let start = Instant::now();
        let mut clock = ClaudeGenerationClock::default();
        clock.message_started();
        clock.content(start);
        clock.close();
        clock.message_started();
        clock.content(start);
        clock.content(start + Duration::from_secs(2));
        clock.close();
        assert_eq!(clock.rate(), None);
    }
}
