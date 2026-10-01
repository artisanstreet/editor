//! Assistant text commit seam for one live turn.
//!
//! Owns every durable assistant-body mutation outside terminal settlement:
//! opening the item, full-body replacement, and the coalesced append flush.
//! Every failure marks the turn's progress uncertain exactly like the former
//! per-delta append path did, so an incomplete commit is never presented as
//! durably completed. The coalesced flush carries a bounded run of buffered
//! deltas in one batch, which also collapses their subscriber wakes into the
//! single wake published by the shared commit path.

#![forbid(unsafe_code)]

use artisan_database::AssistantChange;
use artisan_domain::{AssistantBody, IncrementalText, Revision};

use crate::engine_owner::operation::AcceptedTurn;

use super::diagnostics::StepError;
use super::dispatch_support::{at_or_after, mint_item_id, mint_patch_id};
use super::turn::{TurnConsumptionContext, TurnConsumptionState, mark_interrupted};
use super::{CommitBatchRequest, commit_batch_with_retry};

/// Commits the buffered delta run as one append batch, if anything is
/// buffered.
///
/// The buffer is left intact when a fallible step fails so a later flush or
/// the terminal settlement can still surface the assembled body; on success
/// the durable revision and batch sequence advance exactly once, matching one
/// uncoalesced append.
pub(super) async fn flush_pending_deltas(
    context: &TurnConsumptionContext<'_>,
    state: &mut TurnConsumptionState<'_>,
    turn: &mut AcceptedTurn,
) -> bool {
    if state.coalescer.is_empty() {
        return true;
    }
    let Some(item_id) = state.assistant_item.clone() else {
        // The first durable body commits on arrival, so buffered bytes can
        // only precede the item in an impossible state; open the item with
        // the assembled body rather than dropping the tail. The start clears
        // the buffer on success, so its emptiness reports the outcome.
        start_assistant_item(context, state, turn).await;
        return state.coalescer.is_empty();
    };
    let fragment = match IncrementalText::parse(state.coalescer.pending().to_owned()) {
        Ok(fragment) => fragment,
        Err(error) => {
            mark_interrupted(
                state,
                turn,
                false,
                &StepError::failed("validating the buffered text append", error),
            );
            return false;
        }
    };
    let Some(patch_id) = mint_patch_id(context.origin) else {
        mark_interrupted(
            state,
            turn,
            false,
            &StepError::refused(
                "minting the patch id for the buffered text append: entropy or identifier validation failed",
            ),
        );
        return false;
    };
    let changes = [AssistantChange::Append {
        item_id: &item_id,
        expected_revision: state.assistant_revision,
        text: &fragment,
        patch_id: &patch_id,
    }];
    let Some(operated_at) = at_or_after(context.origin, state.scope.expected_updated_at) else {
        mark_interrupted(
            state,
            turn,
            false,
            &StepError::refused("reading the clock for the buffered text append"),
        );
        return false;
    };
    if let Err(error) = commit_batch_with_retry(CommitBatchRequest {
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
    {
        mark_interrupted(
            state,
            turn,
            true,
            &StepError::failed("committing the buffered text append", error),
        );
        return false;
    }
    // The bytes are durable: clear the buffer before any later fallible
    // bookkeeping so a second flush can never replay them.
    state.coalescer.clear();
    let next_revision = match state.assistant_revision.checked_next() {
        Ok(next_revision) => next_revision,
        Err(error) => {
            mark_interrupted(
                state,
                turn,
                true,
                &StepError::failed(
                    "advancing the assistant revision after the text append",
                    error,
                ),
            );
            return false;
        }
    };
    state.assistant_revision = next_revision;
    state.scope.expected_updated_at = operated_at;
    let Some(next_sequence) = state.batch_sequence.checked_add(1) else {
        mark_interrupted(
            state,
            turn,
            true,
            &StepError::refused(
                "advancing the batch sequence after the text append: the counter overflowed",
            ),
        );
        return false;
    };
    state.batch_sequence = next_sequence;
    true
}

/// Persists the full in-memory body as one replacement batch.
///
/// Used when a delta extends the assembled body non-contiguously (a new
/// provider part earns a separator) and when a snapshot rewrites it. Any
/// buffered fragments are part of the persisted body and are cleared with it.
#[expect(
    clippy::too_many_lines,
    reason = "linear sequence of fallible commit steps, each reporting its own failure"
)]
pub(super) async fn replace_assistant_body(
    context: &TurnConsumptionContext<'_>,
    state: &mut TurnConsumptionState<'_>,
    turn: &mut AcceptedTurn,
) {
    let body = match AssistantBody::parse(state.assistant_body.clone()) {
        Ok(body) => body,
        Err(error) => {
            mark_interrupted(
                state,
                turn,
                true,
                &StepError::failed("validating the replacement assistant body", error),
            );
            return;
        }
    };
    let Some(item_id) = state.assistant_item.clone() else {
        mark_interrupted(
            state,
            turn,
            true,
            &StepError::refused(
                "replacing the assistant body: the turn has no open assistant item",
            ),
        );
        return;
    };
    let next_revision = match state.assistant_revision.checked_next() {
        Ok(next_revision) => next_revision,
        Err(error) => {
            mark_interrupted(
                state,
                turn,
                true,
                &StepError::failed(
                    "advancing the assistant revision for the body replacement",
                    error,
                ),
            );
            return;
        }
    };
    let Some(patch_id) = mint_patch_id(context.origin) else {
        mark_interrupted(
            state,
            turn,
            false,
            &StepError::refused(
                "minting the patch id for the body replacement: entropy or identifier validation failed",
            ),
        );
        return;
    };
    let changes = [AssistantChange::Replace {
        item_id: &item_id,
        expected_revision: state.assistant_revision,
        body: &body,
        phase: state.assistant_phase,
        patch_id: &patch_id,
    }];
    let Some(operated_at) = at_or_after(context.origin, state.scope.expected_updated_at) else {
        mark_interrupted(
            state,
            turn,
            false,
            &StepError::refused("reading the clock for the body replacement"),
        );
        return;
    };
    if let Err(error) = commit_batch_with_retry(CommitBatchRequest {
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
    {
        mark_interrupted(
            state,
            turn,
            true,
            &StepError::failed("committing the body replacement", error),
        );
        return;
    }
    // The whole body is durable: clear the buffer before any later fallible
    // bookkeeping so a second flush can never replay it.
    state.coalescer.clear();
    state.assistant_revision = next_revision;
    state.scope.expected_updated_at = operated_at;
    let Some(next_sequence) = state.batch_sequence.checked_add(1) else {
        mark_interrupted(
            state,
            turn,
            true,
            &StepError::refused(
                "advancing the batch sequence after the body replacement: the counter overflowed",
            ),
        );
        return;
    };
    state.batch_sequence = next_sequence;
}

/// Opens the assistant item with the current in-memory body.
///
/// Called eagerly on the first delta so a held stream's first durable body
/// arrives without waiting for a coalescing threshold, and from snapshot or
/// resolution paths when no item exists yet.
pub(super) async fn start_assistant_item(
    context: &TurnConsumptionContext<'_>,
    state: &mut TurnConsumptionState<'_>,
    turn: &mut AcceptedTurn,
) {
    let opening = open_item_inputs(context, &state.assistant_body);
    let (item_id, body, patch_id, activation_patch_id) = match opening {
        Ok(opening) => opening,
        Err(error) => {
            mark_interrupted(state, turn, false, &error);
            return;
        }
    };
    let changes = [AssistantChange::Start {
        item_id: &item_id,
        phase: state.assistant_phase,
        body: &body,
        patch_id: &patch_id,
    }];
    let Some(operated_at) = at_or_after(context.origin, state.scope.expected_updated_at) else {
        mark_interrupted(
            state,
            turn,
            false,
            &StepError::refused("reading the clock to open the assistant item"),
        );
        return;
    };
    if let Err(error) = commit_batch_with_retry(CommitBatchRequest {
        repository: context.repository,
        notifier: &context.config.notifier,
        scope: &state.scope,
        batch_sequence: state.batch_sequence,
        operated_at,
        activate_turn_patch_id: state
            .parked_parts
            .is_empty()
            .then_some(&activation_patch_id),
        changes: &changes,
        checkpoint: artisan_database::CheckpointUpdate::Keep,
        retries: context.config.max_command_retries,
    })
    .await
    {
        mark_interrupted(
            state,
            turn,
            true,
            &StepError::failed("committing the opened assistant item", error),
        );
        return;
    }
    state.coalescer.clear();
    state.assistant_item = Some(item_id);
    state.assistant_revision = Revision::new(0);
    state.scope.expected_updated_at = operated_at;
    let Some(next_sequence) = state.batch_sequence.checked_add(1) else {
        mark_interrupted(
            state,
            turn,
            true,
            &StepError::refused(
                "advancing the batch sequence after opening the assistant item: the counter overflowed",
            ),
        );
        return;
    };
    state.batch_sequence = next_sequence;
}

/// Mints the identities and validates the body one assistant item opens with:
/// the item id, its body, the item patch id, and the turn activation patch id.
fn open_item_inputs(
    context: &TurnConsumptionContext<'_>,
    assistant_body: &str,
) -> Result<
    (
        artisan_domain::ItemId,
        AssistantBody,
        artisan_domain::PatchId,
        artisan_domain::PatchId,
    ),
    StepError,
> {
    let item_id = mint_item_id(context.origin).ok_or(StepError::refused(
        "minting the assistant item id: entropy or identifier validation failed",
    ))?;
    let body = AssistantBody::parse(assistant_body.to_owned())
        .map_err(|error| StepError::failed("validating the opening assistant body", error))?;
    let patch_id = mint_patch_id(context.origin).ok_or(StepError::refused(
        "minting the item patch id: entropy or identifier validation failed",
    ))?;
    let activation_patch_id = mint_patch_id(context.origin).ok_or(StepError::refused(
        "minting the turn activation patch id: entropy or identifier validation failed",
    ))?;
    Ok((item_id, body, patch_id, activation_patch_id))
}

/// Opens the assistant item if it is still missing; reports whether the item
/// exists afterwards.
///
/// Used by terminal settlement so a run that carried text always has an item
/// to settle.
///
/// # Errors
///
/// Returns the [`StepError`] naming the step that kept the item from opening.
pub(super) async fn ensure_assistant_item(
    context: &TurnConsumptionContext<'_>,
    state: &mut TurnConsumptionState<'_>,
) -> Result<(), StepError> {
    if state.assistant_item.is_some() {
        return Ok(());
    }
    let (item_id, body, patch_id, activation_patch_id) =
        open_item_inputs(context, &state.assistant_body)?;
    let changes = [AssistantChange::Start {
        item_id: &item_id,
        phase: state.assistant_phase,
        body: &body,
        patch_id: &patch_id,
    }];
    let operated_at = at_or_after(context.origin, state.scope.expected_updated_at).ok_or(
        StepError::refused("reading the clock to open the assistant item at settlement"),
    )?;
    commit_batch_with_retry(CommitBatchRequest {
        repository: context.repository,
        notifier: &context.config.notifier,
        scope: &state.scope,
        batch_sequence: state.batch_sequence,
        operated_at,
        activate_turn_patch_id: Some(&activation_patch_id),
        changes: &changes,
        checkpoint: artisan_database::CheckpointUpdate::Keep,
        retries: context.config.max_command_retries,
    })
    .await
    .map_err(|error| {
        StepError::failed("committing the assistant item opened at settlement", error)
    })?;
    state.coalescer.clear();
    state.assistant_item = Some(item_id);
    state.assistant_revision = Revision::new(0);
    state.scope.expected_updated_at = operated_at;
    Ok(())
}
