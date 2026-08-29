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

#[derive(Debug, Error)]
pub enum PrepareSubscriptionError {
    #[error(transparent)]
    Repository(#[from] RepositoryError),
    #[error(transparent)]
    Register(#[from] RegisterError),
    #[error("resnapshot required")]
    ResnapshotRequired {
        requested_cursor: ConversationCursor,
        current_cursor: ConversationCursor,
    },
}

#[derive(Debug, Eq, PartialEq)]
pub struct PreparedConversationSubscription {
    started: ConversationSubscriptionStarted,
    lease: SubscriptionLease,
}

impl PreparedConversationSubscription {
    pub const fn started(&self) -> &ConversationSubscriptionStarted {
        &self.started
    }

    pub const fn lease(&self) -> &SubscriptionLease {
        &self.lease
    }

    pub fn into_parts(self) -> (ConversationSubscriptionStarted, SubscriptionLease) {
        (self.started, self.lease)
    }
}

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

#[derive(Debug, Eq, PartialEq)]
pub struct StoppedConversationSubscription {
    response: ConversationSubscriptionStopped,
    outcome: UnsubscribeOutcome,
}

impl StoppedConversationSubscription {
    pub const fn response(&self) -> &ConversationSubscriptionStopped {
        &self.response
    }

    pub const fn outcome(&self) -> &UnsubscribeOutcome {
        &self.outcome
    }

    pub fn into_parts(self) -> (ConversationSubscriptionStopped, UnsubscribeOutcome) {
        (self.response, self.outcome)
    }
}

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
