//! Live-run interaction handling: approval and question routing into
//! owning run inboxes plus named-steer delivery and settlement.
//!
//! Entry methods are `pub(super)` for the root command dispatch; the
//! routing and receipt helpers stay private to this module. Every
//! failure maps through the shared protocol vocabulary of the parent.

use artisan_database::QueueMessageInput;
use artisan_domain::{InteractionOutcome, MessageId, QueueMessage, RequestId, ThreadId};
use artisan_protocol::{
    ErrorCode, ProtocolFailure, QueueMessageReceipt, RespondApprovalReceipt,
    RespondQuestionReceipt, ResponsePayload, RunInteractionOutcome, ServerResponse,
};

use crate::run_interaction::{
    OwnedInteractionCommand, RunInteractionAck, RunInteractionEnvelope, RunInteractionRegistryError,
};

use super::failures::{outcome, repository_failure, typed_failure};
use super::{
    RUN_INTERACTION_INBOX_BUSY_DETAIL, RUN_INTERACTION_UNAVAILABLE_DETAIL, RequestHandler,
    forged_identity_failure, origin_clock_failure, origin_entropy_failure,
};

impl RequestHandler {
    /// Answers one approval response by routing it into its owning live run.
    ///
    /// Receipt replay precedes routing: an exact retry answers `duplicate`
    /// without consulting the registry, and a reused request id with a
    /// different intent fails as an idempotency conflict. A live route miss
    /// answers `wrong_run` without storing anything, so the client may retry
    /// once the owning run is live. Only the owning loop's acknowledgement
    /// settles the receipt.
    pub(super) async fn respond_approval_outcome(
        &self,
        request_id: &RequestId,
        respond: &artisan_domain::RespondApproval,
    ) -> Result<ServerResponse, ProtocolFailure> {
        let command = OwnedInteractionCommand::RespondApproval {
            request_id: respond.request_id().clone(),
            approval_id: respond.approval_id().clone(),
            approved: respond.approved(),
        };
        let stored = self
            .route_interaction(
                request_id,
                respond.thread_id(),
                respond.run_id(),
                &command,
                &respond.intent_key(),
            )
            .await?;
        approval_response(request_id, &stored)
    }

    /// Answers one question response with the same routing contract as
    /// [`Self::respond_approval_outcome`].
    pub(super) async fn respond_question_outcome(
        &self,
        request_id: &RequestId,
        respond: &artisan_domain::RespondQuestion,
    ) -> Result<ServerResponse, ProtocolFailure> {
        let command = OwnedInteractionCommand::RespondQuestion {
            request_id: respond.request_id().clone(),
            question_id: respond.question_id().clone(),
            answers: respond.answers().clone(),
        };
        let stored = self
            .route_interaction(
                request_id,
                respond.thread_id(),
                respond.run_id(),
                &command,
                &respond.intent_key(),
            )
            .await?;
        Ok(question_response(request_id, &stored))
    }

