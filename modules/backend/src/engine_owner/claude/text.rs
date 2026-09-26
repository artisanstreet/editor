//! Claude assistant-text reconciliation: streamed deltas against buffered
//! blocks.
//!
//! With `--include-partial-messages` the CLI streams each text block as
//! `text_delta` fragments and then emits one buffered `assistant` frame
//! carrying that block's full text (see `tests/fixtures/claude/manifest.json`).
//! Streamed fragments are provisional; the buffered text is authoritative for
//! its message. The ledger tracks what the current message part already
//! holds, so the buffered block settles it exactly once: an identical block
//! emits nothing, a block the stream only started appends its missing
//! suffix, and a diverging block replaces the whole message part. A buffered
//! block that arrives without partials appends in full. Child frames
//! (`parent_tool_use_id`) route before root decoding and never reach this
//! ledger.

/// How one buffered text block settles the current message part.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ClaudeTextSettlement {
    /// The streamed text already equals the authoritative block.
    Settled,
    /// Append these bytes: no partials arrived, or they stopped short.
    Append(String),
    /// Replace the whole message part with this authoritative body.
    Replace(String),
}

/// Run-local text ledger for the current provider message.
#[derive(Debug, Default)]
pub(crate) struct ClaudeTextLedger {
    message_id: Option<String>,
    /// Everything projected into the current message part.
    body: String,
    /// Leading bytes of `body` already settled by buffered blocks.
    settled: usize,
}

impl ClaudeTextLedger {
    /// Starts a fresh message part.
    pub(crate) fn message_started(&mut self, message_id: &str) {
        self.message_id = Some(message_id.to_owned());
        self.body.clear();
        self.settled = 0;
    }

    /// Records one provisional streamed fragment of the current message.
    pub(crate) fn streamed(&mut self, delta: &str) {
        self.body.push_str(delta);
    }

    /// Settles the provisional text with one buffered block's authoritative
    /// text.
    ///
    /// A block naming another message than the streamed one has no
    /// provisional text here and appends in full without disturbing the
    /// current message.
    pub(crate) fn buffered(
        &mut self,
        message_id: Option<&str>,
        text: &str,
    ) -> ClaudeTextSettlement {
        if let (Some(frame), Some(current)) = (message_id, self.message_id.as_deref())
            && frame != current
        {
            return ClaudeTextSettlement::Append(text.to_owned());
        }
        let pending = &self.body[self.settled..];
        let settlement = if pending == text {
            ClaudeTextSettlement::Settled
        } else if let Some(missing) = text.strip_prefix(pending) {
            ClaudeTextSettlement::Append(missing.to_owned())
        } else {
            ClaudeTextSettlement::Replace(format!("{}{text}", &self.body[..self.settled]))
        };
        self.body.truncate(self.settled);
        self.body.push_str(text);
        self.settled = self.body.len();
        settlement
    }

    /// Returns the current message part as projected so far.
    pub(crate) fn body(&self) -> &str {
        &self.body
    }
}

#[cfg(test)]
mod ledger_tests {
    use super::{ClaudeTextLedger, ClaudeTextSettlement};

    #[test]
    fn buffered_blocks_settle_streamed_text_exactly_once() {
        let mut ledger = ClaudeTextLedger::default();
        ledger.message_started("msg-1");
        ledger.streamed("Hel");
        ledger.streamed("lo");
        assert_eq!(
            ledger.buffered(Some("msg-1"), "Hello"),
            ClaudeTextSettlement::Settled
        );
        // A second block: the stream stopped short, then diverged.
        ledger.streamed(" wor");
        assert_eq!(
            ledger.buffered(Some("msg-1"), " world"),
            ClaudeTextSettlement::Append("ld".to_owned())
        );
        ledger.streamed(" draft");
        assert_eq!(
            ledger.buffered(None, " final"),
            ClaudeTextSettlement::Replace("Hello world final".to_owned())
        );
        assert_eq!(ledger.body(), "Hello world final");
        // Without partials a block appends in full.
        assert_eq!(
            ledger.buffered(Some("msg-1"), "!"),
            ClaudeTextSettlement::Append("!".to_owned())
        );
        // A block of another message never touches the current part.
        assert_eq!(
            ledger.buffered(Some("msg-other"), "elsewhere"),
            ClaudeTextSettlement::Append("elsewhere".to_owned())
        );
        assert_eq!(ledger.body(), "Hello world final!");
        ledger.message_started("msg-2");
        assert_eq!(ledger.body(), "");
    }
}
