//! Dependency-free lifecycle policy for authoritative subscriptions.
//!
//! The legacy frontend keeps an authoritative stream alive with an Effect
//! scope and an infinite capped exponential retry schedule. This module keeps
//! only that deterministic decision boundary. It records states, observations,
//! and the actions an owning runtime must perform; it does not create scopes,
//! subscribe to a stream, apply an update, run recovery I/O, or wait for a
//! timer.
//!
//! A [`SubscriptionAttempt`] is an identity for one scope-owned attempt, not a
//! scope implementation. A new identity is allocated for every retry. The
//! caller must acknowledge [`AuthoritativeSubscriptionAction::FinalizeScope`]
//! before the policy can request recovery, which makes the legacy finalization
//! ordering explicit and testable.

use std::fmt;
use std::time::Duration;

/// Initial delay for the first retry after an attempt error.
pub const AUTHORITATIVE_SUBSCRIPTION_RETRY_INITIAL_DELAY_MS: u64 = 100;

/// Maximum delay for an authoritative-subscription retry.
pub const AUTHORITATIVE_SUBSCRIPTION_RETRY_MAX_DELAY_MS: u64 = 5_000;

/// Typed initial retry delay exposed to the eventual timer-owning caller.
pub const AUTHORITATIVE_SUBSCRIPTION_RETRY_INITIAL_DELAY: Duration =
    Duration::from_millis(AUTHORITATIVE_SUBSCRIPTION_RETRY_INITIAL_DELAY_MS);

/// Typed maximum retry delay exposed to the eventual timer-owning caller.
pub const AUTHORITATIVE_SUBSCRIPTION_RETRY_MAX_DELAY: Duration =
    Duration::from_millis(AUTHORITATIVE_SUBSCRIPTION_RETRY_MAX_DELAY_MS);

/// Exact message used when a subscription stream ends normally.
pub const AUTHORITATIVE_SUBSCRIPTION_LOST_MESSAGE: &str =
    "Authoritative subscription ended unexpectedly.";

/// Computes the capped exponential delay for a zero-based retry index.
///
/// Index zero is 100 ms, followed by 200, 400, 800, 1,600, and 3,200 ms.
/// Index six and every later index are capped at 5,000 ms. The loop stops as
/// soon as the cap is reached, so even `u64::MAX` cannot overflow or require
/// work proportional to the supplied index.
#[must_use]
pub const fn retry_delay(retry_index: u64) -> Duration {
    let mut delay_ms = AUTHORITATIVE_SUBSCRIPTION_RETRY_INITIAL_DELAY_MS;
    let mut remaining = retry_index;

    while remaining > 0 && delay_ms < AUTHORITATIVE_SUBSCRIPTION_RETRY_MAX_DELAY_MS {
        delay_ms = match delay_ms.checked_mul(2) {
            Some(candidate) if candidate <= AUTHORITATIVE_SUBSCRIPTION_RETRY_MAX_DELAY_MS => {
                candidate
            }
            _ => AUTHORITATIVE_SUBSCRIPTION_RETRY_MAX_DELAY_MS,
        };
        remaining -= 1;
    }

    Duration::from_millis(delay_ms)
}

/// Opaque identity for one attempt in the policy's lifetime.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct AttemptId(u64);

impl AttemptId {
    /// Returns the stable numeric identity for diagnostics and tests.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Opaque identity for the scope owned by one subscription attempt.
///
/// This is only a typed identity. The caller owns the actual scope and its
/// finalizer.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ScopeId(u64);

impl ScopeId {
    /// Returns the stable numeric identity for diagnostics and tests.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Identity pair carried by every action belonging to one attempt.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SubscriptionAttempt {
    attempt_id: AttemptId,
    scope_id: ScopeId,
}

impl SubscriptionAttempt {
    const fn new(attempt_id: AttemptId, scope_id: ScopeId) -> Self {
        Self {
            attempt_id,
            scope_id,
        }
    }

    /// Returns the attempt identity.
    #[must_use]
    pub const fn attempt_id(self) -> AttemptId {
        self.attempt_id
    }

    /// Returns the fresh scope identity owned by this attempt.
    #[must_use]
    pub const fn scope_id(self) -> ScopeId {
        self.scope_id
    }
}

/// Failure that ends the current subscription attempt.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum AuthoritativeSubscriptionFailure {
    /// The subscription effect failed before a stream was obtained.
    Subscribe,
    /// The stream failed while being consumed.
    Stream,
    /// The update handler failed while consuming a stream item.
    Update,
    /// The stream completed normally and was converted to the lost error.
    LostSubscription,
}