    /// Routes one validated response into its owning live run and awaits the
    /// owning loop's acknowledgement.
    ///
    /// The acknowledgement carries the stored durable outcome, so this route
    /// performs no second resolution and claims no durable effect of its own.
    async fn route_interaction(
        &self,
        request_id: &RequestId,
        thread_id: &ThreadId,
        run_id: &artisan_domain::RunId,
        command: &OwnedInteractionCommand,
        intent_key: &str,
    ) -> Result<artisan_database::StoredInteractionReceipt, ProtocolFailure> {
        let Some(registry) = self.run_interaction.as_ref() else {
            return Err(typed_failure(
                ErrorCode::UnsupportedFeature,
                RUN_INTERACTION_UNAVAILABLE_DETAIL,
                false,
                request_id,
            ));
        };
        if let Some(stored) = self
            .repository
            .lookup_interaction_receipt(request_id)
            .await
            .map_err(|error| interaction_repository_failure(&error, request_id))?
        {
            if interaction_intent_matches(&stored, thread_id, run_id, command, intent_key) {
                let mut replay = stored;
                replay.disposition = artisan_domain::ReceiptDisposition::Duplicate;
                return Ok(replay);
            }
            return Err(typed_failure(
                ErrorCode::IdempotencyConflict,
                format!("request `{request_id}` was already accepted for a different response"),
                false,
                request_id,
            ));
        }
        let Some(inbox) = registry
            .route(thread_id, run_id)
            .map_err(|error| run_interaction_failure(error, request_id))?
        else {
            return unsubmitted_wrong_run(request_id, thread_id, run_id, command);
        };
        let (respond, acknowledged) = tokio::sync::oneshot::channel();
        let envelope = RunInteractionEnvelope {
            thread_id: thread_id.clone(),
            run_id: run_id.clone(),
            command: command.clone(),
            respond,
        };
        match inbox.try_send(envelope) {
            Ok(()) => {}
            Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                return Err(typed_failure(
                    ErrorCode::Internal,
                    RUN_INTERACTION_INBOX_BUSY_DETAIL,
                    true,
                    request_id,
                ));
            }
            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                return unsubmitted_wrong_run(request_id, thread_id, run_id, command);
            }
        }
        match acknowledged.await {
            Ok(RunInteractionAck::Settled(stored)) => Ok(stored),
            Ok(RunInteractionAck::Conflict) => Err(typed_failure(
                ErrorCode::IdempotencyConflict,
                format!("request `{request_id}` was already accepted for a different response"),
                false,
                request_id,
            )),
            Ok(RunInteractionAck::WrongRun) => {
                unsubmitted_wrong_run(request_id, thread_id, run_id, command)
            }
            Ok(RunInteractionAck::Unavailable) => Err(typed_failure(
                ErrorCode::Internal,
                "live run interaction is temporarily unavailable",
                true,
                request_id,
            )),
            Err(_) => Err(typed_failure(
                ErrorCode::Internal,
                "live run interaction ended before settling the response",
                true,
                request_id,
            )),
            // Steer acks never arrive here: request-time routing only
            // carries approval/question envelopes, and only the
            // dispatch-side steer arm produces Steer outcomes.
            Ok(RunInteractionAck::Steered | RunInteractionAck::Refused { .. }) => {
                Err(typed_failure(
                    ErrorCode::Internal,
                    "live run interaction answered out of contract",
                    false,
                    request_id,
                ))
            }
        }
    }

    /// Routes one named steer into its owning live run and awaits the
    /// owning loop's acknowledgement.
    ///
    /// Unlike [`Self::route_interaction`] there is no receipt table: the
    /// outbox row accepted by the caller is the durable record, and the
    /// envelope carries the ORIGINAL client request identity so ledger
    /// redelivery dedup applies end to end.
    async fn route_steer(
        &self,
        request_id: &RequestId,
        thread_id: &ThreadId,
        run_id: &artisan_domain::RunId,
        command: &OwnedInteractionCommand,
    ) -> Result<RunInteractionAck, ProtocolFailure> {
        let Some(registry) = self.run_interaction.as_ref() else {
            return Err(typed_failure(
                ErrorCode::UnsupportedFeature,
                RUN_INTERACTION_UNAVAILABLE_DETAIL,
                false,
                request_id,
            ));
        };
        let Some(inbox) = registry
            .route(thread_id, run_id)
            .map_err(|error| run_interaction_failure(error, request_id))?
        else {
            return Ok(RunInteractionAck::WrongRun);
        };
        let (respond, acknowledged) = tokio::sync::oneshot::channel();
        let envelope = RunInteractionEnvelope {
            thread_id: thread_id.clone(),
            run_id: run_id.clone(),
            command: command.clone(),
            respond,
        };
        match inbox.try_send(envelope) {
            Ok(()) => {}
            Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                return Err(typed_failure(
                    ErrorCode::Internal,
                    RUN_INTERACTION_INBOX_BUSY_DETAIL,
                    true,
                    request_id,
                ));
            }
            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                return Ok(RunInteractionAck::WrongRun);
            }
        }
        match acknowledged.await {
            Ok(ack) => Ok(ack),
            Err(_) => Err(typed_failure(
                ErrorCode::Internal,
                "live run interaction ended before settling the steer",
                true,
                request_id,
            )),
        }
    }

    /// Answers one general message mutation from its durable receipt or a
    /// fresh Forge-minted queued message. Unlike the compatibility
    /// `QueueFirstMessage` path, this command is valid for every existing
    /// thread and carries the ordered text/image payload durably through the
    /// outbox.
    pub(super) async fn queue_message_outcome(
        &self,
        request_id: &RequestId,
        queue: &QueueMessage,
    ) -> Result<ServerResponse, ProtocolFailure> {
        if let Some(replay) = self
            .repository
            .lookup_queue_message(
                &queue.request_id,
                &queue.thread_id,
                &queue.payload,
                queue
                    .steer_target
                    .as_ref()
                    .map(artisan_domain::SteerTarget::run_id),
            )
            .await
            .map_err(|error| repository_failure(&error, request_id))?
        {
            return self.settle_replayed_steer(request_id, queue, &replay).await;
        }
        let identity = self
            .origin
            .mint_identity()
            .map_err(|error| origin_entropy_failure(&error, request_id))?;
        let accepted_at = self
            .origin
            .acceptance_instant()
            .map_err(|error| origin_clock_failure(error, request_id))?;
        let message_id = MessageId::parse(identity)
            .map_err(|_| forged_identity_failure("message", request_id))?;
        let result = self
            .repository
            .queue_message(QueueMessageInput {
                request_id: queue.request_id.clone(),
                message_id,
                thread_id: queue.thread_id.clone(),
                payload: queue.payload.clone(),
                steer_run_id: queue
                    .steer_target
                    .as_ref()
                    .map(|target| target.run_id().clone()),
                accepted_at,
            })
            .await
            .map_err(|error| repository_failure(&error, request_id))?;
        if result.receipt.disposition != artisan_domain::ReceiptDisposition::Accepted {
            // Accept-race duplicate: another transaction won admission for
            // this exact intent. Settle from durable delivery state — never
            // bare receipt — so a completed steer replays its receipt, a
            // failed steer reproduces its typed refusal, and only an open
            // row reroutes.
            return self.settle_replayed_steer(request_id, queue, &result).await;
        }
        let receipt = QueueMessageReceipt {
            request_id: result.receipt.request_id,
            message_id: result.message_id.clone(),
            thread_id: result.thread_id.clone(),
            disposition: result.receipt.disposition,
        };
        self.deliver_accepted_steer(request_id, queue, &result.message_id, receipt)
            .await
    }

    /// Delivers a freshly accepted named steer into its owning live run.
    ///
    /// Called only for first-time acceptances carrying a steer target;
    /// accept-race duplicates settle from durable delivery state instead,
    /// so a retried request can never steer twice. On success the
    /// original receipt stands; on terminal refusal the dispatch row is
    /// failed with the mapped reason and the payload stays preserved
    /// for user recovery. Never a silent fresh run.
    pub(super) async fn deliver_accepted_steer(
        &self,
        request_id: &RequestId,
        queue: &QueueMessage,
        message_id: &MessageId,
        receipt: QueueMessageReceipt,
    ) -> Result<ServerResponse, ProtocolFailure> {
        let Some(target) = queue.steer_target.as_ref() else {
            return Ok(outcome(request_id, ResponsePayload::MessageQueued(receipt)));
        };
        let text = queue
            .payload
            .text()
            .map_or_else(String::new, |text| text.as_str().to_owned());
        let has_attachments = !queue.payload.attachments().is_empty();
        self.settle_named_steer(
            request_id,
            &queue.request_id,
            &queue.thread_id,
            target.run_id(),
            message_id,
            text,
            has_attachments,
            receipt,
        )
        .await
    }

    /// Settles a replayed named steer against its durable delivery state.
    ///
    /// A first acceptance always routes; a replay consults the stored
    /// dispatch row instead of delivering blind: completed returns the
    /// stored receipt, failed reproduces the stored typed refusal, and
    /// only an open row safely reroutes the same original
    /// request/message/target. Unnamed replays return the stored receipt
    /// untouched, exactly as before.
    pub(super) async fn settle_replayed_steer(
        &self,
        request_id: &RequestId,
        queue: &QueueMessage,
        replay: &artisan_database::QueueMessageResult,
    ) -> Result<ServerResponse, ProtocolFailure> {
        let receipt = QueueMessageReceipt {
            request_id: replay.receipt.request_id.clone(),
            message_id: replay.message_id.clone(),
            thread_id: replay.thread_id.clone(),
            disposition: replay.receipt.disposition,
        };
        let Some(target) = queue.steer_target.as_ref() else {
            return Ok(outcome(request_id, ResponsePayload::MessageQueued(receipt)));
        };
        let (state, reason, _) = self
            .repository
            .read_steered_dispatch_state(&replay.message_id)
            .await
            .map_err(|error| repository_failure(&error, request_id))?;
        match state {
            artisan_database::entities::DispatchState::Completed => {
                Ok(outcome(request_id, ResponsePayload::MessageQueued(receipt)))
            }
            artisan_database::entities::DispatchState::Failed => {
                Err(match reason.as_deref() {
                    // Image refusals keep their first-error code on replay:
                    // the stored reason reproduces the typed refusal
                    // instead of collapsing to a generic input error.
                    Some("steer does not support image attachments") => typed_failure(
                        ErrorCode::UnsupportedFeature,
                        "steer does not support image attachments; resend without images or as a fresh message",
                        false,
                        request_id,
                    ),
                    Some(stored) => typed_failure(
                        ErrorCode::InvalidInput,
                        stored.to_owned(),
                        false,
                        request_id,
                    ),
                    None => typed_failure(
                        ErrorCode::InvalidInput,
                        "steered send failed",
                        false,
                        request_id,
                    ),
                })
            }
            artisan_database::entities::DispatchState::Queued => {
                let text = replay
                    .payload
                    .text()
                    .map_or_else(String::new, |text| text.as_str().to_owned());
                let has_attachments = !replay.payload.attachments().is_empty();
                self.settle_named_steer(
                    request_id,
                    &replay.receipt.request_id,
                    &replay.thread_id,
                    target.run_id(),
                    &replay.message_id,
                    text,
                    has_attachments,
                    receipt,
                )
                .await
            }
            artisan_database::entities::DispatchState::Leased
            | artisan_database::entities::DispatchState::Running => Err(typed_failure(
                ErrorCode::Internal,
                "steered dispatch left its queued state",
                false,
                request_id,
            )),
        }
    }

    /// Settles one named steer: attachments gate, live route, ack mapping.
    ///
    /// Shared by first acceptances and open-row replays so both funnel
    /// through identical gating. `request_id` is the frame correlation for
    /// responses and failures; `command_request_id` is the durable command
    /// identity carried in the steer envelope, so snapshot reads and ledger
    /// dedup stay stable across retry frames. Image attachments are refused
    /// BEFORE any provider contact — provider steer verbs carry text only —
    /// with the row failed typed and the original payload preserved for
    /// recovery as a fresh send. Text subsets or blank image-only sends
    /// never succeed here.
    #[allow(clippy::too_many_arguments)]
    async fn settle_named_steer(
        &self,
        request_id: &RequestId,
        command_request_id: &RequestId,
        thread_id: &ThreadId,
        target_run_id: &artisan_domain::RunId,
        message_id: &MessageId,
        text: String,
        has_attachments: bool,
        receipt: QueueMessageReceipt,
    ) -> Result<ServerResponse, ProtocolFailure> {
        if has_attachments {
            self.fail_refused_steer_dispatch(
                message_id,
                "steer does not support image attachments",
                request_id,
            )
            .await?;
            return Err(typed_failure(
                ErrorCode::UnsupportedFeature,
                "steer does not support image attachments; resend without images or as a fresh message",
                false,
                request_id,
            ));
        }
        let command = OwnedInteractionCommand::Steer {
            request_id: command_request_id.clone(),
            message_id: message_id.clone(),
            text,
        };
        match self
            .route_steer(request_id, thread_id, target_run_id, &command)
            .await?
        {
            RunInteractionAck::Steered => {
                Ok(outcome(request_id, ResponsePayload::MessageQueued(receipt)))
            }
            RunInteractionAck::Refused { reason } => {
                self.fail_refused_steer_dispatch(message_id, reason, request_id)
                    .await?;
                Err(typed_failure(
                    ErrorCode::InvalidInput,
                    reason,
                    false,
                    request_id,
                ))
            }
            RunInteractionAck::WrongRun => {
                self.fail_refused_steer_dispatch(
                    message_id,
                    "steer target run is no longer live",
                    request_id,
                )
                .await?;
                Err(typed_failure(
                    ErrorCode::InvalidInput,
                    "steer target run is no longer live",
                    false,
                    request_id,
                ))
            }
            RunInteractionAck::Unavailable => Err(typed_failure(
                ErrorCode::Internal,
                "live run interaction is temporarily unavailable",
                true,
                request_id,
            )),
            RunInteractionAck::Settled(_) | RunInteractionAck::Conflict => Err(typed_failure(
                ErrorCode::IdempotencyConflict,
                format!("request `{request_id}` was already accepted for a different response"),
                false,
                request_id,
            )),
        }
    }

    /// Fails one steered dispatch row after a terminal steer refusal.
    async fn fail_refused_steer_dispatch(
        &self,
        message_id: &MessageId,
        reason: &'static str,
        request_id: &RequestId,
    ) -> Result<(), ProtocolFailure> {
        let operated_at = self
            .origin
            .acceptance_instant()
            .map_err(|error| origin_clock_failure(error, request_id))?;
        self.repository
            .fail_steered_dispatch(message_id, reason, operated_at)
            .await
            .map_err(|error| repository_failure(&error, request_id))?;
        Ok(())
    }
}

