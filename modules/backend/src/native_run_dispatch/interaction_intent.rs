//! Exact stored-intent fingerprints for one delivered interaction command.

#![forbid(unsafe_code)]

use artisan_domain::{RespondApproval, RespondQuestion};

use crate::run_interaction::OwnedInteractionCommand;

use super::turn::TurnConsumptionState;

/// Rebuilds the exact intent fingerprint for one delivered envelope command.
///
/// Reads nothing but the envelope: the fingerprint must equal the one the
/// resolve transaction stored.
pub(super) fn command_request_intent(
    state: &TurnConsumptionState<'_>,
    command: &OwnedInteractionCommand,
) -> Option<String> {
    match command {
        OwnedInteractionCommand::RespondApproval {
            request_id,
            approval_id,
            approved,
        } => Some(
            RespondApproval::new(
                request_id.clone(),
                state.scope.launched.thread_id.clone(),
                state.scope.launched.run_id.clone(),
                approval_id.clone(),
                *approved,
            )
            .intent_key(),
        ),
        OwnedInteractionCommand::RespondQuestion {
            request_id,
            question_id,
            answers,
        } => RespondQuestion::new(
            request_id.clone(),
            state.scope.launched.thread_id.clone(),
            state.scope.launched.run_id.clone(),
            question_id.clone(),
            answers.clone(),
        )
        .ok()
        .map(|question| question.intent_key()),
        OwnedInteractionCommand::Steer { message_id, .. } => {
            // Stable per message: redelivery under the same request id
            // reproduces this fingerprint, so ledger dedup holds without
            // ever consulting provider state.
            Some(format!("steer:{}", message_id.as_str()))
        }
    }
}
