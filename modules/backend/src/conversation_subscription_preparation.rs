//! Durable conversation subscription preparation seam.
//!
//! Prepares fresh and resume acknowledgements from durable state and registers
//! the resulting thread as `SubscriptionState::Pending` at exactly the cursor
//! declared to the client. The seam never activates delivery, publishes patches,
//! notifies, writes transport frames, or owns a connection. A successful
//! durable read always precedes any registry mutation, and successful
//! preparation only registers `Pending`; activation remains the caller’s
//! responsibility after the Subscribe response is proven written. Prepared
//! receipts do not prove response delivery, and no task, queue, stream, clock,
//! or unsafe code is involved.

#![forbid(unsafe_code)]

use artisan_database::{ConversationPatchReplay, Repository, RepositoryError};
use artisan_domain::{
    CONVERSATION_QUERY_MAX_TURNS, ConversationCursor, ConversationQuery, ConversationQueryBounds,
    ConversationSubscribe, ConversationSubscriptionStart, ConversationUnsubscribe, QueryTurnCount,
};
use artisan_protocol::{ConversationSubscriptionStarted, ConversationSubscriptionStopped};
use thiserror::Error;

use crate::conversation_subscription_registry::{
    ConversationSubscriptionRegistry, RegisterError, SubscriptionLease, UnsubscribeOutcome,
};