impl AuthoritativeSubscriptionFailure {
    /// Returns whether this is the synthetic failure for normal stream end.
    #[must_use]
    pub const fn is_lost_subscription(self) -> bool {
        matches!(self, Self::LostSubscription)
    }

    /// Returns the exact lost-subscription message, if this is that failure.
    #[must_use]
    pub const fn lost_subscription_message(self) -> Option<&'static str> {
        match self {
            Self::LostSubscription => Some(AUTHORITATIVE_SUBSCRIPTION_LOST_MESSAGE),
            Self::Subscribe | Self::Stream | Self::Update => None,
        }
    }
}

impl fmt::Display for AuthoritativeSubscriptionFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Subscribe => {
                formatter.write_str("authoritative subscription failed to subscribe")
            }
            Self::Stream => formatter.write_str("authoritative subscription stream failed"),
            Self::Update => formatter.write_str("authoritative subscription update failed"),
            Self::LostSubscription => formatter.write_str(AUTHORITATIVE_SUBSCRIPTION_LOST_MESSAGE),
        }
    }
}

impl std::error::Error for AuthoritativeSubscriptionFailure {}

/// Coarse state kind used in deterministic transition errors.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum AuthoritativeSubscriptionStateKind {
    /// No attempt has been started yet.
    Ready,
    /// The owning runtime is obtaining a stream for an attempt.
    Subscribing,
    /// The stream is live and may produce updates.
    Streaming,
    /// The attempt failed and its scope must be finalized.
    FinalizingScope,
    /// The failed attempt's scope is finalized and recovery is in progress.
    Recovering,
    /// Recovery completed or was absorbed and the caller owns the retry wait.
    WaitingToRetry,
}

/// Complete state of the deterministic subscription lifecycle.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum AuthoritativeSubscriptionState {
    /// The policy has not started its first attempt.
    Ready,
    /// A fresh scope-owned attempt is waiting for subscribe to finish.
    Subscribing {
        /// Attempt being subscribed.
        attempt: SubscriptionAttempt,
    },
    /// A stream was obtained for the current attempt.
    Streaming {
        /// Live attempt receiving updates.
        attempt: SubscriptionAttempt,
    },
    /// An attempt error has occurred; scope finalization is mandatory next.
    FinalizingScope {
        /// Failed attempt whose scope must be finalized.
        attempt: SubscriptionAttempt,
        /// Failure that ended the attempt.
        failure: AuthoritativeSubscriptionFailure,
    },
    /// Scope finalization completed; exactly one recovery action is next.
    Recovering {
        /// Attempt whose failure is being recovered.
        attempt: SubscriptionAttempt,
        /// Failure supplied to the recovery action.
        failure: AuthoritativeSubscriptionFailure,
    },
    /// The caller owns the timer/wait before starting the next attempt.
    WaitingToRetry {
        /// Attempt that just completed recovery.
        attempt: SubscriptionAttempt,
        /// Zero-based retry index used for `delay`.
        retry_index: u64,
        /// Delay returned to the caller; no timer is started here.
        delay: Duration,
    },
}

impl AuthoritativeSubscriptionState {
    /// Returns the coarse state kind without exposing lifecycle internals.
    #[must_use]
    pub const fn kind(self) -> AuthoritativeSubscriptionStateKind {
        match self {
            Self::Ready => AuthoritativeSubscriptionStateKind::Ready,
            Self::Subscribing { .. } => AuthoritativeSubscriptionStateKind::Subscribing,
            Self::Streaming { .. } => AuthoritativeSubscriptionStateKind::Streaming,
            Self::FinalizingScope { .. } => AuthoritativeSubscriptionStateKind::FinalizingScope,
            Self::Recovering { .. } => AuthoritativeSubscriptionStateKind::Recovering,
            Self::WaitingToRetry { .. } => AuthoritativeSubscriptionStateKind::WaitingToRetry,
        }
    }

    /// Returns the attempt carried by a non-ready state.
    #[must_use]
    pub const fn attempt(self) -> Option<SubscriptionAttempt> {
        match self {
            Self::Ready => None,
            Self::Subscribing { attempt }
            | Self::Streaming { attempt }
            | Self::FinalizingScope { attempt, .. }
            | Self::Recovering { attempt, .. }
            | Self::WaitingToRetry { attempt, .. } => Some(attempt),
        }
    }
}

