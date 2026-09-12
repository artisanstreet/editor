//! Steered follow-up projection onto a live turn.
//!
//! Owns [`ProjectSteeredMessage`] and the `project_steered_message`
//! transaction: one completed user item and its patch linked to the existing
//! turn, with the dispatch completed under the same fence.

use artisan_domain::{
    ConversationCursor, ItemId, MessageId, PatchId, ThreadId, TurnId, UnixMillis,
};
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, ConnectionTrait, DbBackend, EntityTrait,
    QueryFilter, Statement,
};

use crate::entities;
use crate::entities::{ConversationItemKind, ConversationPatchKind, EntityLifecycle, OrdinalKind};

use crate::repository::message_dispatch::{
    COMPLETE_STEERED_SQL, SteeredRowTerminal, classify_steered_transition_miss,
};
use crate::repository::{Repository, RepositoryError, corrupt_data, database_error, millis};

use super::{
    InitialPatchProjection, LaunchedProjections, RunLaunchError, counter_overflow,
    insert_initial_patch, insert_item_projection, insert_ordinal, obtain_conversation_state,
};

/// Durable input projecting one steered follow-up as a user item.
///
/// Unlike [`LaunchClaimedRun`], no turn, run, or patch identities are
/// minted here: the live turn already owns them. The caller mints only
/// the item and its patch; the live turn id is carried for linkage.
pub struct ProjectSteeredMessage<'a> {
    /// Persisted steered message identity (echo idempotency key).
    pub message_id: &'a MessageId,
    /// Thread owning the message and the live turn.
    pub thread_id: &'a ThreadId,
    /// LIVE turn identity; no new turn is created.
    pub turn_id: &'a TurnId,
    /// Caller-minted identity of the user item to project.
    pub item_id: &'a ItemId,
    /// Caller-minted identity of the item-upsert patch.
    pub patch_id: &'a PatchId,
    /// Validated follow-up body (payload text, or empty for image-only).
    pub body: &'a str,
    /// Caller-owned monotonic operation time stamped on every effect.
    pub operated_at: UnixMillis,
}

/// Durable receipt of one steered projection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SteeredMessageReceipt {
    /// Accepted message the projection originates from.
    pub message_id: MessageId,
    /// Thread owning the projection.
    pub thread_id: ThreadId,
    /// Live turn the projection was linked to.
    pub turn_id: TurnId,
    /// Projected user item identity.
    pub item_id: ItemId,
    /// Patch cursor produced by the projection.
    pub resulting_cursor: ConversationCursor,
}

/// Typed outcome of one steered projection call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProjectSteeredMessageOutcome {
    /// This transaction projected the item and completed the dispatch.
    Projected(SteeredMessageReceipt),
    /// An earlier identical transaction already projected this message;
    /// durable receipt information only, never provider authority.
    AlreadyProjected(SteeredMessageReceipt),
}