/// Maps the live interaction registry's fail-closed errors to one bounded,
/// payload-free protocol failure.
fn run_interaction_failure(
    _error: RunInteractionRegistryError,
    request_id: &RequestId,
) -> ProtocolFailure {
    typed_failure(
        ErrorCode::Internal,
        "live run interaction registry is unavailable",
        false,
        request_id,
    )
}

/// Maps interaction repository failures without leaking stored decisions.
///
/// A reused request identity for a different intent answers the dedicated
/// non-retryable idempotency-conflict code; persisted-state problems stay
/// internal without retry hope; only database-operation failures admit that
/// an identical later request may succeed.
fn interaction_repository_failure(
    error: &artisan_database::RunInteractionError,
    request_id: &RequestId,
) -> ProtocolFailure {
    use artisan_database::RunInteractionError as Failure;

    let (code, retryable) = match error {
        Failure::InvalidBindingVersion { .. }
        | Failure::InvalidInteraction(_)
        | Failure::InvalidObservation(_)
        | Failure::InvalidIdentifier(_) => (ErrorCode::InvalidInput, false),
        Failure::RequestConflict { .. } => (ErrorCode::IdempotencyConflict, false),
        Failure::Repository(_) => (ErrorCode::Internal, true),
    };
    typed_failure(code, error.to_string(), retryable, request_id)
}

