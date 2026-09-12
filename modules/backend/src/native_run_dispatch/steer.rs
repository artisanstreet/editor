//! Steer command handling for one live turn.
//!
//! Handles a routed steer envelope end to end: fence the target run, recheck
//! the captured engine, preflight the turn ledger, drive the provider ack
//! while draining observations inline, then record the resolution and project
//! the follow-up text. A steer is never silently completed: cancellation or
//! turn end answers transiently and leaves the row open for redelivery.

use artisan_database::{ProjectSteeredMessage, ProjectSteeredMessageOutcome, RunLaunchError};
use artisan_domain::{EngineId, MessageId, ObservationId, RequestId, RunId, ThreadId};

use crate::{
    CommandOrigin,
    engine_owner::interaction::{InteractionTarget, TurnInteractionOutcome},
    engine_owner::operation::{AcceptedTurn, SteerError},
    run_interaction::RunInteractionAck,
};

use super::dispatch_support::{mint_item_id, mint_patch_id};
use super::turn::{TurnConsumptionContext, TurnConsumptionState, handle_observation};

/// Outcome of driving one steered provider-ack future.
enum SteerDriveOutcome<E> {
    /// The provider ack resolved (success or typed rejection).
    Acked(Result<(), E>),
    /// A cancellation handle fired while waiting.
    Cancelled,
    /// The turn ended (or terminally settled) while waiting.
    TurnEnded,
}

/// Drives one owned provider-ack future while draining observations inline.
///
/// `pending` must own everything it needs (cloned sender, owned
/// strings): it retains no turn borrow, so the drain below compiles and
/// later steers keep their sender. Observations are processed inline in
/// arrival order through the existing handler — never buffered, never
/// reordered — so a bounded observation channel cannot deadlock the pump
/// mid-steer: a full channel stalls its sends, stalled sends stall the
/// steer read, and without this drain the ack would never come. Pinned
/// once outside the loop: polling by value across iterations would move
/// it after the first poll.
async fn drive_steer_ack<F, E>(
    context: &TurnConsumptionContext<'_>,
    state: &mut TurnConsumptionState<'_>,
    turn: &mut AcceptedTurn,
    pending: F,
) -> SteerDriveOutcome<E>
where
    F: std::future::Future<Output = Result<(), E>>,
{
    tokio::pin!(pending);
    loop {
        tokio::select! {
            biased;
            () = context.stop.wait() => break SteerDriveOutcome::Cancelled,
            () = context.process_cancel.wait() => break SteerDriveOutcome::Cancelled,
            () = context.run_cancel.wait() => break SteerDriveOutcome::Cancelled,
            result = &mut pending => break SteerDriveOutcome::Acked(result),
            observation = turn.next_observation() => {
                match observation {
                    Some(observation) => {
                        handle_observation(context, state, turn, observation).await;
                        if state.terminal.is_some() {
                            break SteerDriveOutcome::TurnEnded;
                        }
                    }
                    None => break SteerDriveOutcome::TurnEnded,
                }
            }
        }
    }
}

/// Reproduces the stored typed refusal for one failed steered dispatch.
///
/// Maps the persisted `last_error` back to its bounded refusal literal so
/// a redelivery reproduces the original typed refusal without provider
/// contact and without re-failing the row. Unknown spellings fall back to
/// a generic static; the payload stays preserved for user recovery in
/// every case.
pub(crate) fn steer_stored_refusal(reason: Option<&str>) -> RunInteractionAck {
    RunInteractionAck::Refused {
        reason: match reason {
            Some("steer message identity invalid") => "steer message identity invalid",
            Some("steer target engine changed") => "steer target engine changed",
            Some("steer is not supported on this engine path") => {
                "steer is not supported on this engine path"
            }
            Some("steer target already resolved") => "steer target already resolved",
            Some("steered projection failed") => "steered projection failed",
            Some("steer write to the provider failed") => "steer write to the provider failed",
            Some("steer target run is no longer live") => "steer target run is no longer live",
            Some("steer does not support image attachments") => {
                "steer does not support image attachments"
            }
            _ => "steered send failed",
        },
    }
}

