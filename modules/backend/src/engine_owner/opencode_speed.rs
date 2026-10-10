//! OpenCode2 generation speed, measured on OpenCode's own clock.
//!
//! OpenCode stamps every event envelope with the server time it was
//! published (`created`, Unix milliseconds). A step is one model call:
//! `session.step.started` opens it and `session.step.ended` closes it with
//! the step's `output` and `reasoning` tokens, so the speed needs no receipt
//! timing at all. The window starts at the step, not at its first output
//! (`session.step.streamed`): a reasoning model generates its hidden
//! reasoning before any output, then often sends the visible text in one
//! burst, so stored steps show first output and completion 1 ms apart. The
//! window therefore includes time to first token, which can only lower the
//! speed. Tool runs fall between steps and never count; the turn's speed
//! sums its steps.

use std::collections::HashMap;

use serde_json::Value;

/// Shortest generation time that yields a speed, in milliseconds.
const MIN_ELAPSED_MS: u64 = 1_000;
/// Fewest generated tokens that yield a speed.
const MIN_TOKENS: u64 = 16;
/// Open steps kept at once; a turn runs its steps one after another.
const MAX_OPEN_STEPS: usize = 64;

/// Generation windows of one turn's steps, keyed by assistant message.
#[derive(Debug, Default)]
pub(crate) struct OpenCodeStepClock {
    /// When each open step started generating, by assistant message id.
    open: HashMap<String, u64>,
    tokens: u64,
    elapsed_ms: u64,
}

impl OpenCodeStepClock {
    /// Notes one envelope; only step events change the clock.
    pub(crate) fn observe(&mut self, payload: &str) {
        if !payload.contains("session.step.") {
            return;
        }
        let Ok(envelope) = serde_json::from_str::<Value>(payload) else {
            return;
        };
        let (Some(kind), Some(created), Some(data)) = (
            envelope.get("type").and_then(Value::as_str),
            envelope.get("created").and_then(Value::as_u64),
            envelope.get("data"),
        ) else {
            return;
        };
        let Some(message) = data.get("assistantMessageID").and_then(Value::as_str) else {
            return;
        };
        match kind {
            "session.step.started" => {
                if self.open.len() < MAX_OPEN_STEPS || self.open.contains_key(message) {
                    self.open.insert(message.to_owned(), created);
                }
            }
            "session.step.ended" => {
                let Some(start) = self.open.remove(message) else {
                    return;
                };
                let tokens = data.get("tokens");
                let count = |field: &str| {
                    tokens
                        .and_then(|tokens| tokens.get(field))
                        .and_then(Value::as_u64)
                        .unwrap_or(0)
                };
                let generated = count("output").saturating_add(count("reasoning"));
                if let Some(elapsed) = created.checked_sub(start).filter(|elapsed| *elapsed > 0)
                    && generated > 0
                {
                    self.tokens = self.tokens.saturating_add(generated);
                    self.elapsed_ms = self.elapsed_ms.saturating_add(elapsed);
                }
            }
            "session.step.failed" => {
                self.open.remove(message);
            }
            _ => {}
        }
    }

    /// Generated tokens per second across the turn's ended steps, multiplied
    /// by 1000; `None` until enough generation was measured.
    pub(crate) fn rate(&self) -> Option<u32> {
        if self.elapsed_ms < MIN_ELAPSED_MS || self.tokens < MIN_TOKENS {
            return None;
        }
        u32::try_from(u128::from(self.tokens).checked_mul(1_000_000)? / u128::from(self.elapsed_ms))
            .ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(kind: &str, created: u64, message: &str, tokens: Option<(u64, u64)>) -> String {
        let tokens = tokens.map_or(String::new(), |(output, reasoning)| {
            format!(r#","tokens":{{"input":10,"output":{output},"reasoning":{reasoning},"cache":{{"read":0,"write":0}}}}"#)
        });
        format!(
            r#"{{"id":"e","created":{created},"type":"{kind}","durable":{{"aggregateID":"s","seq":1,"version":1}},"data":{{"sessionID":"s","assistantMessageID":"{message}"{tokens}}}}}"#
        )
    }

    #[test]
    fn a_step_counts_output_and_reasoning_over_the_whole_step() {
        let mut clock = OpenCodeStepClock::default();
        clock.observe(&step("session.step.started", 1_000, "m1", None));
        // Hidden reasoning, then the visible text in one burst: first output
        // and completion 1 ms apart must not shrink the window.
        clock.observe(&step("session.step.streamed", 2_999, "m1", None));
        clock.observe(&step("session.step.ended", 3_000, "m1", Some((100, 40))));
        assert_eq!(clock.rate(), Some(70_000));
    }

    #[test]
    fn tool_runs_between_steps_never_count() {
        let mut clock = OpenCodeStepClock::default();
        clock.observe(&step("session.step.started", 0, "m1", None));
        clock.observe(&step("session.step.ended", 1_000, "m1", Some((70, 0))));
        clock.observe(&step("session.step.started", 61_000, "m2", None));
        clock.observe(&step("session.step.ended", 63_000, "m2", Some((140, 0))));
        assert_eq!(clock.rate(), Some(70_000));
    }

    #[test]
    fn failed_and_unstarted_steps_and_other_events_stay_out() {
        let mut clock = OpenCodeStepClock::default();
        clock.observe(&step("session.step.started", 0, "m1", None));
        clock.observe(&step("session.step.failed", 5_000, "m1", None));
        clock.observe(&step("session.step.ended", 9_000, "m2", Some((500, 0))));
        clock.observe(r#"{"type":"session.text.delta","created":1,"data":{}}"#);
        assert_eq!(clock.rate(), None);
    }
}
