//! Durable pending approval/question rows plus idempotent response receipts.
//!
//! `Repository::record_approval_request` and
//! [`Repository::record_question_request`](Repository::record_question_request)
//! store the provider's requested state (surviving Forge restarts);
//! [`Repository::resolve_approval_response`](Repository::resolve_approval_response)
//! and [`Repository::resolve_question_response`](Repository::resolve_question_response)
//! settle one client response in a single transaction that replays receipts
//! before touching pending state. Every outcome is bind-scoped: the pending
//! row pins the provider binding version observed at request time, and the
//! resolve transaction fences the live run row, so a response for a rebound
//! run is rejected instead of misapplied. This module never prompts, streams,
//! or delivers: the returned snapshots are durable facts only.
//!
//! Observation sequences for the later resolution commit are allocated here,
//! inside the same transaction family, as `MAX(requested, resolved) + 1`
//! starting at 1. Allocation assumes the single-owner discipline the live
//! run registry enforces: only the owning dispatch loop records and resolves
//! for its run, so no two writers can race the counter.

use artisan_domain::{
    ApprovalObservation, ApprovalRequest, InteractionKind, InteractionOutcome, ObservationId,
    ObservationSequence, QuestionInput, QuestionObservation, QuestionOption, ReceiptDisposition,
    RequestId, RespondApproval, RespondQuestion, RunId,
    RunInteractionError as DomainRunInteractionError, ThreadId, UnixMillis,
};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DbBackend, EntityTrait, QueryFilter, Set,
    Statement, TransactionTrait,
};
use serde_json::{Map, Value};
use thiserror::Error;

use crate::entities::pending_run_interaction::{
    self, InteractionKind as StoredKind, InteractionState as StoredState,
};
use crate::entities::run_interaction_receipt::{
    self, InteractionCommandKind as StoredCommandKind, InteractionDisposition as StoredDisposition,
    InteractionOutcomeValue as StoredOutcome,
};
use crate::entities::{self, AssistantRunLifecycle};

use super::{Repository, RepositoryError, database_error, millis};

/// Throwaway identity used only to run domain validation without persisting.
const VALIDATION_ID: &str = "interaction-validation";

/// Borrowed inputs recording one provider approval request.
pub struct RecordApprovalRequest<'a> {
    /// Thread owning the run that asked.
    pub thread_id: &'a ThreadId,
    /// Run that asked.
    pub run_id: &'a RunId,
    /// Provider approval identity.
    pub approval_id: &'a ObservationId,
    /// Human-readable approval description.
    pub description: String,
    /// The provider-neutral action under review.
    pub request: &'a ApprovalRequest,
    /// Time the request was observed.
    pub requested_at: UnixMillis,
    /// Provider binding version the request was produced under.
    pub binding_version: i64,
}

/// Borrowed inputs recording one provider question request.
pub struct RecordQuestionRequest<'a> {
    /// Thread owning the run that asked.
    pub thread_id: &'a ThreadId,
    /// Run that asked.
    pub run_id: &'a RunId,
    /// Provider question identity.
    pub question_id: &'a ObservationId,
    /// The validated question values.
    pub input: &'a QuestionInput,
    /// Time the request was observed.
    pub requested_at: UnixMillis,
    /// Provider binding version the request was produced under.
    pub binding_version: i64,
}

/// Durable receipt information of one recorded request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecordedInteractionReceipt {
    /// Run that asked.
    pub run_id: RunId,
    /// Thread owning the run.
    pub thread_id: ThreadId,
    /// Provider request identity.
    pub interaction_id: ObservationId,
    /// Allocated durable observation sequence of the request.
    pub sequence: u64,
    /// Time the request was observed.
    pub requested_at: UnixMillis,
}

/// Typed outcome of one record call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecordInteractionOutcome {
    /// This call stored the request.
    Recorded(RecordedInteractionReceipt),
    /// An earlier identical record already stored exactly this request.
    AlreadyRecorded(RecordedInteractionReceipt),
}

/// Scope binding one resolve call to its live run.
pub struct ResolveScope {
    /// Provider binding version the resolving owner currently holds.
    pub binding_version: i64,
    /// Time the response is settled.
    pub responded_at: UnixMillis,
}

/// Reconstructed requested state returned with an applied resolution.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RequestedInteractionSnapshot {
    /// Thread owning the run.
    pub thread_id: ThreadId,
    /// Run that asked.
    pub run_id: RunId,
    /// Approval request, when the target is an approval.
    pub approval: Option<ApprovalSnapshot>,
    /// Question request, when the target is a question.
    pub question: Option<QuestionSnapshot>,
}

/// Reconstructed approval request with its durable sequence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ApprovalSnapshot {
    /// Provider approval identity.
    pub approval_id: ObservationId,
    /// Human-readable approval description.
    pub description: String,
    /// The provider-neutral action under review.
    pub request: ApprovalRequest,
    /// Durable observation sequence allocated at request time.
    pub requested_sequence: u64,
}

/// Reconstructed question request with its durable sequence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QuestionSnapshot {
    /// Provider question identity.
    pub question_id: ObservationId,
    /// The validated question values.
    pub input: QuestionInput,
    /// Durable observation sequence allocated at request time.
    pub requested_sequence: u64,
}

/// Stored response receipt returned for replayable outcomes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredInteractionReceipt {
    /// Client-minted response identity.
    pub request_id: RequestId,
    /// Thread named by the response.
    pub thread_id: ThreadId,
    /// Run named by the response.
    pub run_id: RunId,
    /// Provider request identity that was answered.
    pub interaction_id: ObservationId,
    /// Which pending request kind was answered.
    pub kind: InteractionKind,
    /// How the response settled its target.
    pub outcome: InteractionOutcome,
    /// Accepted now or exact duplicate replay.
    pub disposition: ReceiptDisposition,
    /// Recorded approval decision, for approval responses.
    pub approved: Option<bool>,
    /// Recorded answers, for question responses.
    pub answers: Vec<String>,
    /// Binding version the outcome settled under.
    pub binding_version: i64,
    /// Time the response was settled, as epoch milliseconds.
    pub responded_at_ms: i64,
}