/// Builds the unstored `wrong_run` receipt for a response its run cannot
/// accept right now.
///
/// The outcome is deliberately never stored: the run may register later (a
/// retry can still apply) or be gone (a retry reports `wrong_run` again).
/// The disposition marks the first answer; it never claims a stored replay.
fn unsubmitted_wrong_run(
    request_id: &RequestId,
    thread_id: &ThreadId,
    run_id: &artisan_domain::RunId,
    command: &OwnedInteractionCommand,
) -> Result<artisan_database::StoredInteractionReceipt, ProtocolFailure> {
    // Steers never route through the approval/question path and carry no
    // receipt-table row, so there is no unstored receipt to build. Fail
    // closed with a typed unreachable-path error instead of inventing a
    // target identity or panicking.
    let (interaction_id, kind, approved, answers) = match command {
        OwnedInteractionCommand::RespondApproval {
            approval_id,
            approved,
            ..
        } => (
            approval_id.clone(),
            artisan_domain::InteractionKind::Approval,
            Some(*approved),
            Vec::new(),
        ),
        OwnedInteractionCommand::RespondQuestion {
            question_id,
            answers,
            ..
        } => (
            question_id.clone(),
            artisan_domain::InteractionKind::Question,
            None,
            answers.clone(),
        ),
        OwnedInteractionCommand::Steer { .. } => {
            return Err(typed_failure(
                ErrorCode::Internal,
                "steer responses never route through the approval/question path",
                false,
                request_id,
            ));
        }
    };
    Ok(artisan_database::StoredInteractionReceipt {
        request_id: request_id.clone(),
        thread_id: thread_id.clone(),
        run_id: run_id.clone(),
        interaction_id,
        kind,
        outcome: InteractionOutcome::WrongRun,
        disposition: artisan_domain::ReceiptDisposition::Accepted,
        approved,
        answers,
        binding_version: 0,
        responded_at_ms: 0,
    })
}