/// Commands that the owner may execute outside this pure policy.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum AuthoritativeSubscriptionAction {
    /// Create a fresh scope and begin subscribe/stream work for `attempt`.
    StartAttempt {
        /// Fresh attempt and scope identity.
        attempt: SubscriptionAttempt,
    },
    /// Finalize the failed attempt's scope before any recovery or retry.
    FinalizeScope {
        /// Scope-owning attempt to finalize.
        attempt: SubscriptionAttempt,
    },
    /// Resynchronize the durable projection once for the failed attempt.
    Recover {
        /// Attempt whose failure triggered recovery.
        attempt: SubscriptionAttempt,
        /// Failure that is supplied to the recovery boundary.
        failure: AuthoritativeSubscriptionFailure,
    },
    /// Wait this long before asking the policy to start a fresh attempt.
    RetryAfterDelay {
        /// Attempt that has finished recovery.
        attempt: SubscriptionAttempt,
        /// Capped exponential delay selected for the next retry.
        delay: Duration,
    },
}

/// Observation accepted by [`AuthoritativeSubscriptionPolicy::apply`].
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum AuthoritativeSubscriptionEvent {
    /// Subscribe produced a stream for the current attempt.
    SubscribeSucceeded {
        /// Attempt that produced the stream.
        attempt: SubscriptionAttempt,
    },
    /// One update was accepted by the owner; the policy remains streaming.
    StreamUpdate {
        /// Attempt that produced the update.
        attempt: SubscriptionAttempt,
    },
    /// Subscribe failed and ends the attempt.
    SubscribeFailed {
        /// Failed attempt.
        attempt: SubscriptionAttempt,
    },
    /// Stream consumption failed and ends the attempt.
    StreamFailed {
        /// Failed attempt.
        attempt: SubscriptionAttempt,
    },
    /// Update handling failed and ends the attempt.
    UpdateFailed {
        /// Failed attempt.
        attempt: SubscriptionAttempt,
    },
    /// Normal stream completion converted to the exact lost-subscription failure.
    StreamEnded {
        /// Completed attempt.
        attempt: SubscriptionAttempt,
    },
    /// The owner has completed the failed attempt's scope finalizer.
    ScopeFinalized {
        /// Finalized attempt.
        attempt: SubscriptionAttempt,
    },
    /// Recovery completed successfully.
    RecoverySucceeded {
        /// Attempt being recovered.
        attempt: SubscriptionAttempt,
    },
    /// Recovery failed; the failure is intentionally absorbed.
    RecoveryFailed {
        /// Attempt being recovered.
        attempt: SubscriptionAttempt,
    },
    /// The caller's externally owned retry delay elapsed.
    RetryReady {
        /// Attempt whose retry delay elapsed.
        attempt: SubscriptionAttempt,
    },
}

/// One state transition and at most one external action.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct AuthoritativeSubscriptionTransition {
    /// State after the observation.
    pub state: AuthoritativeSubscriptionState,
    /// Action to execute, if this transition requires one.
    pub action: Option<AuthoritativeSubscriptionAction>,
}

impl AuthoritativeSubscriptionTransition {
    const fn new(
        state: AuthoritativeSubscriptionState,
        action: Option<AuthoritativeSubscriptionAction>,
    ) -> Self {
        Self { state, action }
    }
}

/// Invalid lifecycle input rejected without mutating policy state.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum AuthoritativeSubscriptionPolicyError {
    /// The operation does not belong to the current state kind.
    InvalidTransition {
        /// State that rejected the operation.
        state: AuthoritativeSubscriptionStateKind,
        /// Stable operation label for diagnostics.
        operation: &'static str,
    },
    /// An event from another attempt or scope tried to mutate this attempt.
    AttemptMismatch {
        /// Attempt currently owned by the policy.
        expected: SubscriptionAttempt,
        /// Attempt named by the stale event.
        actual: SubscriptionAttempt,
    },
    /// The finite identity representation cannot allocate another attempt.
    AttemptIdExhausted,
}

impl fmt::Display for AuthoritativeSubscriptionPolicyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidTransition { state, operation } => {
                write!(
                    formatter,
                    "cannot {operation} while subscription is {state:?}"
                )
            }
            Self::AttemptMismatch { expected, actual } => write!(
                formatter,
                "subscription event addressed attempt {:?}, expected {:?}",
                actual, expected
            ),
            Self::AttemptIdExhausted => {
                formatter.write_str("authoritative subscription attempt identity is exhausted")
            }
        }
    }
}