impl Repository {
    /// Projects one steered follow-up as a completed user item and
    /// completes its dispatch row in one transaction.
    ///
    /// The live turn is never touched: linkage only. Redelivery with the
    /// same message answers [`ProjectSteeredMessageOutcome::AlreadyProjected`]
    /// from the persisted item without writing anything. Every other
    /// divergence is typed without writing anything.
    ///
    /// # Errors
    ///
    /// Returns [`RunLaunchError::Repository`] for missing message rows,
    /// thread mismatches, non-open dispatches, counter overflows, and
    /// corrupt data, and [`RunLaunchError::IdentityConflict`] for
    /// colliding durable identities.
    #[expect(
        clippy::too_many_lines,
        reason = "the method keeps the steered projection's fence, insert, and dispatch \
                  completion inside one transaction narrative; extraction would split its locals"
    )]
    pub async fn project_steered_message(
        &self,
        command: ProjectSteeredMessage<'_>,
    ) -> Result<ProjectSteeredMessageOutcome, RunLaunchError> {
        let operated_at_ms = millis(command.operated_at);
        let transaction = self.begin_write().await.map_err(|source| {
            RunLaunchError::Repository(database_error("begin steered projection", source))
        })?;
        let message = entities::message::Entity::find_by_id(command.message_id.as_str())
            .one(&transaction)
            .await
            .map_err(|source| {
                RunLaunchError::Repository(database_error("load steered message", source))
            })?
            .ok_or(RunLaunchError::Repository(RepositoryError::Invariant {
                reason: "steered projection references a missing message",
            }))?;
        if message.thread_id != command.thread_id.as_str() {
            return Err(RunLaunchError::Repository(RepositoryError::Invariant {
                reason: "steered projection thread does not own its message",
            }));
        }
        if let Some(existing) = entities::conversation_item::Entity::find()
            .filter(entities::conversation_item::Column::ThreadId.eq(command.thread_id.as_str()))
            .filter(
                entities::conversation_item::Column::SourceMessageId
                    .eq(command.message_id.as_str()),
            )
            .one(&transaction)
            .await
            .map_err(|source| {
                RunLaunchError::Repository(database_error("find steered user item", source))
            })?
        {
            let state =
                entities::conversation_state::Entity::find_by_id(command.thread_id.as_str())
                    .one(&transaction)
                    .await
                    .map_err(|source| {
                        RunLaunchError::Repository(database_error(
                            "read steered cursor state",
                            source,
                        ))
                    })?
                    .ok_or(RunLaunchError::Repository(RepositoryError::Invariant {
                        reason: "projected steered item references missing conversation state",
                    }))?;
            let cursor_value = u64::try_from(state.last_patch_sequence)
                .map_err(|_| counter_overflow("patch sequence", state.last_patch_sequence))?;
            let receipt = SteeredMessageReceipt {
                message_id: command.message_id.clone(),
                thread_id: command.thread_id.clone(),
                turn_id: TurnId::parse(existing.turn_id).map_err(|error| {
                    RunLaunchError::Repository(corrupt_data("conversation_items", "turn_id", error))
                })?,
                item_id: ItemId::parse(existing.item_id).map_err(|error| {
                    RunLaunchError::Repository(corrupt_data("conversation_items", "item_id", error))
                })?,
                resulting_cursor: ConversationCursor::new(cursor_value),
            };
            transaction.commit().await.map_err(|source| {
                RunLaunchError::Repository(database_error("commit steered replay read", source))
            })?;
            return Ok(ProjectSteeredMessageOutcome::AlreadyProjected(receipt));
        }
        if operated_at_ms < message.accepted_at_ms {
            return Err(RunLaunchError::Repository(
                RepositoryError::InvalidChronology {
                    earlier_field: "messages.accepted_at_ms",
                    later_field: "steer operated_at",
                },
            ));
        }
        let (next_renderer_ordinal, last_patch_sequence) =
            obtain_conversation_state(&transaction, command.thread_id.as_str(), operated_at_ms)
                .await?;
        let item_ordinal = next_renderer_ordinal;
        let final_renderer_ordinal = item_ordinal
            .checked_add(1)
            .ok_or_else(|| counter_overflow("renderer ordinal", item_ordinal))?;
        let patch_sequence = last_patch_sequence
            .checked_add(1)
            .ok_or_else(|| counter_overflow("patch sequence", last_patch_sequence))?;
        let projections = LaunchedProjections {
            thread_id: command.thread_id.as_str(),
            turn_id: command.turn_id.as_str(),
            item_id: command.item_id.as_str(),
            message_id: command.message_id.as_str(),
            body: command.body,
            turn_ordinal: item_ordinal,
            item_ordinal,
        };
        insert_ordinal(
            &transaction,
            command.thread_id.as_str(),
            item_ordinal,
            OrdinalKind::Item,
            command.item_id.as_str(),
        )
        .await?;
        insert_item_projection(&transaction, &projections, operated_at_ms).await?;
        insert_initial_patch(
            &transaction,
            command.thread_id.as_str(),
            patch_sequence,
            ConversationPatchKind::ItemUpsert,
            operated_at_ms,
            InitialPatchProjection {
                patch_id: command.patch_id,
                turn_id: command.turn_id.as_str(),
                item_id: Some(command.item_id.as_str()),
                ordinal: item_ordinal,
                lifecycle: EntityLifecycle::Completed,
                item_kind: Some(ConversationItemKind::UserMessage),
                body: Some(command.body.to_owned()),
            },
        )
        .await?;
        // Complete under the same fence as the standalone path: the row
        // must still be open here (same transaction, no interleaving), so
        // any miss is a genuine terminal race mapped distinctly — a
        // completed row without its item is invariant-violating, and a
        // failed row is never reported as completed.
        let completed = transaction
            .execute_raw(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                COMPLETE_STEERED_SQL,
                [operated_at_ms.into(), command.message_id.as_str().into()],
            ))
            .await
            .map_err(|source| {
                RunLaunchError::Repository(database_error("complete steered dispatch", source))
            })?
            .rows_affected();
        if completed != 1 {
            match classify_steered_transition_miss(&transaction, command.message_id).await {
                Ok(SteeredRowTerminal::Completed(_)) => {
                    return Err(RunLaunchError::Repository(RepositoryError::Invariant {
                        reason: "steered dispatch completed without its projected item",
                    }));
                }
                Ok(SteeredRowTerminal::Failed(_)) => {
                    return Err(RunLaunchError::Repository(
                        RepositoryError::InvalidDispatchState {
                            message_id: command.message_id.clone(),
                            state: "failed",
                        },
                    ));
                }
                Err(error) => return Err(RunLaunchError::Repository(error)),
            }
        }
        entities::conversation_state::ActiveModel {
            thread_id: Set(command.thread_id.as_str().to_owned()),
            next_renderer_ordinal: Set(final_renderer_ordinal),
            last_patch_sequence: Set(patch_sequence),
            updated_at_ms: Set(operated_at_ms),
        }
        .update(&transaction)
        .await
        .map_err(|source| {
            RunLaunchError::Repository(database_error("advance conversation counters", source))
        })?;
        let cursor_value = u64::try_from(patch_sequence)
            .map_err(|_| counter_overflow("patch sequence", patch_sequence))?;
        transaction.commit().await.map_err(|source| {
            RunLaunchError::Repository(database_error("commit steered projection", source))
        })?;
        Ok(ProjectSteeredMessageOutcome::Projected(
            SteeredMessageReceipt {
                message_id: command.message_id.clone(),
                thread_id: command.thread_id.clone(),
                turn_id: command.turn_id.clone(),
                item_id: command.item_id.clone(),
                resulting_cursor: ConversationCursor::new(cursor_value),
            },
        ))
    }
}