/// Replays the exact intent fingerprint instead of trusting the stored row.
///
/// Rebuilds the domain command from the stored decision plus the supplied
/// naming and compares fingerprints, so a reused request id with a different
/// target or decision is a conflict even when the stored row itself is the
/// only durable evidence.
fn interaction_intent_matches(
    stored: &artisan_database::StoredInteractionReceipt,
    thread_id: &ThreadId,
    run_id: &artisan_domain::RunId,
    command: &OwnedInteractionCommand,
    intent_key: &str,
) -> bool {
    if stored.thread_id != *thread_id || stored.run_id != *run_id {
        return false;
    }
    let kinds_agree = matches!(
        (&stored.kind, command),
        (
            artisan_domain::InteractionKind::Approval,
            OwnedInteractionCommand::RespondApproval { .. }
        ) | (
            artisan_domain::InteractionKind::Question,
            OwnedInteractionCommand::RespondQuestion { .. }
        )
    );
    if !kinds_agree {
        return false;
    }
    let fingerprint = match stored.kind {
        artisan_domain::InteractionKind::Approval => {
            let Some(approved) = stored.approved else {
                return false;
            };
            artisan_domain::RespondApproval::new(
                stored.request_id.clone(),
                thread_id.clone(),
                run_id.clone(),
                stored.interaction_id.clone(),
                approved,
            )
            .intent_key()
        }
        artisan_domain::InteractionKind::Question => {
            let Ok(command) = artisan_domain::RespondQuestion::new(
                stored.request_id.clone(),
                thread_id.clone(),
                run_id.clone(),
                stored.interaction_id.clone(),
                stored.answers.clone(),
            ) else {
                return false;
            };
            command.intent_key()
        }
    };
    // Steers carry no receipt-table row: intent matching never applies
    // to them. Bind the compared target as an option so the Steer arm
    // fails closed without inventing a target identity.
    let command_target = match command {
        OwnedInteractionCommand::RespondApproval { approval_id, .. } => Some(approval_id.clone()),
        OwnedInteractionCommand::RespondQuestion { question_id, .. } => Some(question_id.clone()),
        OwnedInteractionCommand::Steer { .. } => None,
    };
    let Some(command_target) = command_target else {
        return false;
    };
    fingerprint == intent_key && stored.interaction_id == command_target
}