/// Projects one provider-acked steer without provider contact.
///
/// Shared by the fresh-ack path and the known-acked duplicate path. The
/// ledger `Duplicate` now proves the actual ack already happened
/// (resolutions are recorded only post-ack), so an open durable row means
/// only the projection is missing — e.g. a transient database failure
/// that answered `Unavailable` — and retrying the idempotent projection
/// eventually completes without ever resending to the provider. Completed
/// rows never reach here (they answer `Steered` from durable state) and
/// failed rows never reach here (they reproduce their stored refusal).
/// A transient mint/clock/database miss answers `Unavailable` with the
/// row left open; any other projection miss fails the row typed with the
/// payload preserved.
async fn project_known_acked_steer(
    context: &TurnConsumptionContext<'_>,
    state: &mut TurnConsumptionState<'_>,
    thread_id: &ThreadId,
    message_id: &MessageId,
    text: &str,
    respond: tokio::sync::oneshot::Sender<RunInteractionAck>,
) {
    let Some(item_id) = mint_item_id(context.origin) else {
        let _ = respond.send(RunInteractionAck::Unavailable);
        return;
    };
    let Some(patch_id) = mint_patch_id(context.origin) else {
        let _ = respond.send(RunInteractionAck::Unavailable);
        return;
    };
    let Ok(operated_at) = context.origin.acceptance_instant() else {
        let _ = respond.send(RunInteractionAck::Unavailable);
        return;
    };
    match context
        .repository
        .project_steered_message(ProjectSteeredMessage {
            message_id,
            thread_id,
            turn_id: &state.scope.launched.turn_id,
            item_id: &item_id,
            patch_id: &patch_id,
            body: text,
            operated_at,
        })
        .await
    {
        Ok(
            ProjectSteeredMessageOutcome::Projected(_)
            | ProjectSteeredMessageOutcome::AlreadyProjected(_),
        ) => {
            let _ = context.config.notifier.publish(thread_id);
            let _ = respond.send(RunInteractionAck::Steered);
        }
        Err(RunLaunchError::Repository(artisan_database::RepositoryError::Database { .. })) => {
            let _ = respond.send(RunInteractionAck::Unavailable);
        }
        Err(_) => {
            fail_steered_row(context, message_id, "steered projection failed").await;
            let _ = respond.send(RunInteractionAck::Refused {
                reason: "steered projection failed",
            });
        }
    }
}