/// Durable application of one response with its requested snapshot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppliedInteraction {
    /// The stored accepted receipt.
    pub receipt: StoredInteractionReceipt,
    /// The reconstructed requested state, for the resolution commit.
    pub requested: RequestedInteractionSnapshot,
    /// Allocated durable observation sequence of the resolution.
    pub resolved_sequence: u64,
}

/// Typed outcome of one resolve call.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResolveInteractionOutcome {
    /// This transaction recorded the decision.
    Applied(AppliedInteraction),
    /// An earlier identical response already settled; no second effect.
    Duplicate(StoredInteractionReceipt),
    /// No pending request carries this target id on the run.
    UnknownTarget(StoredInteractionReceipt),
    /// The target was already resolved by an earlier response.
    AlreadyResolved(StoredInteractionReceipt),
    /// The named run is not live, not running, or rebound. Never stored:
    /// the client may retry once the owning run is live.
    WrongRun,
    /// The request id was already accepted for a different intent. The
    /// originally accepted outcome stands.
    Conflict(StoredInteractionReceipt),
}

/// Pending row view for owner-side seeding and tests.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingInteractionView {
    /// Run that asked.
    pub run_id: RunId,
    /// Thread owning the run.
    pub thread_id: ThreadId,
    /// Provider request identity.
    pub interaction_id: ObservationId,
    /// Which pending request kind was asked.
    pub kind: InteractionKind,
    /// Whether the request still awaits a decision.
    pub requested: bool,
    /// Allocated durable observation sequence of the request.
    pub requested_sequence: u64,
    /// Binding version observed at request time.
    pub binding_version: i64,
}

