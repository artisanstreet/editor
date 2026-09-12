//! Unconditional durable replay after a conversation subscription activates.
//!
//! The activation remains the single owner of the connection-local lease and
//! declared cursor. This seam performs exactly one transactionally consistent
//! repository read from those values and hands both the activation and the
//! exact repository outcome to the later delivery writer.

#![forbid(unsafe_code)]

use artisan_database::{ConversationPatchReplay, Repository, RepositoryError};

use artisan_domain::{EngineObservationEvent, ThreadId};

use crate::request_handler::ActivatedConversationSubscription;

/// Bounded page size for authoritative observation-history drains.
///
/// Keeps every `read_observation_history` call finite while still letting a
/// single wake or activation loop until the durable tail. A wake that arrives
/// mid-drain is preserved by the process-wide notifier and observed on the
/// next scan; no page is ever unbounded.
pub const OBSERVATION_HISTORY_PAGE_LIMIT: usize = 64;

/// The still-owned activation paired with its one bounded durable replay.
///
/// The activation is consumed into this owner, so no subscription authority is
/// cloned or fabricated while the repository read is in flight.
#[must_use]
#[derive(Debug, Eq, PartialEq)]
pub struct ActivatedConversationReplay {
    subscription: ActivatedConversationSubscription,
    replay: ConversationPatchReplay,
}

impl ActivatedConversationReplay {
    /// Returns the exact activation supplied to the replay read.
    pub const fn subscription(&self) -> &ActivatedConversationSubscription {
        &self.subscription
    }

    /// Returns the exact repository replay result.
    #[must_use]
    pub const fn replay(&self) -> &ConversationPatchReplay {
        &self.replay
    }

    /// Consumes the owner into its activation and replay result.
    pub fn into_parts(self) -> (ActivatedConversationSubscription, ConversationPatchReplay) {
        (self.subscription, self.replay)
    }
}

/// Performs one bounded durable replay from an activated subscription.
///
/// The thread and cursor come only from `subscription`. The repository owns
/// the transaction and replay classification; this function neither interprets
/// nor transforms the result.
///
/// # Errors
///
/// Returns the repository error unchanged when the durable read fails.
pub async fn read_activated_conversation_replay(
    repository: &Repository,
    subscription: ActivatedConversationSubscription,
) -> Result<ActivatedConversationReplay, RepositoryError> {
    let replay = repository
        .read_conversation_patch_replay(subscription.lease().thread_id(), subscription.cursor())
        .await?;
    Ok(ActivatedConversationReplay {
        subscription,
        replay,
    })
}

/// Performs one bounded authoritative observation-history read for an
/// activated subscription.
///
/// The thread comes only from `subscription.lease()`; callers supply the
/// subscriber's current thread-scoped `delivery_sequence` cursor (`after`) and
/// a bounded `limit`. The repository owns ordering (bounded ascending
/// `delivery_sequence`) and fencing; this function neither stamps clocks nor
/// casts turn ids, and it never synthesizes attribution.
///
/// # Errors
///
/// Returns the repository error unchanged when the durable read fails.
pub async fn read_activated_observation_history(
    repository: &Repository,
    subscription: &ActivatedConversationSubscription,
    after_sequence: u64,
    limit: usize,
) -> Result<Vec<EngineObservationEvent>, RepositoryError> {
    repository
        .read_observation_history(subscription.lease().thread_id(), after_sequence, limit)
        .await
}

/// Performs one bounded authoritative observation-history read for a bare
/// thread identity owned by the delivery driver.
///
/// `thread_id` must be the exact thread of an active subscription lease held
/// by the caller; this seam performs no lease lookup itself. Prefer
/// [`read_activated_observation_history`] when the activation is available.
///
/// # Errors
///
/// Returns [`RepositoryError`] when the durable observation read fails.
pub async fn read_observation_history_for_thread(
    repository: &Repository,
    thread_id: &ThreadId,
    after_sequence: u64,
    limit: usize,
) -> Result<Vec<EngineObservationEvent>, RepositoryError> {
    repository
        .read_observation_history(thread_id, after_sequence, limit)
        .await
}
