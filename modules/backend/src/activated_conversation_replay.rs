//! Unconditional durable replay after a conversation subscription activates.
//!
//! The activation remains the single owner of the connection-local lease and
//! declared cursor. This seam performs exactly one transactionally consistent
//! repository read from those values and hands both the activation and the
//! exact repository outcome to the later delivery writer.

#![forbid(unsafe_code)]

use artisan_database::{ConversationPatchReplay, Repository, RepositoryError};

use crate::request_handler::ActivatedConversationSubscription;

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