/// Failure while preparing a conversation subscription.
///
/// Repository and registry failures preserve their typed sources without
/// fabricating durability or masking message content.
#[derive(Debug, Error)]
pub enum PrepareSubscriptionError {
    /// The durable snapshot or patch-replay read failed.
    #[error(transparent)]
    Repository(#[from] RepositoryError),
    /// The per-connection generation for the new lease is exhausted.
    #[error(transparent)]
    Register(#[from] RegisterError),
    /// The requested resume cursor is beyond the durable tail and requires a
    /// fresh snapshot.
    #[error("resnapshot required")]
    ResnapshotRequired {
        /// Cursor requested by the client.
        requested_cursor: ConversationCursor,
        /// Durable tail cursor at the validated snapshot.
        current_cursor: ConversationCursor,
    },
}

/// Fresh or resumed subscription acknowledgement paired with its fencing lease.
///
/// The entry is always `Pending`; the driver must activate it only after the
/// Subscribe response write is proven. No patch is cached or published here —
/// post-response activation unconditionally re-reads from the registered cursor,
/// so commits between preparation and activation remain authoritative.
#[derive(Debug, Eq, PartialEq)]
pub struct PreparedConversationSubscription {
    started: ConversationSubscriptionStarted,
    lease: SubscriptionLease,
}

impl PreparedConversationSubscription {
    /// Returns the subscription acknowledgement to be sent to the client.
    #[must_use]
    pub const fn started(&self) -> &ConversationSubscriptionStarted {
        &self.started
    }

    /// Returns the connection-local lease fencing this pending registration.
    #[must_use]
    pub const fn lease(&self) -> &SubscriptionLease {
        &self.lease
    }

    /// Consumes the prepared subscription into its owned parts.
    #[must_use]
    pub fn into_parts(self) -> (ConversationSubscriptionStarted, SubscriptionLease) {
        (self.started, self.lease)
    }
}

/// Prepares a fresh or resume subscription from durable state and registers it
/// as `Pending` at exactly the cursor declared to the client.
///
/// Fresh subscribes read one repository snapshot using
/// [`ConversationQueryBounds::Window`] with exactly
/// [`CONVERSATION_QUERY_MAX_TURNS`] before any registry mutation and register
/// `Pending` at `snapshot.cursor()` with `Fresh(Start(snapshot))`.
///
/// Resume subscribes call `Repository::read_conversation_patch_replay` once.
/// `Current` and `Batch` both register `Pending` at the requested `after`
/// cursor; any returned batch is intentionally not cached or published so the
/// later activation re-read remains authoritative. `ResnapshotRequired` is
/// mapped losslessly and makes no registry mutation.
///
/// A successful repeated subscribe for the same thread replaces the current
/// entry exactly once through `register_pending`; the prior lease becomes
/// stale. Repository and generation failures make no partial mutation beyond
/// the registry method’s atomic contract. This function never activates,
/// publishes, notifies, or proves response delivery.
///
/// # Errors
///
/// Returns [`PrepareSubscriptionError::Repository`] when the durable read fails,
/// [`PrepareSubscriptionError::Register`] when the connection-local generation
/// is exhausted, or [`PrepareSubscriptionError::ResnapshotRequired`] when the
/// requested cursor is beyond the durable tail. `ResnapshotRequired` and
/// registry exhaustion leave existing registry state unchanged.
///
/// # Panics
///
/// Panics if `CONVERSATION_QUERY_MAX_TURNS` is not within the validated
/// `QueryTurnCount` bounds. This indicates a programming error; the documented
/// constant is 512 and always validates.
pub async fn prepare_conversation_subscription(
    repository: &Repository,
    registry: &mut ConversationSubscriptionRegistry,
    subscribe: &ConversationSubscribe,
) -> Result<PreparedConversationSubscription, PrepareSubscriptionError> {
    match &subscribe.after {
        None => {
            let query = ConversationQuery {
                thread_id: subscribe.thread_id.clone(),
                bounds: ConversationQueryBounds::Window {
                    maximum_turn_count: QueryTurnCount::new(u64::from(
                        CONVERSATION_QUERY_MAX_TURNS,
                    ))
                    .expect("CONVERSATION_QUERY_MAX_TURNS is within validated bounds"),
                },
            };
            let snapshot = repository.read_conversation_snapshot(&query).await?;
            let cursor = snapshot.cursor();
            let lease = registry.register_pending(subscribe.thread_id.clone(), cursor)?;
            let started = ConversationSubscriptionStarted::Fresh(
                ConversationSubscriptionStart::new(snapshot),
            );
            Ok(PreparedConversationSubscription { started, lease })
        }
        Some(after) => {
            let replay = repository
                .read_conversation_patch_replay(&subscribe.thread_id, *after)
                .await?;
            match replay {
                ConversationPatchReplay::ResnapshotRequired {
                    requested_cursor,
                    current_cursor,
                } => Err(PrepareSubscriptionError::ResnapshotRequired {
                    requested_cursor,
                    current_cursor,
                }),
                ConversationPatchReplay::Current { .. } | ConversationPatchReplay::Batch(_) => {
                    let lease = registry.register_pending(subscribe.thread_id.clone(), *after)?;
                    let started = ConversationSubscriptionStarted::Resumed {
                        thread_id: subscribe.thread_id.clone(),
                        cursor: *after,
                    };
                    Ok(PreparedConversationSubscription { started, lease })
                }
            }
        }
    }
}

/// Idempotent stop acknowledgement paired with the registry outcome.
///
/// Always produced by calling `unsubscribe` exactly once; `Absent` does not
/// fabricate a removed entry.
#[derive(Debug, Eq, PartialEq)]
pub struct StoppedConversationSubscription {
    response: ConversationSubscriptionStopped,
    outcome: UnsubscribeOutcome,
}

impl StoppedConversationSubscription {
    /// Returns the protocol acknowledgement for the stop.
    #[must_use]
    pub const fn response(&self) -> &ConversationSubscriptionStopped {
        &self.response
    }

    /// Returns the exact registry outcome (`Removed` or `Absent`).
    #[must_use]
    pub const fn outcome(&self) -> &UnsubscribeOutcome {
        &self.outcome
    }

    /// Consumes the stopped subscription into its owned parts.
    #[must_use]
    pub fn into_parts(self) -> (ConversationSubscriptionStopped, UnsubscribeOutcome) {
        (self.response, self.outcome)
    }
}

/// Removes the current subscription entry for `unsubscribe.thread_id` and
/// returns the protocol acknowledgement with the exact outcome.
///
/// Always calls `ConversationSubscriptionRegistry::unsubscribe` exactly once
/// and returns `ConversationSubscriptionStopped { thread_id }` for both
/// `Removed` and `Absent`; stop is idempotent and never fabricates a removal.
pub fn stop_conversation_subscription(
    registry: &mut ConversationSubscriptionRegistry,
    unsubscribe: &ConversationUnsubscribe,
) -> StoppedConversationSubscription {
    let outcome = registry.unsubscribe(&unsubscribe.thread_id);
    let response = ConversationSubscriptionStopped {
        thread_id: unsubscribe.thread_id.clone(),
    };
    StoppedConversationSubscription { response, outcome }
}
