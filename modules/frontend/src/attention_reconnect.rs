//! Pure attention-return reconnect policy.
//!
//! This is the native counterpart of
//! `modules/frontend/src/lib/runtime/attention-reconnect.ts`. It consumes
//! already-observed `watching` values and returns an intent; the caller owns
//! `RetryConnection` execution and failure handling. Host access, timing,
//! retry budgets, asynchronous work, and global state remain outside this
//! boundary.

/// The effect-free outcome of one observed reader-attention value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AttentionReconnectIntent {
    /// The observation does not authorize a reconnect attempt.
    NoRetry,
    /// The caller may invoke the existing `RetryConnection` capability and
    /// ignore its failure.
    RetryConnection,
}

/// Stateful value policy for the attention-return reconnect boundary.
///
/// The policy starts without an observation. Its first `watching` value only
/// establishes the current state. Later equal values are ignored like
/// `Stream::changes`; only a `false` to `true` transition returns
/// [`AttentionReconnectIntent::RetryConnection`]. A transition to `false`
/// and every other observation return [`AttentionReconnectIntent::NoRetry`].
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AttentionReconnectPolicy {
    last_watching: Option<bool>,
}

impl AttentionReconnectPolicy {
    /// Creates a policy that has not observed an attention value yet.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            last_watching: None,
        }
    }

    /// Records one observed `watching` value and returns its reconnect intent.
    ///
    /// The first observation establishes state without authorizing a retry.
    /// Subsequent equal observations and transitions to `false` also return
    /// [`AttentionReconnectIntent::NoRetry`]. Each later `false` to `true`
    /// transition returns [`AttentionReconnectIntent::RetryConnection`]; the
    /// caller may then invoke the existing retry capability and ignore its
    /// failure.
    #[must_use]
    pub const fn observe(&mut self, watching: bool) -> AttentionReconnectIntent {
        let intent = match self.last_watching {
            Some(previous) if !previous && watching => AttentionReconnectIntent::RetryConnection,
            _ => AttentionReconnectIntent::NoRetry,
        };

        self.last_watching = Some(watching);
        intent
    }

    /// Returns the most recently observed attention value, if initialized.
    #[must_use]
    pub const fn last_watching(&self) -> Option<bool> {
        self.last_watching
    }
}
