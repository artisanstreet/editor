//! Durable message identities survive provider part switches and corrections.

use super::assistant_commit::flush_pending_deltas;
use super::dispatch_support::{at_or_after, mint_patch_id};
use super::turn::{TurnConsumptionContext, TurnConsumptionState, mark_interrupted};
use super::{CommitBatchRequest, commit_batch_with_retry};
use crate::engine_owner::operation::AcceptedTurn;
use artisan_database::AssistantChange;
use artisan_domain::{AssistantMessagePhase, ItemId, Revision};

pub(super) struct MessageCursor {
    item: Option<ItemId>,
    revision: Revision,
    body: String,
    phase: AssistantMessagePhase,
}

/// Switches the durable target, flushing the previous target before changing it.
/// Late snapshots select their existing identity rather than creating duplicates.
pub(super) async fn select_part(
    context: &TurnConsumptionContext<'_>,
    state: &mut TurnConsumptionState<'_>,
    turn: &mut AcceptedTurn,
    part: &str,
) -> bool {
    if state.active_part.as_deref() == Some(part) {
        return true;
    }
    if !flush_pending_deltas(context, state, turn).await {
        return false;
    }
    if let Some(previous) = state.active_part.take() {
        state.parked_parts.insert(
            previous,
            MessageCursor {
                item: state.assistant_item.take(),
                revision: state.assistant_revision,
                body: std::mem::take(&mut state.assistant_body),
                phase: state.assistant_phase,
            },
        );
        state.assistant_revision = Revision::new(0);
        state.assistant_phase = AssistantMessagePhase::Unspecified;
    }
    if let Some(cursor) = state.parked_parts.remove(part) {
        state.assistant_item = cursor.item;
        state.assistant_revision = cursor.revision;
        state.assistant_body = cursor.body;
        state.assistant_phase = cursor.phase;
    } else {
        state.last_part = Some(part.to_owned());
    }
    state.active_part = Some(part.to_owned());
    true
}

/// Seals earlier messages before the owning run's terminal transaction.
/// The last-created message stays selected even if an older part was corrected.
pub(super) async fn finish_history(
    context: &TurnConsumptionContext<'_>,
    state: &mut TurnConsumptionState<'_>,
    turn: &mut AcceptedTurn,
) -> bool {
    if let Some(last) = state.last_part.clone()
        && !select_part(context, state, turn, &last).await
    {
        return false;
    }
    let targets: Vec<_> = state
        .parked_parts
        .values()
        .filter_map(|cursor| cursor.item.clone().map(|item| (item, cursor.revision)))
        .collect();
    for (item_id, revision) in targets {
        let Some(patch_id) = mint_patch_id(context.origin) else {
            mark_interrupted(state, turn, true);
            return false;
        };
        let Some(operated_at) = at_or_after(context.origin, state.scope.expected_updated_at) else {
            mark_interrupted(state, turn, true);
            return false;
        };
        let changes = [AssistantChange::Finish {
            item_id: &item_id,
            expected_revision: revision,
            patch_id: &patch_id,
        }];
        if commit_batch_with_retry(CommitBatchRequest {
            repository: context.repository,
            notifier: &context.config.notifier,
            scope: &state.scope,
            batch_sequence: state.batch_sequence,
            operated_at,
            activate_turn_patch_id: None,
            changes: &changes,
            checkpoint: artisan_database::CheckpointUpdate::Keep,
            retries: context.config.max_command_retries,
        })
        .await
        .is_err()
        {
            mark_interrupted(state, turn, true);
            return false;
        }
        state.scope.expected_updated_at = operated_at;
        let Some(next) = state.batch_sequence.checked_add(1) else {
            mark_interrupted(state, turn, true);
            return false;
        };
        state.batch_sequence = next;
    }
    true
}