impl std::error::Error for AuthoritativeSubscriptionPolicyError {}

/// Deterministic state machine for one authoritative subscription.
///
/// The policy has no terminal retry state: after each accepted recovery
/// outcome, [`AuthoritativeSubscriptionEvent::RetryReady`] allocates another
/// attempt with another scope identity. Recovery success and failure follow
/// the same path; a recovery failure is therefore absorbed exactly as in the
/// legacy `catch(() => void)` boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthoritativeSubscriptionPolicy {
    state: AuthoritativeSubscriptionState,
    next_attempt_id: u64,
    next_scope_id: u64,
    next_retry_index: u64,
}

impl Default for AuthoritativeSubscriptionPolicy {
    fn default() -> Self {
        Self::new()
    }
}

impl AuthoritativeSubscriptionPolicy {
    /// Creates a policy that is ready to start its first attempt.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            state: AuthoritativeSubscriptionState::Ready,
            next_attempt_id: 0,
            next_scope_id: 0,
            next_retry_index: 0,
        }
    }

    /// Returns the current typed lifecycle state.
    #[must_use]
    pub const fn state(&self) -> AuthoritativeSubscriptionState {
        self.state
    }

    /// Returns the current attempt, if the policy has begun one.
    #[must_use]
    pub const fn current_attempt(&self) -> Option<SubscriptionAttempt> {
        self.state.attempt()
    }

    /// Returns the next zero-based retry index that will be scheduled.
    #[must_use]
    pub const fn next_retry_index(&self) -> u64 {
        self.next_retry_index
    }

    /// Starts the first fresh scope-owned attempt.
    pub fn begin_attempt(
        &mut self,
    ) -> Result<AuthoritativeSubscriptionTransition, AuthoritativeSubscriptionPolicyError> {
        if self.state != AuthoritativeSubscriptionState::Ready {
            return Err(self.invalid("begin an attempt"));
        }

        self.start_fresh_attempt()
    }

    /// Applies one typed external observation to the state machine.
    pub fn apply(
        &mut self,
        event: AuthoritativeSubscriptionEvent,
    ) -> Result<AuthoritativeSubscriptionTransition, AuthoritativeSubscriptionPolicyError> {
        match event {
            AuthoritativeSubscriptionEvent::SubscribeSucceeded { attempt } => {
                self.require_attempt(
                    "accept subscribe",
                    AuthoritativeSubscriptionStateKind::Subscribing,
                    attempt,
                )?;
                self.state = AuthoritativeSubscriptionState::Streaming { attempt };
                Ok(self.transition(None))
            }
            AuthoritativeSubscriptionEvent::StreamUpdate { attempt } => {
                self.require_attempt(
                    "accept stream update",
                    AuthoritativeSubscriptionStateKind::Streaming,
                    attempt,
                )?;
                Ok(self.transition(None))
            }
            AuthoritativeSubscriptionEvent::SubscribeFailed { attempt } => self.end_attempt(
                attempt,
                AuthoritativeSubscriptionFailure::Subscribe,
                AuthoritativeSubscriptionStateKind::Subscribing,
            ),
            AuthoritativeSubscriptionEvent::StreamFailed { attempt } => self.end_attempt(
                attempt,
                AuthoritativeSubscriptionFailure::Stream,
                AuthoritativeSubscriptionStateKind::Streaming,
            ),
            AuthoritativeSubscriptionEvent::UpdateFailed { attempt } => self.end_attempt(
                attempt,
                AuthoritativeSubscriptionFailure::Update,
                AuthoritativeSubscriptionStateKind::Streaming,
            ),
            AuthoritativeSubscriptionEvent::StreamEnded { attempt } => self.end_attempt(
                attempt,
                AuthoritativeSubscriptionFailure::LostSubscription,
                AuthoritativeSubscriptionStateKind::Streaming,
            ),
            AuthoritativeSubscriptionEvent::ScopeFinalized { attempt } => {
                let failure = match self.state {
                    AuthoritativeSubscriptionState::FinalizingScope {
                        attempt: expected,
                        failure,
                    } => {
                        self.ensure_same_attempt(expected, attempt)?;
                        failure
                    }
                    _ => {
                        return Err(self.invalid("finalize the attempt scope"));
                    }
                };
                self.state = AuthoritativeSubscriptionState::Recovering { attempt, failure };
                Ok(
                    self.transition(Some(AuthoritativeSubscriptionAction::Recover {
                        attempt,
                        failure,
                    })),
                )
            }
            AuthoritativeSubscriptionEvent::RecoverySucceeded { attempt }
            | AuthoritativeSubscriptionEvent::RecoveryFailed { attempt } => {
                let expected = match self.state {
                    AuthoritativeSubscriptionState::Recovering { attempt, .. } => attempt,
                    _ => return Err(self.invalid("complete recovery")),
                };
                self.ensure_same_attempt(expected, attempt)?;
                self.schedule_retry(attempt)
            }
            AuthoritativeSubscriptionEvent::RetryReady { attempt } => {
                let expected = match self.state {
                    AuthoritativeSubscriptionState::WaitingToRetry { attempt, .. } => attempt,
                    _ => return Err(self.invalid("start the next retry")),
                };
                self.ensure_same_attempt(expected, attempt)?;
                self.start_fresh_attempt()
            }
        }
    }

    /// Records successful subscribe and enters stream consumption.
    pub fn subscribe_succeeded(
        &mut self,
        attempt: SubscriptionAttempt,
    ) -> Result<AuthoritativeSubscriptionTransition, AuthoritativeSubscriptionPolicyError> {
        self.apply(AuthoritativeSubscriptionEvent::SubscribeSucceeded { attempt })
    }

    /// Records one update accepted by the owner without applying it here.
    pub fn stream_update(
        &mut self,
        attempt: SubscriptionAttempt,
    ) -> Result<AuthoritativeSubscriptionTransition, AuthoritativeSubscriptionPolicyError> {
        self.apply(AuthoritativeSubscriptionEvent::StreamUpdate { attempt })
    }

    /// Records subscribe failure and requests scope finalization.
    pub fn subscribe_failed(
        &mut self,
        attempt: SubscriptionAttempt,
    ) -> Result<AuthoritativeSubscriptionTransition, AuthoritativeSubscriptionPolicyError> {
        self.apply(AuthoritativeSubscriptionEvent::SubscribeFailed { attempt })
    }

    /// Records stream failure and requests scope finalization.
    pub fn stream_failed(
        &mut self,
        attempt: SubscriptionAttempt,
    ) -> Result<AuthoritativeSubscriptionTransition, AuthoritativeSubscriptionPolicyError> {
        self.apply(AuthoritativeSubscriptionEvent::StreamFailed { attempt })
    }

    /// Records update failure and requests scope finalization.
    pub fn update_failed(
        &mut self,
        attempt: SubscriptionAttempt,
    ) -> Result<AuthoritativeSubscriptionTransition, AuthoritativeSubscriptionPolicyError> {
        self.apply(AuthoritativeSubscriptionEvent::UpdateFailed { attempt })
    }

    /// Converts normal stream completion into the lost-subscription failure.
    pub fn stream_ended(
        &mut self,
        attempt: SubscriptionAttempt,
    ) -> Result<AuthoritativeSubscriptionTransition, AuthoritativeSubscriptionPolicyError> {
        self.apply(AuthoritativeSubscriptionEvent::StreamEnded { attempt })
    }

    /// Acknowledges scope finalization and requests exactly one recovery.
    pub fn scope_finalized(
        &mut self,
        attempt: SubscriptionAttempt,
    ) -> Result<AuthoritativeSubscriptionTransition, AuthoritativeSubscriptionPolicyError> {
        self.apply(AuthoritativeSubscriptionEvent::ScopeFinalized { attempt })
    }

    /// Accepts successful recovery and returns the next retry delay.
    pub fn recovery_succeeded(
        &mut self,
        attempt: SubscriptionAttempt,
    ) -> Result<AuthoritativeSubscriptionTransition, AuthoritativeSubscriptionPolicyError> {
        self.apply(AuthoritativeSubscriptionEvent::RecoverySucceeded { attempt })
    }

    /// Absorbs failed recovery and returns the same next retry delay.
    pub fn recovery_failed(
        &mut self,
        attempt: SubscriptionAttempt,
    ) -> Result<AuthoritativeSubscriptionTransition, AuthoritativeSubscriptionPolicyError> {
        self.apply(AuthoritativeSubscriptionEvent::RecoveryFailed { attempt })
    }

    /// Starts the next attempt after the caller's externally owned delay.
    pub fn retry_ready(
        &mut self,
        attempt: SubscriptionAttempt,
    ) -> Result<AuthoritativeSubscriptionTransition, AuthoritativeSubscriptionPolicyError> {
        self.apply(AuthoritativeSubscriptionEvent::RetryReady { attempt })
    }

    fn start_fresh_attempt(
        &mut self,
    ) -> Result<AuthoritativeSubscriptionTransition, AuthoritativeSubscriptionPolicyError> {
        let attempt_id = self
            .next_attempt_id
            .checked_add(1)
            .ok_or(AuthoritativeSubscriptionPolicyError::AttemptIdExhausted)?;
        let scope_id = self
            .next_scope_id
            .checked_add(1)
            .ok_or(AuthoritativeSubscriptionPolicyError::AttemptIdExhausted)?;
        self.next_attempt_id = attempt_id;
        self.next_scope_id = scope_id;

        let attempt = SubscriptionAttempt::new(AttemptId(attempt_id), ScopeId(scope_id));
        self.state = AuthoritativeSubscriptionState::Subscribing { attempt };
        Ok(
            self.transition(Some(AuthoritativeSubscriptionAction::StartAttempt {
                attempt,
            })),
        )
    }

    fn end_attempt(
        &mut self,
        attempt: SubscriptionAttempt,
        failure: AuthoritativeSubscriptionFailure,
        expected_kind: AuthoritativeSubscriptionStateKind,
    ) -> Result<AuthoritativeSubscriptionTransition, AuthoritativeSubscriptionPolicyError> {
        self.require_attempt("end the attempt", expected_kind, attempt)?;

        self.state = AuthoritativeSubscriptionState::FinalizingScope { attempt, failure };
        Ok(
            self.transition(Some(AuthoritativeSubscriptionAction::FinalizeScope {
                attempt,
            })),
        )
    }

    fn schedule_retry(
        &mut self,
        attempt: SubscriptionAttempt,
    ) -> Result<AuthoritativeSubscriptionTransition, AuthoritativeSubscriptionPolicyError> {
        let retry_index = self.next_retry_index;
        let delay = retry_delay(retry_index);
        self.next_retry_index = self.next_retry_index.saturating_add(1);
        self.state = AuthoritativeSubscriptionState::WaitingToRetry {
            attempt,
            retry_index,
            delay,
        };
        Ok(
            self.transition(Some(AuthoritativeSubscriptionAction::RetryAfterDelay {
                attempt,
                delay,
            })),
        )
    }

    fn transition(
        &self,
        action: Option<AuthoritativeSubscriptionAction>,
    ) -> AuthoritativeSubscriptionTransition {
        AuthoritativeSubscriptionTransition::new(self.state, action)
    }

    fn ensure_same_attempt(
        &self,
        expected: SubscriptionAttempt,
        actual: SubscriptionAttempt,
    ) -> Result<(), AuthoritativeSubscriptionPolicyError> {
        if expected == actual {
            Ok(())
        } else {
            Err(AuthoritativeSubscriptionPolicyError::AttemptMismatch { expected, actual })
        }
    }

    fn require_attempt(
        &self,
        operation: &'static str,
        expected_kind: AuthoritativeSubscriptionStateKind,
        actual: SubscriptionAttempt,
    ) -> Result<(), AuthoritativeSubscriptionPolicyError> {
        if self.state.kind() != expected_kind {
            return Err(self.invalid(operation));
        }
        let expected = self
            .state
            .attempt()
            .expect("non-ready state kinds always carry an attempt");
        self.ensure_same_attempt(expected, actual)
    }

    fn invalid(&self, operation: &'static str) -> AuthoritativeSubscriptionPolicyError {
        AuthoritativeSubscriptionPolicyError::InvalidTransition {
            state: self.state.kind(),
            operation,
        }
    }
}

/// Conversation subscriptions use the authoritative policy without a copy.
pub type ConversationSubscriptionPolicy = AuthoritativeSubscriptionPolicy;

/// Conversation subscription state is the authoritative state verbatim.
pub type ConversationSubscriptionState = AuthoritativeSubscriptionState;

/// Conversation subscription actions are the authoritative actions verbatim.
pub type ConversationSubscriptionAction = AuthoritativeSubscriptionAction;

/// Conversation subscription attempts use the same scope-owned identity.
pub type ConversationSubscriptionAttempt = SubscriptionAttempt;

/// Legacy naming for the conversation runner's policy boundary.
pub type RunConversationSubscriptionPolicy = AuthoritativeSubscriptionPolicy;