/// Maps a stored domain outcome to its wire disposition.
const fn map_interaction_outcome(outcome: InteractionOutcome) -> RunInteractionOutcome {
    match outcome {
        InteractionOutcome::Applied => RunInteractionOutcome::Applied,
        InteractionOutcome::UnknownTarget => RunInteractionOutcome::UnknownTarget,
        InteractionOutcome::AlreadyResolved => RunInteractionOutcome::AlreadyResolved,
        InteractionOutcome::WrongRun => RunInteractionOutcome::WrongRun,
    }
}

/// Builds the correlated approval response from its stored receipt.
fn approval_response(
    request_id: &RequestId,
    stored: &artisan_database::StoredInteractionReceipt,
) -> Result<ServerResponse, ProtocolFailure> {
    let Some(approved) = stored.approved else {
        return Err(typed_failure(
            ErrorCode::Internal,
            "stored approval response carries no decision",
            false,
            request_id,
        ));
    };
    Ok(outcome(
        request_id,
        ResponsePayload::ApprovalResponse(RespondApprovalReceipt {
            request_id: stored.request_id.clone(),
            thread_id: stored.thread_id.clone(),
            run_id: stored.run_id.clone(),
            approval_id: stored.interaction_id.clone(),
            approved,
            outcome: map_interaction_outcome(stored.outcome),
            disposition: stored.disposition,
        }),
    ))
}

/// Builds the correlated question response from its stored receipt.
fn question_response(
    request_id: &RequestId,
    stored: &artisan_database::StoredInteractionReceipt,
) -> ServerResponse {
    outcome(
        request_id,
        ResponsePayload::QuestionResponse(RespondQuestionReceipt {
            request_id: stored.request_id.clone(),
            thread_id: stored.thread_id.clone(),
            run_id: stored.run_id.clone(),
            question_id: stored.interaction_id.clone(),
            answers: stored.answers.clone(),
            outcome: map_interaction_outcome(stored.outcome),
            disposition: stored.disposition,
        }),
    )
}