/// Capability-specific failures of the run-interaction boundary.
#[derive(Debug, Error)]
pub enum RunInteractionError {
    /// The supplied binding version is not positive.
    #[error("provider binding version {version} must be positive")]
    InvalidBindingVersion { version: i64 },
    /// The same run/target pair was already recorded with different content.
    #[error("run interaction `{interaction_id}` was already recorded with different content")]
    RequestConflict { interaction_id: String },
    /// A domain interaction value violated its bounds.
    #[error(transparent)]
    InvalidInteraction(#[from] DomainRunInteractionError),
    /// A domain observation value violated its bounds.
    #[error(transparent)]
    InvalidObservation(#[from] artisan_domain::ObservationError),
    /// An identifier failed the shared wire rule.
    #[error(transparent)]
    InvalidIdentifier(#[from] artisan_domain::IdentifierError),
    /// Existing repository-layer rejections surface with their typed source.
    #[error(transparent)]
    Repository(#[from] RepositoryError),
}

impl Repository {
    /// Records one provider approval request as durable requested state.
    ///
    /// The description and request pass through the exact domain
    /// constructors, so a stored row always rebuilds a valid requested
    /// observation. An exact replay answers `AlreadyRecorded`; a colliding
    /// record for the same run/target pair is a conflict.
    ///
    /// # Errors
    ///
    /// Returns [`RunInteractionError`] for invalid bindings, domain bound
    /// violations, record conflicts, or database failures.
    pub async fn record_approval_request(
        &self,
        command: RecordApprovalRequest<'_>,
    ) -> Result<RecordInteractionOutcome, RunInteractionError> {
        if command.binding_version <= 0 {
            return Err(RunInteractionError::InvalidBindingVersion {
                version: command.binding_version,
            });
        }
        // Run the exact domain validation without persisting its identities.
        ApprovalObservation::requested(
            ObservationId::parse(VALIDATION_ID)?,
            ObservationSequence::new(1).expect("sequence one is representable"),
            command.approval_id.clone(),
            command.description.clone(),
            command.request.clone(),
        )?;
        let request_json = approval_request_json(&command.description, command.request);
        self.record_request(
            command.thread_id,
            command.run_id,
            command.approval_id,
            StoredKind::Approval,
            &request_json,
            command.requested_at,
            command.binding_version,
        )
        .await
    }

    /// Records one provider question request as durable requested state.
    ///
    /// Same replay and conflict contract as
    /// [`Repository::record_approval_request`].
    ///
    /// # Errors
    ///
    /// Returns [`RunInteractionError`] for invalid bindings, domain bound
    /// violations, record conflicts, or database failures.
    pub async fn record_question_request(
        &self,
        command: RecordQuestionRequest<'_>,
    ) -> Result<RecordInteractionOutcome, RunInteractionError> {
        if command.binding_version <= 0 {
            return Err(RunInteractionError::InvalidBindingVersion {
                version: command.binding_version,
            });
        }
        QuestionObservation::requested(
            ObservationId::parse(VALIDATION_ID)?,
            ObservationSequence::new(1).expect("sequence one is representable"),
            command.input.clone(),
        )?;
        let request_json = question_request_json(command.input);
        self.record_request(
            command.thread_id,
            command.run_id,
            command.question_id,
            StoredKind::Question,
            &request_json,
            command.requested_at,
            command.binding_version,
        )
        .await
    }

    /// Resolves one approval response against its live owning run.
    ///
    /// One transaction replays the response receipt, fences the pending row
    /// and the live run row (including its binding version), then stores the
    /// outcome. `WrongRun` is never stored so the client can retry once the
    /// owning run is live; every other outcome stores its receipt.
    ///
    /// # Errors
    ///
    /// Returns [`RunInteractionError`] for invalid scopes or database
    /// failures. Target misses and conflicts are outcomes, not errors.
    pub async fn resolve_approval_response(
        &self,
        command: &RespondApproval,
        scope: &ResolveScope,
    ) -> Result<ResolveInteractionOutcome, RunInteractionError> {
        if scope.binding_version <= 0 {
            return Err(RunInteractionError::InvalidBindingVersion {
                version: scope.binding_version,
            });
        }
        self.resolve_response(
            &command.request_id,
            &command.thread_id,
            &command.run_id,
            &command.approval_id,
            StoredKind::Approval,
            StoredCommandKind::RespondApproval,
            &command.intent_key(),
            Some(command.approved()),
            &[],
            scope,
        )
        .await
    }

    /// Resolves one question response against its live owning run.
    ///
    /// Same receipt-replay, fencing, and storage contract as
    /// [`Repository::resolve_approval_response`]. The command arrives
    /// domain-validated; its answers echo into the stored receipt.
    ///
    /// # Errors
    ///
    /// Returns [`RunInteractionError`] for invalid scopes or database
    /// failures. Target misses and conflicts are outcomes, not errors.
    pub async fn resolve_question_response(
        &self,
        command: &RespondQuestion,
        scope: &ResolveScope,
    ) -> Result<ResolveInteractionOutcome, RunInteractionError> {
        if scope.binding_version <= 0 {
            return Err(RunInteractionError::InvalidBindingVersion {
                version: scope.binding_version,
            });
        }
        self.resolve_response(
            command.request_id(),
            command.thread_id(),
            command.run_id(),
            command.question_id(),
            StoredKind::Question,
            StoredCommandKind::RespondQuestion,
            &command.intent_key(),
            None,
            command.answers(),
            scope,
        )
        .await
    }

    /// Looks up one stored response receipt by its client request id.
    ///
    /// # Errors
    ///
    /// Returns [`RunInteractionError`] for corrupt rows or database failures.
    pub async fn lookup_interaction_receipt(
        &self,
        request_id: &RequestId,
    ) -> Result<Option<StoredInteractionReceipt>, RunInteractionError> {
        let Some(row) = run_interaction_receipt::Entity::find_by_id(request_id.as_str())
            .one(&self.database)
            .await
            .map_err(|source| {
                RunInteractionError::Repository(database_error(
                    "lookup interaction receipt",
                    source,
                ))
            })?
        else {
            return Ok(None);
        };
        stored_receipt(&row).map(Some)
    }

    /// Lists every pending row for one run, requested or resolved.
    ///
    /// The owning dispatch loop seeds its delivery ledger from this view;
    /// tests assert per-run scoping through it.
    ///
    /// # Errors
    ///
    /// Returns [`RunInteractionError`] for corrupt rows or database failures.
    pub async fn pending_interactions(
        &self,
        run_id: &RunId,
    ) -> Result<Vec<PendingInteractionView>, RunInteractionError> {
        let rows = pending_run_interaction::Entity::find()
            .filter(pending_run_interaction::Column::RunId.eq(run_id.as_str()))
            .all(&self.database)
            .await
            .map_err(|source| {
                RunInteractionError::Repository(database_error("list pending interactions", source))
            })?;
        rows.iter().map(pending_view).collect()
    }

    /// Returns the newest requested instant per run with open requests.
    ///
    /// This is the durable `approval_requested_at` signal the activity and
    /// wake-lock policy consumes so human-blocked runs are not reaped as
    /// stalled: one instant per run, runs with no open request absent.
    ///
    /// # Errors
    ///
    /// Returns [`RunInteractionError`] for database failures.
    pub async fn pending_request_instants(&self) -> Result<Vec<i64>, RunInteractionError> {
        let rows = self
            .database
            .query_all_raw(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                "SELECT MAX(requested_at_ms) AS requested_at_ms FROM pending_run_interactions WHERE state = 'requested' GROUP BY run_id ORDER BY requested_at_ms",
                [],
            ))
            .await
            .map_err(|source| {
                RunInteractionError::Repository(database_error(
                    "read pending request instants",
                    source,
                ))
            })?;
        let mut instants = Vec::with_capacity(rows.len());
        for row in rows {
            let instant: i64 = row.try_get("", "requested_at_ms").map_err(|source| {
                RunInteractionError::Repository(database_error(
                    "read pending request instants",
                    source,
                ))
            })?;
            instants.push(instant);
        }
        Ok(instants)
    }

    /// Deletes every pending row for one run and returns the removed count.
    ///
    /// Runs call this when they settle so decisions never leak across runs.
    /// Idempotent: settling twice removes nothing the second time. Receipts
    /// are intentionally retained: replays must still answer `duplicate`.
    ///
    /// # Errors
    ///
    /// Returns [`RunInteractionError`] for database failures.
    pub async fn settle_run_interactions(
        &self,
        run_id: &RunId,
    ) -> Result<u64, RunInteractionError> {
        let result = pending_run_interaction::Entity::delete_many()
            .filter(pending_run_interaction::Column::RunId.eq(run_id.as_str()))
            .exec(&self.database)
            .await
            .map_err(|source| {
                RunInteractionError::Repository(database_error("settle run interactions", source))
            })?;
        Ok(result.rows_affected)
    }

    /// Returns the greatest observation sequence committed for one run.
    ///
    /// Reads the persisted observation checkpoint written by the S1b batch
    /// path and returns its maximum sequence, or `None` when no observation
    /// batch has committed yet. Resolution commits chain from this value so
    /// sequences stay strictly increasing across batches.
    ///
    /// # Errors
    ///
    /// Returns [`RunInteractionError`] for corrupt checkpoint bytes or
    /// database failures.
    pub async fn last_committed_observation_sequence(
        &self,
        run_id: &RunId,
    ) -> Result<Option<u64>, RunInteractionError> {
        let Some(row) = entities::run_checkpoint::Entity::find_by_id(run_id.as_str())
            .one(&self.database)
            .await
            .map_err(|source| {
                RunInteractionError::Repository(database_error(
                    "read observation checkpoint",
                    source,
                ))
            })?
        else {
            return Ok(None);
        };
        let (Some(version), Some(blob)) = (
            row.engine_checkpoint_version,
            row.engine_checkpoint_blob.as_ref(),
        ) else {
            return Ok(None);
        };
        let decoded =
            super::run_observation::decode_observation_checkpoint(version, blob.as_slice())
                .map_err(|source| {
                    RunInteractionError::Repository(super::corrupt_data(
                        "run_checkpoints",
                        "engine_checkpoint_blob",
                        &source,
                    ))
                })?;
        Ok(decoded.max_sequence())
    }

    async fn record_request(
        &self,
        thread_id: &ThreadId,
        run_id: &RunId,
        interaction_id: &ObservationId,
        kind: StoredKind,
        request_json: &str,
        requested_at: UnixMillis,
        binding_version: i64,
    ) -> Result<RecordInteractionOutcome, RunInteractionError> {
        let transaction = self.database.begin().await.map_err(|source| {
            RunInteractionError::Repository(database_error("begin record interaction", source))
        })?;
        let existing = pending_run_interaction::Entity::find()
            .filter(pending_run_interaction::Column::RunId.eq(run_id.as_str()))
            .filter(pending_run_interaction::Column::InteractionId.eq(interaction_id.as_str()))
            .one(&transaction)
            .await
            .map_err(|source| {
                RunInteractionError::Repository(database_error("fence record interaction", source))
            })?;
        if let Some(existing) = existing {
            let receipt = recorded_receipt(thread_id, run_id, &existing, requested_at)?;
            if existing.kind != kind || existing.request_json != request_json {
                transaction.rollback().await.map_err(|source| {
                    RunInteractionError::Repository(database_error(
                        "roll back record interaction conflict",
                        source,
                    ))
                })?;
                return Err(RunInteractionError::RequestConflict {
                    interaction_id: interaction_id.as_str().to_owned(),
                });
            }
            transaction.rollback().await.map_err(|source| {
                RunInteractionError::Repository(database_error(
                    "roll back record interaction replay",
                    source,
                ))
            })?;
            return Ok(RecordInteractionOutcome::AlreadyRecorded(receipt));
        }
        let sequence = next_sequence(&transaction, run_id).await?;
        let model = pending_run_interaction::ActiveModel {
            run_id: Set(run_id.as_str().to_owned()),
            interaction_id: Set(interaction_id.as_str().to_owned()),
            thread_id: Set(thread_id.as_str().to_owned()),
            kind: Set(kind),
            state: Set(StoredState::Requested),
            request_json: Set(request_json.to_owned()),
            requested_sequence: Set(sequence_i64(sequence)?),
            approved: Set(None),
            answers_json: Set(None),
            requested_at_ms: Set(millis(requested_at)),
            resolved_at_ms: Set(None),
            resolved_sequence: Set(None),
            binding_version: Set(binding_version),
            ..Default::default()
        };
        model.insert(&transaction).await.map_err(|source| {
            RunInteractionError::Repository(database_error("insert pending interaction", source))
        })?;
        transaction.commit().await.map_err(|source| {
            RunInteractionError::Repository(database_error("commit record interaction", source))
        })?;
        Ok(RecordInteractionOutcome::Recorded(
            RecordedInteractionReceipt {
                run_id: run_id.clone(),
                thread_id: thread_id.clone(),
                interaction_id: interaction_id.clone(),
                sequence,
                requested_at,
            },
        ))
    }

    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::too_many_lines)]
    async fn resolve_response(
        &self,
        request_id: &RequestId,
        thread_id: &ThreadId,
        run_id: &RunId,
        interaction_id: &ObservationId,
        kind: StoredKind,
        command_kind: StoredCommandKind,
        intent_key: &str,
        approved: Option<bool>,
        answers: &[String],
        scope: &ResolveScope,
    ) -> Result<ResolveInteractionOutcome, RunInteractionError> {
        let transaction = self.database.begin().await.map_err(|source| {
            RunInteractionError::Repository(database_error("begin resolve interaction", source))
        })?;
        if let Some(row) = run_interaction_receipt::Entity::find_by_id(request_id.as_str())
            .one(&transaction)
            .await
            .map_err(|source| {
                RunInteractionError::Repository(database_error(
                    "replay resolve interaction",
                    source,
                ))
            })?
        {
            let stored = stored_receipt(&row)?;
            transaction.rollback().await.map_err(|source| {
                RunInteractionError::Repository(database_error(
                    "roll back resolve interaction replay",
                    source,
                ))
            })?;
            if stored_receipt_matches(&stored, thread_id, run_id, interaction_id, intent_key) {
                let mut replay = stored;
                replay.disposition = ReceiptDisposition::Duplicate;
                return Ok(ResolveInteractionOutcome::Duplicate(replay));
            }
            return Ok(ResolveInteractionOutcome::Conflict(stored));
        }
        if !live_run_matches(&transaction, thread_id, run_id, scope).await? {
            transaction.rollback().await.map_err(|source| {
                RunInteractionError::Repository(database_error(
                    "roll back resolve interaction wrong run",
                    source,
                ))
            })?;
            return Ok(ResolveInteractionOutcome::WrongRun);
        }
        let pending = pending_run_interaction::Entity::find()
            .filter(pending_run_interaction::Column::RunId.eq(run_id.as_str()))
            .filter(pending_run_interaction::Column::InteractionId.eq(interaction_id.as_str()))
            .one(&transaction)
            .await
            .map_err(|source| {
                RunInteractionError::Repository(database_error("fence resolve interaction", source))
            })?;
        let Some(pending) = pending else {
            let stored = self
                .store_outcome(
                    &transaction,
                    request_id,
                    thread_id,
                    run_id,
                    interaction_id,
                    kind,
                    command_kind,
                    InteractionOutcome::UnknownTarget,
                    intent_key,
                    approved,
                    answers,
                    scope,
                )
                .await?;
            transaction.commit().await.map_err(|source| {
                RunInteractionError::Repository(database_error(
                    "commit resolve interaction unknown target",
                    source,
                ))
            })?;
            return Ok(ResolveInteractionOutcome::UnknownTarget(stored));
        };
        if pending.kind != kind {
            let stored = self
                .store_outcome(
                    &transaction,
                    request_id,
                    thread_id,
                    run_id,
                    interaction_id,
                    kind,
                    command_kind,
                    InteractionOutcome::UnknownTarget,
                    intent_key,
                    approved,
                    answers,
                    scope,
                )
                .await?;
            transaction.commit().await.map_err(|source| {
                RunInteractionError::Repository(database_error(
                    "commit resolve interaction kind mismatch",
                    source,
                ))
            })?;
            return Ok(ResolveInteractionOutcome::UnknownTarget(stored));
        }
        if pending.state == StoredState::Resolved {
            let stored = self
                .store_outcome(
                    &transaction,
                    request_id,
                    thread_id,
                    run_id,
                    interaction_id,
                    kind,
                    command_kind,
                    InteractionOutcome::AlreadyResolved,
                    intent_key,
                    approved,
                    answers,
                    scope,
                )
                .await?;
            transaction.commit().await.map_err(|source| {
                RunInteractionError::Repository(database_error(
                    "commit resolve interaction already resolved",
                    source,
                ))
            })?;
            return Ok(ResolveInteractionOutcome::AlreadyResolved(stored));
        }
        if pending.thread_id != thread_id.as_str()
            || !bind_matches(&transaction, &pending, scope).await?
        {
            transaction.rollback().await.map_err(|source| {
                RunInteractionError::Repository(database_error(
                    "roll back resolve interaction wrong run",
                    source,
                ))
            })?;
            return Ok(ResolveInteractionOutcome::WrongRun);
        }
        let resolved_sequence = next_sequence(&transaction, run_id).await?;
        let encoded_answers = if matches!(kind, StoredKind::Question) {
            Some(answers_json(answers)?)
        } else {
            None
        };
        let mut pending_active: pending_run_interaction::ActiveModel = pending.clone().into();
        pending_active.state = Set(StoredState::Resolved);
        pending_active.approved = Set(approved.map(i32::from));
        pending_active.answers_json = Set(encoded_answers);
        pending_active.resolved_at_ms = Set(Some(millis(scope.responded_at)));
        pending_active.resolved_sequence = Set(Some(sequence_i64(resolved_sequence)?));
        pending_active
            .update(&transaction)
            .await
            .map_err(|source| {
                RunInteractionError::Repository(database_error(
                    "resolve pending interaction",
                    source,
                ))
            })?;
        let stored = self
            .store_outcome(
                &transaction,
                request_id,
                thread_id,
                run_id,
                interaction_id,
                kind,
                command_kind,
                InteractionOutcome::Applied,
                intent_key,
                approved,
                answers,
                scope,
            )
            .await?;
        transaction.commit().await.map_err(|source| {
            RunInteractionError::Repository(database_error("commit resolve interaction", source))
        })?;
        let requested = requested_snapshot(&pending)?;
        Ok(ResolveInteractionOutcome::Applied(AppliedInteraction {
            receipt: stored,
            requested,
            resolved_sequence,
        }))
    }

    #[allow(clippy::too_many_arguments)]
    async fn store_outcome(
        &self,
        transaction: &sea_orm::DatabaseTransaction,
        request_id: &RequestId,
        thread_id: &ThreadId,
        run_id: &RunId,
        interaction_id: &ObservationId,
        kind: StoredKind,
        command_kind: StoredCommandKind,
        outcome: InteractionOutcome,
        intent_key: &str,
        approved: Option<bool>,
        answers: &[String],
        scope: &ResolveScope,
    ) -> Result<StoredInteractionReceipt, RunInteractionError> {
        let encoded_answers = if matches!(command_kind, StoredCommandKind::RespondQuestion) {
            Some(answers_json(answers)?)
        } else {
            None
        };
        let model = run_interaction_receipt::ActiveModel {
            request_id: Set(request_id.as_str().to_owned()),
            command_kind: Set(command_kind),
            thread_id: Set(thread_id.as_str().to_owned()),
            run_id: Set(run_id.as_str().to_owned()),
            interaction_id: Set(interaction_id.as_str().to_owned()),
            outcome: Set(match outcome {
                InteractionOutcome::Applied => StoredOutcome::Applied,
                InteractionOutcome::UnknownTarget => StoredOutcome::UnknownTarget,
                InteractionOutcome::AlreadyResolved => StoredOutcome::AlreadyResolved,
                InteractionOutcome::WrongRun => StoredOutcome::WrongRun,
            }),
            disposition: Set(StoredDisposition::Accepted),
            intent_key: Set(intent_key.to_owned()),
            approved: Set(approved.map(i32::from)),
            answers_json: Set(encoded_answers),
            binding_version: Set(scope.binding_version),
            responded_at_ms: Set(millis(scope.responded_at)),
            ..Default::default()
        };
        model.insert(transaction).await.map_err(|source| {
            RunInteractionError::Repository(database_error("insert interaction receipt", source))
        })?;
        Ok(StoredInteractionReceipt {
            request_id: request_id.clone(),
            thread_id: thread_id.clone(),
            run_id: run_id.clone(),
            interaction_id: interaction_id.clone(),
            kind: match kind {
                StoredKind::Approval => InteractionKind::Approval,
                StoredKind::Question => InteractionKind::Question,
            },
            outcome,
            disposition: ReceiptDisposition::Accepted,
            approved,
            answers: answers.to_vec(),
            binding_version: scope.binding_version,
            responded_at_ms: millis(scope.responded_at),
        })
    }
}

async fn next_sequence(
    transaction: &sea_orm::DatabaseTransaction,
    run_id: &RunId,
) -> Result<u64, RunInteractionError> {
    let requested = pending_run_interaction::Entity::find()
        .filter(pending_run_interaction::Column::RunId.eq(run_id.as_str()))
        .all(transaction)
        .await
        .map_err(|source| {
            RunInteractionError::Repository(database_error("allocate interaction sequence", source))
        })?;
    let mut maximum: i64 = 0;
    for row in &requested {
        maximum = maximum.max(row.requested_sequence);
        if let Some(resolved) = row.resolved_sequence {
            maximum = maximum.max(resolved);
        }
    }
    let next = maximum.checked_add(1).ok_or_else(|| {
        RunInteractionError::Repository(RepositoryError::Invariant {
            reason: "run interaction sequence space is exhausted",
        })
    })?;
    u64::try_from(next).map_err(|_| {
        RunInteractionError::Repository(RepositoryError::Invariant {
            reason: "run interaction sequence is not representable",
        })
    })
}

/// Converts an allocated sequence for its signed persistence column.
///
/// The allocator already bounds sequences to the signed range; this is the
/// total projection the column write needs.
fn sequence_i64(sequence: u64) -> Result<i64, RunInteractionError> {
    i64::try_from(sequence).map_err(|_| {
        RunInteractionError::Repository(RepositoryError::Invariant {
            reason: "run interaction sequence is not representable",
        })
    })
}

/// Requires the NAMED run itself to be live before any pending state is
/// consulted.
///
/// The row must exist, belong to the command thread, still be `running`,
/// and carry the resolving scope's provider binding version. Any divergence
/// is a foreign, settled, or rebound run: reject without storing so the
/// client can retry against the owning run. This first-layer fence keeps a
/// response for a never-launched run out of `UnknownTarget`; the pending-row
/// bind check below stays as the second layer.
async fn live_run_matches(
    transaction: &sea_orm::DatabaseTransaction,
    thread_id: &ThreadId,
    run_id: &RunId,
    scope: &ResolveScope,
) -> Result<bool, RunInteractionError> {
    let Some(run) = entities::assistant_run::Entity::find_by_id(run_id.as_str())
        .one(transaction)
        .await
        .map_err(|source| {
            RunInteractionError::Repository(database_error(
                "fence resolve interaction live run",
                source,
            ))
        })?
    else {
        return Ok(false);
    };
    if run.thread_id != thread_id.as_str() {
        return Ok(false);
    }
    if run.lifecycle != AssistantRunLifecycle::Running {
        return Ok(false);
    }
    Ok(run.provider_binding_version == Some(scope.binding_version))
}

/// Requires the pending row to agree with the live bound run.
///
/// The pending binding version must equal both the resolving scope and the
/// run's currently persisted provider binding, and the run must still be
/// `running`. Any divergence is a rebound or settled run: reject without
/// storing so the client can retry against the owning run.
async fn bind_matches(
    transaction: &sea_orm::DatabaseTransaction,
    pending: &pending_run_interaction::Model,
    scope: &ResolveScope,
) -> Result<bool, RunInteractionError> {
    if pending.binding_version != scope.binding_version {
        return Ok(false);
    }
    let Some(run) = entities::assistant_run::Entity::find_by_id(pending.run_id.as_str())
        .one(transaction)
        .await
        .map_err(|source| {
            RunInteractionError::Repository(database_error("fence resolve interaction run", source))
        })?
    else {
        return Ok(false);
    };
    if run.lifecycle != AssistantRunLifecycle::Running {
        return Ok(false);
    }
    Ok(run.provider_binding_version == Some(scope.binding_version))
}

fn recorded_receipt(
    thread_id: &ThreadId,
    run_id: &RunId,
    existing: &pending_run_interaction::Model,
    requested_at: UnixMillis,
) -> Result<RecordedInteractionReceipt, RunInteractionError> {
    Ok(RecordedInteractionReceipt {
        run_id: run_id.clone(),
        thread_id: thread_id.clone(),
        interaction_id: ObservationId::parse(existing.interaction_id.clone())?,
        sequence: u64::try_from(existing.requested_sequence).map_err(|_| {
            RunInteractionError::Repository(RepositoryError::Invariant {
                reason: "run interaction sequence is not representable",
            })
        })?,
        requested_at,
    })
}

fn pending_view(
    row: &pending_run_interaction::Model,
) -> Result<PendingInteractionView, RunInteractionError> {
    Ok(PendingInteractionView {
        run_id: RunId::parse(row.run_id.clone())?,
        thread_id: ThreadId::parse(row.thread_id.clone())?,
        interaction_id: ObservationId::parse(row.interaction_id.clone())?,
        kind: match row.kind {
            StoredKind::Approval => InteractionKind::Approval,
            StoredKind::Question => InteractionKind::Question,
        },
        requested: row.state == StoredState::Requested,
        requested_sequence: u64::try_from(row.requested_sequence).map_err(|_| {
            RunInteractionError::Repository(RepositoryError::Invariant {
                reason: "run interaction sequence is not representable",
            })
        })?,
        binding_version: row.binding_version,
    })
}

fn stored_receipt(
    row: &run_interaction_receipt::Model,
) -> Result<StoredInteractionReceipt, RunInteractionError> {
    let answers = match row.answers_json.as_deref() {
        Some(encoded) => decoded_answers(encoded)?,
        None => Vec::new(),
    };
    Ok(StoredInteractionReceipt {
        request_id: RequestId::parse(row.request_id.clone())?,
        thread_id: ThreadId::parse(row.thread_id.clone())?,
        run_id: RunId::parse(row.run_id.clone())?,
        interaction_id: ObservationId::parse(row.interaction_id.clone())?,
        kind: match row.command_kind {
            StoredCommandKind::RespondApproval => InteractionKind::Approval,
            StoredCommandKind::RespondQuestion => InteractionKind::Question,
        },
        outcome: match row.outcome {
            StoredOutcome::Applied => InteractionOutcome::Applied,
            StoredOutcome::UnknownTarget => InteractionOutcome::UnknownTarget,
            StoredOutcome::AlreadyResolved => InteractionOutcome::AlreadyResolved,
            StoredOutcome::WrongRun => InteractionOutcome::WrongRun,
        },
        disposition: match row.disposition {
            StoredDisposition::Accepted => ReceiptDisposition::Accepted,
            StoredDisposition::Duplicate => ReceiptDisposition::Duplicate,
        },
        approved: row.approved.map(|decision| decision != 0),
        answers,
        binding_version: row.binding_version,
        responded_at_ms: row.responded_at_ms,
    })
}

fn stored_receipt_matches(
    stored: &StoredInteractionReceipt,
    thread_id: &ThreadId,
    run_id: &RunId,
    interaction_id: &ObservationId,
    intent_key: &str,
) -> bool {
    // The receipt primary key already proves the request id; the remaining
    // comparison replays the exact intent fingerprint instead of trusting it.
    let fingerprint = match stored.kind {
        InteractionKind::Approval => {
            let Some(approved) = stored.approved else {
                return false;
            };
            artisan_domain::RespondApproval::new(
                stored.request_id.clone(),
                thread_id.clone(),
                run_id.clone(),
                interaction_id.clone(),
                approved,
            )
            .intent_key()
        }
        InteractionKind::Question => {
            let Ok(command) = artisan_domain::RespondQuestion::new(
                stored.request_id.clone(),
                thread_id.clone(),
                run_id.clone(),
                interaction_id.clone(),
                stored.answers.clone(),
            ) else {
                return false;
            };
            command.intent_key()
        }
    };
    stored.thread_id == *thread_id
        && stored.run_id == *run_id
        && stored.interaction_id == *interaction_id
        && fingerprint == intent_key
}

/// Rebuilds the requested snapshot from its canonical stored JSON.
///
/// Every value passes through the exact domain constructors again, so a row
/// that cannot rebuild a valid request surfaces as corrupt data instead of a
/// malformed resolution.
fn requested_snapshot(
    pending: &pending_run_interaction::Model,
) -> Result<RequestedInteractionSnapshot, RunInteractionError> {
    let thread_id = ThreadId::parse(pending.thread_id.clone())?;
    let run_id = RunId::parse(pending.run_id.clone())?;
    let interaction_id = ObservationId::parse(pending.interaction_id.clone())?;
    let requested_sequence = u64::try_from(pending.requested_sequence).map_err(|_| {
        RunInteractionError::Repository(RepositoryError::Invariant {
            reason: "run interaction sequence is not representable",
        })
    })?;
    let value: Value = serde_json::from_str(&pending.request_json).map_err(|source| {
        RunInteractionError::Repository(super::corrupt_data(
            "pending_run_interactions",
            "request_json",
            &source,
        ))
    })?;
    match pending.kind {
        StoredKind::Approval => {
            let (description, request) = approval_request_from_json(&value)?;
            Ok(RequestedInteractionSnapshot {
                thread_id,
                run_id,
                approval: Some(ApprovalSnapshot {
                    approval_id: interaction_id,
                    description,
                    request,
                    requested_sequence,
                }),
                question: None,
            })
        }
        StoredKind::Question => {
            let input = question_input_from_json(&value)?;
            Ok(RequestedInteractionSnapshot {
                thread_id,
                run_id,
                approval: None,
                question: Some(QuestionSnapshot {
                    question_id: interaction_id,
                    input,
                    requested_sequence,
                }),
            })
        }
    }
}

fn approval_request_json(description: &str, request: &ApprovalRequest) -> String {
    let mut map = Map::new();
    map.insert(
        "description".to_owned(),
        Value::String(description.to_owned()),
    );
    map.insert(
        "kind".to_owned(),
        Value::String(request.kind().as_str().to_owned()),
    );
    if let Some(command) = request.command_text() {
        map.insert("command".to_owned(), Value::String(command.to_owned()));
    }
    if let Some(cwd) = request.cwd() {
        map.insert("cwd".to_owned(), Value::String(cwd.to_owned()));
    }
    if let Some(reason) = request.reason() {
        map.insert("reason".to_owned(), Value::String(reason.to_owned()));
    }
    Value::Object(map).to_string()
}

fn question_request_json(input: &QuestionInput) -> String {
    let mut map = Map::new();
    map.insert("text".to_owned(), Value::String(input.text.clone()));
    if let Some(header) = input.header.as_deref() {
        map.insert("header".to_owned(), Value::String(header.to_owned()));
    }
    map.insert("multi_select".to_owned(), Value::Bool(input.multi_select));
    if let Some(options) = input.options.as_deref() {
        let encoded = options
            .iter()
            .map(|option| {
                let mut entry = Map::new();
                entry.insert("label".to_owned(), Value::String(option.label().to_owned()));
                if let Some(description) = option.description() {
                    entry.insert(
                        "description".to_owned(),
                        Value::String(description.to_owned()),
                    );
                }
                Value::Object(entry)
            })
            .collect::<Vec<_>>();
        map.insert("options".to_owned(), Value::Array(encoded));
    }
    Value::Object(map).to_string()
}

fn answers_json(answers: &[String]) -> Result<String, RunInteractionError> {
    serde_json::to_string(answers).map_err(|source| {
        RunInteractionError::Repository(super::corrupt_data(
            "run_interaction_receipts",
            "answers_json",
            &source,
        ))
    })
}

fn decoded_answers(encoded: &str) -> Result<Vec<String>, RunInteractionError> {
    let value: Value = serde_json::from_str(encoded).map_err(|source| {
        RunInteractionError::Repository(super::corrupt_data(
            "run_interaction_receipts",
            "answers_json",
            &source,
        ))
    })?;
    let answers = value.as_array().ok_or_else(|| {
        RunInteractionError::Repository(super::corrupt_data(
            "run_interaction_receipts",
            "answers_json",
            "expected a JSON array",
        ))
    })?;
    let mut decoded = Vec::with_capacity(answers.len());
    for answer in answers {
        decoded.push(
            answer
                .as_str()
                .ok_or_else(|| {
                    RunInteractionError::Repository(super::corrupt_data(
                        "run_interaction_receipts",
                        "answers_json",
                        "expected JSON strings",
                    ))
                })?
                .to_owned(),
        );
    }
    Ok(decoded)
}

fn json_string(value: &Value, field: &'static str) -> Result<String, RunInteractionError> {
    value
        .as_str()
        .ok_or_else(|| {
            RunInteractionError::Repository(super::corrupt_data(
                "pending_run_interactions",
                "request_json",
                field,
            ))
        })
        .map(str::to_owned)
}

fn json_optional_string(value: &Value, field: &str) -> Result<Option<String>, RunInteractionError> {
    value
        .get(field)
        .map(|entry| {
            entry.as_str().ok_or_else(|| {
                RunInteractionError::Repository(super::corrupt_data(
                    "pending_run_interactions",
                    "request_json",
                    field,
                ))
            })
        })
        .transpose()
        .map(|entry| entry.map(str::to_owned))
}

fn approval_request_from_json(
    value: &Value,
) -> Result<(String, ApprovalRequest), RunInteractionError> {
    let object = value.as_object().ok_or_else(|| {
        RunInteractionError::Repository(super::corrupt_data(
            "pending_run_interactions",
            "request_json",
            "expected a JSON object",
        ))
    })?;
    let description = object
        .get("description")
        .ok_or_else(|| {
            RunInteractionError::Repository(super::corrupt_data(
                "pending_run_interactions",
                "request_json",
                "missing description",
            ))
        })
        .and_then(|entry| json_string(entry, "description"))?;
    let kind = object
        .get("kind")
        .ok_or_else(|| {
            RunInteractionError::Repository(super::corrupt_data(
                "pending_run_interactions",
                "request_json",
                "missing kind",
            ))
        })
        .and_then(|entry| json_string(entry, "kind"))?;
    let command = json_optional_string(value, "command")?;
    let cwd = json_optional_string(value, "cwd")?;
    let reason = json_optional_string(value, "reason")?;
    let request = match kind.as_str() {
        "command" => {
            let Some(command) = command else {
                return Err(RunInteractionError::Repository(super::corrupt_data(
                    "pending_run_interactions",
                    "request_json",
                    "command approval requires a command",
                )));
            };
            ApprovalRequest::command(command, cwd, reason)?
        }
        "file_change" => ApprovalRequest::file_change(reason)?,
        "action" => ApprovalRequest::action(reason)?,
        _ => {
            return Err(RunInteractionError::Repository(super::corrupt_data(
                "pending_run_interactions",
                "request_json",
                "unknown approval kind",
            )));
        }
    };
    // Re-run the exact stored-shape validation so a corrupt row can never
    // produce a malformed resolution observation.
    ApprovalObservation::requested(
        ObservationId::parse(VALIDATION_ID)?,
        ObservationSequence::new(1).expect("sequence one is representable"),
        ObservationId::parse(VALIDATION_ID)?,
        description.clone(),
        request.clone(),
    )?;
    Ok((description, request))
}

fn question_input_from_json(value: &Value) -> Result<QuestionInput, RunInteractionError> {
    let object = value.as_object().ok_or_else(|| {
        RunInteractionError::Repository(super::corrupt_data(
            "pending_run_interactions",
            "request_json",
            "expected a JSON object",
        ))
    })?;
    let text = object
        .get("text")
        .ok_or_else(|| {
            RunInteractionError::Repository(super::corrupt_data(
                "pending_run_interactions",
                "request_json",
                "missing text",
            ))
        })
        .and_then(|entry| json_string(entry, "text"))?;
    let header = json_optional_string(value, "header")?;
    let multi_select = object
        .get("multi_select")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let options = object
        .get("options")
        .map(|entries| {
            entries
                .as_array()
                .ok_or_else(|| {
                    RunInteractionError::Repository(super::corrupt_data(
                        "pending_run_interactions",
                        "request_json",
                        "expected an option array",
                    ))
                })
                .and_then(|entries| {
                    entries
                        .iter()
                        .map(|entry| {
                            let label = entry
                                .get("label")
                                .ok_or_else(|| {
                                    RunInteractionError::Repository(super::corrupt_data(
                                        "pending_run_interactions",
                                        "request_json",
                                        "missing option label",
                                    ))
                                })
                                .and_then(|label| json_string(label, "label"))?;
                            let description = entry
                                .get("description")
                                .map(|entry| json_string(entry, "description"))
                                .transpose()?;
                            QuestionOption::new(label, description).map_err(|source| {
                                RunInteractionError::Repository(super::corrupt_data(
                                    "pending_run_interactions",
                                    "request_json",
                                    &source,
                                ))
                            })
                        })
                        .collect::<Result<Vec<_>, _>>()
                })
        })
        .transpose()?;
    let input = QuestionInput {
        question_id: ObservationId::parse(VALIDATION_ID)?,
        text,
        header,
        multi_select,
        options,
    };
    QuestionObservation::requested(
        ObservationId::parse(VALIDATION_ID)?,
        ObservationSequence::new(1).expect("sequence one is representable"),
        input.clone(),
    )?;
    Ok(input)
}