/// Applies one named steer to the owning live turn.
///
/// Flow: fence thread/run (same as every arm) → recheck the captured
/// send-time engine against the live turn's engine → nonmutating ledger
/// preflight (eligible heads to the provider; duplicates consult durable
/// delivery state instead of touching the provider) → drive the owned
/// provider-ack future while draining observations inline (the bounded
/// observation channel would otherwise deadlock the pump: full channel
/// stalls its sends, stalled sends stall the steer read, and the ack
/// never comes) → on ack, record the ledger resolution and then project
/// the follow-up text keyed by the steered message id, completing the
/// row atomically, or fail the row typed with the payload preserved.
///
/// The resolution is recorded ONLY after the actual ack and before
/// projection: a projection retry then observes `Duplicate` (known acked)
/// while a pre-ack interruption never wrote anything, so the row stays
/// open for the orphan sweep or a redelivery. Cancellation or turn end
/// breaks the wait into a transient acknowledgement for the same reason:
/// never silently completed, never fresh-run.
#[allow(clippy::too_many_arguments)]
#[expect(
    clippy::too_many_lines,
    reason = "steer handling is one guarded sequence; each guard answers the caller directly"
)]
pub(super) async fn handle_steer(
    context: &TurnConsumptionContext<'_>,
    state: &mut TurnConsumptionState<'_>,
    turn: &mut AcceptedTurn,
    thread_id: ThreadId,
    run_id: RunId,
    request_id: RequestId,
    message_id: MessageId,
    text: String,
    respond: tokio::sync::oneshot::Sender<RunInteractionAck>,
) {
    if thread_id != state.scope.launched.thread_id || run_id != state.scope.launched.run_id {
        let _ = respond.send(RunInteractionAck::WrongRun);
        return;
    }
    if context.origin.acceptance_instant().is_err() {
        let _ = respond.send(RunInteractionAck::Unavailable);
        return;
    }
    let Ok(target_id) = ObservationId::parse(message_id.as_str()) else {
        fail_steered_row(context, &message_id, "steer message identity invalid").await;
        let _ = respond.send(RunInteractionAck::Refused {
            reason: "steer message identity invalid",
        });
        return;
    };
    // Same-engine recheck against the ACCEPTED (captured) settings, not
    // current thread settings: a selection change after send must fail
    // this steer, never move it to another engine.
    let captured = match context
        .repository
        .read_receipt_engine_settings(&request_id)
        .await
    {
        Ok(Some(settings)) => Some(settings),
        Ok(None) => None,
        Err(_) => {
            let _ = respond.send(RunInteractionAck::Unavailable);
            return;
        }
    };
    let captured_engine = match captured.as_ref() {
        Some(settings) => Some(settings.config().selection().engine_id()),
        None => context
            .repository
            .read_thread_engine_settings(&thread_id)
            .await
            .ok()
            .flatten()
            .map(|settings| settings.config().selection().engine_id()),
    };
    if captured_engine != Some(state.engine) {
        fail_steered_row(context, &message_id, "steer target engine changed").await;
        let _ = respond.send(RunInteractionAck::Refused {
            reason: "steer target engine changed",
        });
        return;
    }
    // Engine pre-filter: only turns whose pumps can write a steer verb
    // proceed to the provider call. Cursor/Grok/OpenCode2 have no verb
    // wired, so they fail here — deterministically, without provider
    // contact and without prompt-state plumbing. Waiting-session
    // follow-up delivery is a named follow-up packet, not this one.
    if !matches!(state.engine, EngineId::Codex | EngineId::Claude) {
        fail_steered_row(
            context,
            &message_id,
            "steer is not supported on this engine path",
        )
        .await;
        let _ = respond.send(RunInteractionAck::Refused {
            reason: "steer is not supported on this engine path",
        });
        return;
    }
    let intent = format!("steer:{}", message_id.as_str());
    turn.note_interaction_requested(&target_id, InteractionTarget::Steer);
    // Nonmutating preflight shares deliver validation: eligible heads to
    // the provider below; duplicates consult durable delivery state (the
    // actual ack state, never pre-ack intent) without provider contact;
    // conflicts and resolved targets fail typed. Nothing is recorded yet.
    match turn.preflight_interaction_response(
        request_id.as_str(),
        &target_id,
        InteractionTarget::Steer,
        &intent,
    ) {
        Ok(TurnInteractionOutcome::Applied) => {}
        Ok(TurnInteractionOutcome::Duplicate) => {
            // Ledger Duplicate now proves the actual ack already happened
            // (resolutions are recorded only post-ack): consult durable
            // state. Completed replays success; Failed reproduces the
            // stored typed refusal; an open row means only the projection
            // is missing, so retry the idempotent projection without
            // provider contact. Anything unreadable answers transiently.
            // Never a pre-ack false completion: an interruption before the
            // ack never wrote anything, so it preflights eligible, never
            // Duplicate.
            match context
                .repository
                .read_steered_dispatch_state(&message_id)
                .await
            {
                Ok((dispatch_state, reason, _)) => {
                    use artisan_database::entities::DispatchState as Row;
                    match dispatch_state {
                        Row::Completed => {
                            let _ = respond.send(RunInteractionAck::Steered);
                        }
                        Row::Failed => {
                            let _ = respond.send(steer_stored_refusal(reason.as_deref()));
                        }
                        Row::Queued | Row::Leased | Row::Running => {
                            project_known_acked_steer(
                                context,
                                state,
                                &thread_id,
                                &message_id,
                                text.as_str(),
                                respond,
                            )
                            .await;
                        }
                    }
                }
                Err(_) => {
                    let _ = respond.send(RunInteractionAck::Unavailable);
                }
            }
            return;
        }
        Err(_) => {
            fail_steered_row(context, &message_id, "steer target already resolved").await;
            let _ = respond.send(RunInteractionAck::Refused {
                reason: "steer target already resolved",
            });
            return;
        }
    }
    // Owned future: captures clones only, retains no turn borrow, so the
    // drain below compiles and later steers keep their sender. Pinned
    // once inside `drive_steer_ack`: polling by value across iterations
    // would move it after the first poll.
    let outcome = {
        let pending = turn.steer_text(request_id.as_str(), text.as_str());
        drive_steer_ack(context, state, turn, pending).await
    };
    match outcome {
        SteerDriveOutcome::Acked(Ok(())) => {
            // Record the resolution ONLY now — after the actual ack — and
            // before projection, so a projection retry observes Duplicate
            // (known acked) while a pre-ack interruption never wrote
            // anything. The serialized turn loop prevents competing steer
            // handling, so a miss here is a genuine terminal race.
            if turn
                .deliver_interaction_response(
                    request_id.as_str(),
                    &target_id,
                    InteractionTarget::Steer,
                    &intent,
                )
                .is_err()
            {
                fail_steered_row(context, &message_id, "steer target already resolved").await;
                let _ = respond.send(RunInteractionAck::Refused {
                    reason: "steer target already resolved",
                });
                return;
            }
            project_known_acked_steer(
                context,
                state,
                &thread_id,
                &message_id,
                text.as_str(),
                respond,
            )
            .await;
        }
        SteerDriveOutcome::Acked(Err(SteerError::Unsupported)) => {
            fail_steered_row(
                context,
                &message_id,
                "steer is not supported on this engine path",
            )
            .await;
            let _ = respond.send(RunInteractionAck::Refused {
                reason: "steer is not supported on this engine path",
            });
        }
        SteerDriveOutcome::Acked(Err(SteerError::DeliveryFailed)) => {
            fail_steered_row(context, &message_id, "steer write to the provider failed").await;
            let _ = respond.send(RunInteractionAck::Refused {
                reason: "steer write to the provider failed",
            });
        }
        SteerDriveOutcome::Cancelled | SteerDriveOutcome::TurnEnded => {
            // The run is dying or dead: leave the row open for the orphan
            // sweep or a redelivery, and answer transiently so the sender
            // retries against a live loop instead of recording a false
            // terminal refusal.
            let _ = respond.send(RunInteractionAck::Unavailable);
        }
    }
}

/// Fails one steered dispatch row with a bounded arm-mapped reason,
/// preserving the payload for user recovery.
async fn fail_steered_row(
    context: &TurnConsumptionContext<'_>,
    message_id: &MessageId,
    reason: &'static str,
) {
    let Ok(operated_at) = context.origin.acceptance_instant() else {
        return;
    };
    let _ = context
        .repository
        .fail_steered_dispatch(message_id, reason, operated_at)
        .await;
}
