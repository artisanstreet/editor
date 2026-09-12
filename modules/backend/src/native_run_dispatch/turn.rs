//! One turn's live consumption: ordered observation handling, interaction
//! delivery, assistant projection commits, and terminal settlement.
//!
//! `consume_turn` drives the accepted owner turn to its terminal close,
//! handing every provider observation through the durable S1b batch path and
//! every routed response through durable resolution. Steer handling lives in
//! `super::steer`; claim loading, launch, and binding live in `super::claim`.

use artisan_database::{
    AssistantChange, CompleteRun, InterruptRun, RecordRunUsage, Repository,
    ResolveInteractionOutcome, RunBatchScope, RunErrorCode, RunErrorMessage,
};
use artisan_domain::{
    AssistantBody, AssistantMessagePhase, EngineId, ItemId, Observation, ObservationId,
    ObservationSequence, PatchId, RespondApproval, RespondQuestion, Revision, RunId, UnixMillis,
};

use artisan_transport::CancelHandle;

use crate::{
    CommandOrigin, SystemCommandOrigin,
    engine_owner::interaction::InteractionTarget,
    engine_owner::observation::{
        EngineObservation, SubagentLifecycleRow, SubagentTranscriptRow, TerminalState, TextDelta,
        TextSnapshot, UsageObservation,
    },
    engine_owner::operation::{AcceptedTurn, EngineOperationError, TurnResult},
    run_interaction::{OwnedInteractionCommand, RunInteractionAck, RunInteractionEnvelope},
};

use super::assistant_commit::{
    ensure_assistant_item, flush_pending_deltas, replace_assistant_body, start_assistant_item,
};
use super::claim::drain_interactions;
use super::delta_coalescer::DeltaCoalescer;
use super::dispatch_support::{at_or_after, mint_item_id, mint_patch_id};
use super::interaction_intent::command_request_intent;
use super::observation_commit::{
    SubagentCommitCursor, commit_activity_observation, commit_subagent_observation,
};
use super::steer::handle_steer;
use super::text_projection::OrderedAssistantText;
use super::{
    CommitBatchRequest, INTERRUPTED_CODE, INTERRUPTED_MESSAGE, NativeRunDispatcherConfig,
    PROVIDER_FAILURE_CODE, PROVIDER_FAILURE_MESSAGE, commit_batch_with_retry,
};

fn copy_scope<'a>(scope: &RunBatchScope<'a>) -> RunBatchScope<'a> {
    RunBatchScope {
        claimed: scope.claimed,
        launched: scope.launched,
        bound: scope.bound,
        run_start_key: scope.run_start_key,
        credentials: scope.credentials,
        expected_launch_at: scope.expected_launch_at,
        expected_updated_at: scope.expected_updated_at,
    }
}

pub(super) struct TurnConsumptionContext<'a> {
    pub(super) repository: &'a Repository,
    pub(super) config: &'a NativeRunDispatcherConfig,
    pub(super) origin: &'a SystemCommandOrigin,
    pub(super) stop: &'a CancelHandle,
    pub(super) process_cancel: &'a CancelHandle,
    pub(super) run_cancel: &'a CancelHandle,
}

pub(super) struct TurnConsumptionState<'a> {
    pub(super) scope: RunBatchScope<'a>,
    pub(super) engine: EngineId,
    pub(super) assistant_item: Option<ItemId>,
    pub(super) assistant_revision: Revision,
    pub(super) assistant_parts: OrderedAssistantText,
    pub(super) assistant_body: String,
    /// Byte-exact append fragments waiting for one coalesced batch commit.
    pub(super) coalescer: DeltaCoalescer,
    pub(super) last_usage: Option<artisan_domain::RunUsageReport>,
    pub(super) streaming_speed: super::streaming_speed::StreamingSpeed,
    pub(super) batch_sequence: i64,
    pub(super) forced_interrupted: bool,
    pub(super) forced_cancelled: bool,
    pub(super) progress_uncertain: bool,
    pub(super) terminal: Option<TerminalState>,
}

impl<'a> TurnConsumptionState<'a> {
    pub(super) fn new(scope: RunBatchScope<'a>, engine: EngineId) -> Self {
        Self {
            scope,
            engine,
            assistant_item: None,
            assistant_revision: Revision::new(0),
            assistant_parts: OrderedAssistantText::default(),
            assistant_body: String::new(),
            coalescer: DeltaCoalescer::default(),
            last_usage: None,
            streaming_speed: super::streaming_speed::StreamingSpeed::default(),
            batch_sequence: 1,
            forced_interrupted: false,
            forced_cancelled: false,
            progress_uncertain: false,
            terminal: None,
        }
    }
}

pub(super) async fn consume_turn(
    context: TurnConsumptionContext<'_>,
    mut turn: crate::engine_owner::operation::AcceptedTurn,
    scope: RunBatchScope<'_>,
    engine: EngineId,
    inbox: Option<&mut tokio::sync::mpsc::Receiver<RunInteractionEnvelope>>,
) -> bool {
    let mut state = TurnConsumptionState::new(scope, engine);

    let mut cancel_signalled = false;
    let mut inbox = inbox;
    loop {
        if cancel_signalled {
            // Cancellation was already delivered to the turn: drain the
            // remaining observations to the driver's terminal close. The
            // fired cancel branches stay ready forever once cancelled, so
            // re-selecting them under `biased` would starve
            // `next_observation()` on a held stream that never emits a
            // terminal event. Queued responses are answered from the durable
            // fence below: the run is dying, so they settle as `wrong_run`
            // without storing anything.
            if let Some(receiver) = inbox.as_mut() {
                drain_interactions(receiver);
            }
            let observation = turn.next_observation().await;
            let Some(observation) = observation else {
                break;
            };
            handle_observation(&context, &mut state, &mut turn, observation).await;
        } else {
            let flush_deadline = state.coalescer.deadline();
            tokio::select! {
                biased;
                () = context.stop.wait() => {
                    cancel_signalled = true;
                    state.forced_interrupted = true;
                    turn.cancel();
                }
                () = context.process_cancel.wait() => {
                    cancel_signalled = true;
                    state.forced_interrupted = true;
                    turn.cancel();
                }
                () = context.run_cancel.wait() => {
                    cancel_signalled = true;
                    state.forced_cancelled = true;
                    turn.cancel();
                }
                () = async {
                    match flush_deadline {
                        Some(deadline) => tokio::time::sleep_until(deadline.into()).await,
                        None => std::future::pending().await,
                    }
                } => {
                    if !flush_pending_deltas(&context, &mut state, &mut turn).await {
                        cancel_signalled = true;
                    }
                }
                interaction = async {
                    match inbox.as_mut() {
                        Some(receiver) => receiver.recv().await,
                        // No routing registration: never resolve this branch
                        // so observations keep flowing.
                        None => std::future::pending().await,
                    }
                } => {
                    let Some(envelope) = interaction else {
                        // The inbox closed while its lease is still held,
                        // which the registry cannot produce; fuse the branch
                        // and keep consuming observations.
                        inbox = None;
                        continue;
                    };
                    handle_interaction(&context, &mut state, &mut turn, envelope).await;
                }
                observation = turn.next_observation() => {
                    let Some(observation) = observation else { break; };
                    handle_observation(&context, &mut state, &mut turn, observation).await;
                }
            }
        }
        if state.terminal.is_some() {
            break;
        }
    }
    // Persist the coalesced tail before ownership resolution and terminal
    // settlement: an abort, an owner failure, or a held stream must not drop
    // bytes that a per-delta commit would already have written.
    let _ = flush_pending_deltas(&context, &mut state, &mut turn).await;
    let owner_result = turn.finish().await;
    if is_unresolved_reap(&owner_result) {
        return true;
    }
    if state.progress_uncertain {
        // A turn whose durable commit path already failed must not be left
        // for late lease-expiry recovery: settle the known-incomplete turn
        // as interrupted now so the failed output is durably surfaced. If
        // the same unavailable database rejects this settlement too, the
        // recovery sweep still owns the run.
        settle_terminal(&context, state, TerminalState::Interrupted).await;
        return false;
    }
    if context.run_cancel.is_cancelled() {
        state.forced_cancelled = true;
    }
    let Some(terminal) = resolve_terminal(
        state.forced_interrupted,
        state.forced_cancelled,
        state.terminal,
        &owner_result,
    ) else {
        return false;
    };
    if !ensure_assistant_item(&context, &mut state).await {
        return false;
    }
    if let Some(report) = &state.last_usage {
        // Optional display metadata must never turn a successful response into
        // a failed run. The provider counters were already committed above.
        let _ = context
            .repository
            .record_run_streaming_speed(report, state.streaming_speed.rate())
            .await;
    }
    settle_terminal(&context, state, terminal).await;
    false
}

pub(super) fn mark_interrupted(
    state: &mut TurnConsumptionState<'_>,
    turn: &AcceptedTurn,
    progress_uncertain: bool,
) {
    state.forced_interrupted = true;
    state.progress_uncertain |= progress_uncertain;
    turn.cancel();
}

pub(super) async fn handle_observation(
    context: &TurnConsumptionContext<'_>,
    state: &mut TurnConsumptionState<'_>,
    turn: &mut AcceptedTurn,
    observation: EngineObservation,
) {
    match observation {
        EngineObservation::TextDelta(delta) => {
            if delta.run_id() == &state.scope.launched.run_id {
                state.streaming_speed.push(&delta);
            }
            handle_text_delta(context, state, turn, delta).await;
        }
        EngineObservation::TextSnapshot(snapshot) => {
            state.streaming_speed.end_interval();
            handle_text_snapshot(context, state, turn, snapshot).await;
        }
        EngineObservation::Usage(usage) => {
            handle_usage(context, state, turn, usage).await;
        }
        EngineObservation::Terminal(observation) => {
            state.terminal = Some(observation.state());
            if state.forced_interrupted || state.forced_cancelled {
                turn.cancel();
            }
        }
        EngineObservation::Subagent(row) => {
            handle_subagent_lifecycle_row(context, state, turn, row).await;
        }
        EngineObservation::SubagentTranscript(row) => {
            handle_subagent_transcript_row(context, state, turn, row).await;
        }
        EngineObservation::Activity(observation) => {
            state.streaming_speed.end_interval();
            handle_activity_observation(context, state, turn, observation).await;
        }
    }
}

/// Handles one routed mid-turn response: durable resolve, owner delivery,
/// and resolution-observation commit.
///
/// The resolve transaction is authoritative for the acknowledgement: replays
/// and misses settle from durable state with no second effect. Only an
/// applied decision reaches the accepted turn ledger and the S1b observation
/// commit, and neither disturbs control flow: answering never cancels,
/// interrupts, or steers the run, so a deny lands with no side effect while
/// the turn continues.
#[allow(clippy::too_many_lines)]
async fn handle_interaction(
    context: &TurnConsumptionContext<'_>,
    state: &mut TurnConsumptionState<'_>,
    turn: &mut AcceptedTurn,
    envelope: RunInteractionEnvelope,
) {
    let RunInteractionEnvelope {
        thread_id,
        run_id,
        command,
        respond,
    } = envelope;
    // The registry routed the exact pair, but the scope owns the fence:
    // never resolve for a run this turn does not own.
    if thread_id != state.scope.launched.thread_id || run_id != state.scope.launched.run_id {
        let _ = respond.send(RunInteractionAck::WrongRun);
        return;
    }
    sync_turn_ledger(context, turn, &run_id).await;
    let scope = artisan_database::ResolveScope {
        binding_version: state.scope.bound.binding_version,
        responded_at: if let Ok(instant) = context.origin.acceptance_instant() {
            instant
        } else {
            // No timestamp, no settlement: nothing was stored, so the
            // client retry stays safe.
            let _ = respond.send(RunInteractionAck::Unavailable);
            return;
        },
    };
    let outcome = match &command {
        OwnedInteractionCommand::RespondApproval {
            approval_id,
            approved,
            ..
        } => {
            let approval = RespondApproval::new(
                command.request_id().clone(),
                thread_id.clone(),
                run_id.clone(),
                approval_id.clone(),
                *approved,
            );
            context
                .repository
                .resolve_approval_response(&approval, &scope)
                .await
        }
        OwnedInteractionCommand::RespondQuestion {
            question_id,
            answers,
            ..
        } => {
            let Ok(question) = RespondQuestion::new(
                command.request_id().clone(),
                thread_id.clone(),
                run_id.clone(),
                question_id.clone(),
                answers.clone(),
            ) else {
                let _ = respond.send(RunInteractionAck::Unavailable);
                return;
            };
            context
                .repository
                .resolve_question_response(&question, &scope)
                .await
        }
        OwnedInteractionCommand::Steer {
            request_id,
            message_id,
            text,
        } => {
            // Steers complete through `handle_steer` (provider write +
            // projection + row completion under the original request id),
            // never through the approval/question resolve path.
            handle_steer(
                context,
                state,
                turn,
                thread_id,
                run_id,
                request_id.clone(),
                message_id.clone(),
                text.clone(),
                respond,
            )
            .await;
            return;
        }
    };
    let Ok(outcome) = outcome else {
        // A resolve failure leaves durability unknown, so the run fails
        // safe instead of presenting a stream as durably completed.
        mark_interrupted(state, turn, true);
        let _ = respond.send(RunInteractionAck::Unavailable);
        return;
    };
    match outcome {
        ResolveInteractionOutcome::WrongRun => {
            let _ = respond.send(RunInteractionAck::WrongRun);
        }
        ResolveInteractionOutcome::Conflict(_) => {
            let _ = respond.send(RunInteractionAck::Conflict);
        }
        ResolveInteractionOutcome::Duplicate(stored)
        | ResolveInteractionOutcome::UnknownTarget(stored)
        | ResolveInteractionOutcome::AlreadyResolved(stored) => {
            let _ = respond.send(RunInteractionAck::Settled(stored));
        }
        ResolveInteractionOutcome::Applied(applied) => {
            deliver_applied_response(context, state, turn, &command, applied, respond).await;
        }
    }
}

/// Seeds the accepted turn ledger from durable pending state.
///
/// Runs before the resolve transaction so the later delivery agrees with
/// what the transaction settles. A seeding failure leaves the ledger as-is:
/// the resolve transaction stays authoritative, and a delivery that then
/// disagrees fails the run safe in the caller.
async fn sync_turn_ledger(
    context: &TurnConsumptionContext<'_>,
    turn: &mut AcceptedTurn,
    run_id: &RunId,
) {
    let Ok(pending) = context.repository.pending_interactions(run_id).await else {
        return;
    };
    for view in pending.iter().filter(|view| view.requested) {
        turn.note_interaction_requested(
            &view.interaction_id,
            match view.kind {
                artisan_domain::InteractionKind::Approval => InteractionTarget::Approval,
                artisan_domain::InteractionKind::Question => InteractionTarget::Question,
            },
        );
    }
}

/// Delivers one applied decision to the accepted turn, commits its
/// resolution observation through the S1b checkpoint path, and acknowledges
/// the stored receipt.
///
/// Ledger disagreement after a committed resolve cannot happen under the
/// single-owner discipline, but if it does the run fails safe while the
/// acknowledgement still reports the durable truth.
async fn deliver_applied_response(
    context: &TurnConsumptionContext<'_>,
    state: &mut TurnConsumptionState<'_>,
    turn: &mut AcceptedTurn,
    command: &OwnedInteractionCommand,
    applied: artisan_database::AppliedInteraction,
    respond: tokio::sync::oneshot::Sender<RunInteractionAck>,
) {
    let (target_id, target, intent) = match command {
        OwnedInteractionCommand::RespondApproval { approval_id, .. } => (
            approval_id,
            InteractionTarget::Approval,
            command_request_intent(state, command),
        ),
        OwnedInteractionCommand::RespondQuestion { question_id, .. } => (
            question_id,
            InteractionTarget::Question,
            command_request_intent(state, command),
        ),
        OwnedInteractionCommand::Steer { .. } => {
            // Steers complete through `handle_steer`, never through this
            // resolve path. This arm is unreachable by construction; answer
            // transiently without disturbing the live turn.
            let _ = respond.send(RunInteractionAck::Unavailable);
            return;
        }
    };
    let Some(intent) = intent else {
        mark_interrupted(state, turn, true);
        let _ = respond.send(RunInteractionAck::Settled(applied.receipt));
        return;
    };
    if turn
        .deliver_interaction_response(
            applied.receipt.request_id.as_str(),
            target_id,
            target,
            &intent,
        )
        .is_err()
    {
        mark_interrupted(state, turn, true);
        let _ = respond.send(RunInteractionAck::Settled(applied.receipt));
        return;
    }
    if !commit_resolution_observation(context, state, turn, &applied).await {
        mark_interrupted(state, turn, true);
    }
    let _ = respond.send(RunInteractionAck::Settled(applied.receipt));
}

/// Commits one applied resolution as an S1b observation checkpoint batch.
///
/// Encodes the resolved observation under the run's binding with the
/// previous committed sequence as its base, then commits through the
/// existing batch path with a content-neutral assistant change: the body is
/// rewritten verbatim so subscribers receive the wake hint without any
/// transcript mutation. The batch advances the scope stamps exactly like a
/// text batch, so later commits keep fencing.
async fn commit_resolution_observation(
    context: &TurnConsumptionContext<'_>,
    state: &mut TurnConsumptionState<'_>,
    turn: &mut AcceptedTurn,
    applied: &artisan_database::AppliedInteraction,
) -> bool {
    let Ok(base) = context
        .repository
        .last_committed_observation_sequence(&state.scope.launched.run_id)
        .await
    else {
        return false;
    };
    let Ok(observation_id) = context.origin.mint_identity() else {
        return false;
    };
    let Ok(observation_id) = ObservationId::parse(observation_id) else {
        return false;
    };
    let Ok(resolved_sequence) = ObservationSequence::new(applied.resolved_sequence) else {
        return false;
    };
    let Some(resolved) = build_resolved_observation(applied, &observation_id, resolved_sequence)
    else {
        return false;
    };
    let Ok(checkpoint) = artisan_database::encode_observation_checkpoint(
        state.engine,
        state.scope.bound.binding_version,
        base,
        &[resolved],
    ) else {
        return false;
    };
    if artisan_database::validate_observation_bind(
        state.scope.bound.binding_version,
        state.scope.bound,
    )
    .is_err()
    {
        return false;
    }
    let Ok(body) = AssistantBody::parse(state.assistant_body.clone()) else {
        mark_interrupted(state, turn, false);
        return false;
    };
    let Some(patch_id) = mint_patch_id(context.origin) else {
        mark_interrupted(state, turn, false);
        return false;
    };
    let Some(operated_at) = at_or_after(context.origin, state.scope.expected_updated_at) else {
        mark_interrupted(state, turn, false);
        return false;
    };
    let committed = if let Some(item_id) = state.assistant_item.clone() {
        commit_resolution_replace(
            context,
            state,
            turn,
            checkpoint,
            operated_at,
            &item_id,
            &body,
            &patch_id,
        )
        .await
    } else {
        commit_resolution_start(
            context,
            state,
            turn,
            checkpoint,
            operated_at,
            &body,
            &patch_id,
        )
        .await
    };
    if committed {
        // The replace/start persisted the whole assembled body, including
        // any buffered append fragments, so the buffer is now redundant.
        state.coalescer.clear();
    }
    committed
}

/// Commits a resolution checkpoint beside a content-neutral body replace.
///
/// The body is rewritten verbatim at the next revision so subscribers
/// receive the wake hint without any transcript mutation.
#[expect(
    clippy::too_many_arguments,
    reason = "arguments mirror the CommitBatchRequest fields one-for-one; a wrapper struct would only rename them"
)]
async fn commit_resolution_replace(
    context: &TurnConsumptionContext<'_>,
    state: &mut TurnConsumptionState<'_>,
    turn: &mut AcceptedTurn,
    checkpoint: artisan_database::EngineCheckpoint,
    operated_at: UnixMillis,
    item_id: &ItemId,
    body: &AssistantBody,
    patch_id: &PatchId,
) -> bool {
    let changes = [AssistantChange::Replace {
        item_id,
        expected_revision: state.assistant_revision,
        body,
        phase: AssistantMessagePhase::Unspecified,
        patch_id,
    }];
    if commit_batch_with_retry(CommitBatchRequest {
        repository: context.repository,
        notifier: &context.config.notifier,
        scope: &state.scope,
        batch_sequence: state.batch_sequence,
        operated_at,
        activate_turn_patch_id: None,
        changes: &changes,
        checkpoint: artisan_database::CheckpointUpdate::Replace(&checkpoint),
        retries: context.config.max_command_retries,
    })
    .await
    .is_err()
    {
        return false;
    }
    let Ok(next_revision) = state.assistant_revision.checked_next() else {
        mark_interrupted(state, turn, true);
        return false;
    };
    state.assistant_revision = next_revision;
    state.scope.expected_updated_at = operated_at;
    let Some(next_sequence) = state.batch_sequence.checked_add(1) else {
        mark_interrupted(state, turn, true);
        return false;
    };
    state.batch_sequence = next_sequence;
    true
}

/// Commits a resolution checkpoint while opening the assistant item.
///
/// Used only when the response arrived before any text: the item opens with
/// the current (possibly empty) body exactly like the text path opens it,
/// including turn activation.
async fn commit_resolution_start(
    context: &TurnConsumptionContext<'_>,
    state: &mut TurnConsumptionState<'_>,
    turn: &mut AcceptedTurn,
    checkpoint: artisan_database::EngineCheckpoint,
    operated_at: UnixMillis,
    body: &AssistantBody,
    patch_id: &PatchId,
) -> bool {
    let Some(item_id) = mint_item_id(context.origin) else {
        mark_interrupted(state, turn, false);
        return false;
    };
    let Some(activation_patch_id) = mint_patch_id(context.origin) else {
        mark_interrupted(state, turn, false);
        return false;
    };
    let changes = [AssistantChange::Start {
        item_id: &item_id,
        phase: AssistantMessagePhase::Unspecified,
        body,
        patch_id,
    }];
    if commit_batch_with_retry(CommitBatchRequest {
        repository: context.repository,
        notifier: &context.config.notifier,
        scope: &state.scope,
        batch_sequence: state.batch_sequence,
        operated_at,
        activate_turn_patch_id: Some(&activation_patch_id),
        changes: &changes,
        checkpoint: artisan_database::CheckpointUpdate::Replace(&checkpoint),
        retries: context.config.max_command_retries,
    })
    .await
    .is_err()
    {
        return false;
    }
    state.assistant_item = Some(item_id);
    state.assistant_revision = Revision::new(0);
    state.scope.expected_updated_at = operated_at;
    let Some(next_sequence) = state.batch_sequence.checked_add(1) else {
        mark_interrupted(state, turn, true);
        return false;
    };
    state.batch_sequence = next_sequence;
    true
}

/// Builds the resolved domain observation for one applied decision.
///
/// The observation carries the full stored request, so the checkpoint batch
/// preserves the complete history even though only the resolution commits.
fn build_resolved_observation(
    applied: &artisan_database::AppliedInteraction,
    observation_id: &ObservationId,
    sequence: ObservationSequence,
) -> Option<artisan_domain::Observation> {
    if let Some(approval) = applied.requested.approval.as_ref() {
        let approved = applied.receipt.approved?;
        return artisan_domain::ApprovalObservation::resolved(
            observation_id.clone(),
            sequence,
            approval.approval_id.clone(),
            approval.description.clone(),
            approval.request.clone(),
            approved,
        )
        .ok()
        .map(artisan_domain::Observation::Approval);
    }
    if let Some(question) = applied.requested.question.as_ref() {
        return artisan_domain::QuestionObservation::resolved(
            observation_id.clone(),
            sequence,
            question.input.clone(),
            applied.receipt.answers.clone(),
        )
        .ok()
        .map(artisan_domain::Observation::Question);
    }
    None
}

/// Commits one subagent lifecycle row as an S1b observation checkpoint batch.
async fn handle_subagent_lifecycle_row(
    context: &TurnConsumptionContext<'_>,
    state: &mut TurnConsumptionState<'_>,
    turn: &mut AcceptedTurn,
    row: SubagentLifecycleRow,
) {
    handle_subagent_row(
        context,
        state,
        turn,
        Observation::Subagent(row.into_observation()),
    )
    .await;
}

/// Commits one subagent transcript row as an S1b observation checkpoint batch.
async fn handle_subagent_transcript_row(
    context: &TurnConsumptionContext<'_>,
    state: &mut TurnConsumptionState<'_>,
    turn: &mut AcceptedTurn,
    row: SubagentTranscriptRow,
) {
    handle_subagent_row(
        context,
        state,
        turn,
        Observation::SubagentTranscript(row.into_observation()),
    )
    .await;
}

/// Commits one subagent row through the shared S1b checkpoint batch path.
///
/// The row is re-sequenced onto the durable chain freshly read for this
/// batch, encoded under the run bind, and committed with a content-neutral
/// assistant change so the body is rewritten verbatim: subscribers receive
/// the wake hint without any transcript mutation. Sequencing, stamps, and
/// the assistant projection advance exactly like a text batch, so later
/// commits keep fencing. Any failure marks the turn interrupted with
/// uncertain progress: a valid stream must not present as durably completed
/// when its subagent rows did not persist.
async fn handle_subagent_row(
    context: &TurnConsumptionContext<'_>,
    state: &mut TurnConsumptionState<'_>,
    turn: &mut AcceptedTurn,
    observation: Observation,
) {
    let mut cursor = SubagentCommitCursor {
        scope: copy_scope(&state.scope),
        engine: state.engine,
        batch_sequence: state.batch_sequence,
        assistant_item: state.assistant_item.clone(),
        assistant_revision: state.assistant_revision,
        assistant_body: state.assistant_body.clone(),
    };
    if !commit_subagent_observation(
        context.repository,
        context.config,
        context.origin,
        &mut cursor,
        observation,
    )
    .await
    {
        mark_interrupted(state, turn, true);
        return;
    }
    let updated_at = cursor.scope.expected_updated_at;
    let revision = cursor.assistant_revision;
    let sequence = cursor.batch_sequence;
    let item = cursor.assistant_item.clone();
    let body = cursor.assistant_body.clone();
    state.scope.expected_updated_at = updated_at;
    state.assistant_revision = revision;
    state.batch_sequence = sequence;
    state.assistant_item = item;
    state.assistant_body = body;
    // The content-neutral replace persisted the whole assembled body,
    // including any buffered append fragments.
    state.coalescer.clear();
}

/// Commits one validated rich activity row through the shared S1b checkpoint
/// batch path.
///
/// The row carries source-local durable identity/sequence from the owner
/// stream; the dispatcher remints both (fresh identity, run-local
/// base-plus-one sequence freshly read for this batch) before persistence, so
/// the thread-scoped `delivery_sequence` attribution assigned by the database
/// stays strictly increasing across runs. Encoding, fencing,
/// `commit_batch_with_retry`, and the content-neutral assistant projection
/// are identical to the subagent path: the body is rewritten verbatim so
/// subscribers receive the wake hint without any transcript mutation. Any
/// failure marks the turn interrupted with uncertain progress; no row is
/// discarded and no no-op commit is emitted.
async fn handle_activity_observation(
    context: &TurnConsumptionContext<'_>,
    state: &mut TurnConsumptionState<'_>,
    turn: &mut AcceptedTurn,
    observation: Observation,
) {
    let mut cursor = SubagentCommitCursor {
        scope: copy_scope(&state.scope),
        engine: state.engine,
        batch_sequence: state.batch_sequence,
        assistant_item: state.assistant_item.clone(),
        assistant_revision: state.assistant_revision,
        assistant_body: state.assistant_body.clone(),
    };
    if !commit_activity_observation(
        context.repository,
        context.config,
        context.origin,
        &mut cursor,
        observation,
    )
    .await
    {
        mark_interrupted(state, turn, true);
        return;
    }
    let updated_at = cursor.scope.expected_updated_at;
    let revision = cursor.assistant_revision;
    let sequence = cursor.batch_sequence;
    let item = cursor.assistant_item.clone();
    let body = cursor.assistant_body.clone();
    state.scope.expected_updated_at = updated_at;
    state.assistant_revision = revision;
    state.batch_sequence = sequence;
    state.assistant_item = item;
    state.assistant_body = body;
    // The content-neutral replace persisted the whole assembled body,
    // including any buffered append fragments.
    state.coalescer.clear();
}

async fn handle_usage(
    context: &TurnConsumptionContext<'_>,
    state: &mut TurnConsumptionState<'_>,
    turn: &mut AcceptedTurn,
    usage: UsageObservation,
) {
    let report = usage.report();
    state.last_usage = Some(report.clone());
    if report.run_id() != &state.scope.launched.run_id
        || report.thread_id() != &state.scope.launched.thread_id
        || context
            .repository
            .record_run_usage(RecordRunUsage {
                run_id: &state.scope.launched.run_id,
                thread_id: &state.scope.launched.thread_id,
                report,
            })
            .await
            .is_err()
    {
        // A valid text stream must not be presented as durably completed when
        // its authenticated usage observation could not be fenced/persisted.
        mark_interrupted(state, turn, true);
    }
}

async fn handle_text_snapshot(
    context: &TurnConsumptionContext<'_>,
    state: &mut TurnConsumptionState<'_>,
    turn: &mut AcceptedTurn,
    snapshot: TextSnapshot,
) {
    if snapshot.run_id() != &state.scope.launched.run_id {
        mark_interrupted(state, turn, true);
        return;
    }
    // A snapshot rewrites the assembled body, so the buffered append run must
    // reach durable state first; otherwise the snapshot's replace would
    // silently drop the uncommitted suffix from the append history.
    if !flush_pending_deltas(context, state, turn).await {
        return;
    }
    let current = state.assistant_body.clone();
    let Some(next_body) = state.assistant_parts.replace_snapshot(&snapshot) else {
        mark_interrupted(state, turn, true);
        return;
    };
    if next_body == current {
        return;
    }
    state.streaming_speed = super::streaming_speed::StreamingSpeed::default();
    state.assistant_body = next_body;
    if state.assistant_item.is_none() {
        // An ended-only part is a first durable body, not a text delta. Start
        // it through the normal run-scoped item-upsert seam without replaying
        // an artificial provider append.
        start_assistant_item(context, state, turn).await;
    } else {
        replace_assistant_body(context, state, turn).await;
    }
}

async fn handle_text_delta(
    context: &TurnConsumptionContext<'_>,
    state: &mut TurnConsumptionState<'_>,
    turn: &mut AcceptedTurn,
    delta: TextDelta,
) {
    if delta.run_id() != &state.scope.launched.run_id || delta.delta().is_empty() {
        if delta.run_id() != &state.scope.launched.run_id {
            mark_interrupted(state, turn, true);
        }
        return;
    }
    let Some(appended) = state.assistant_parts.append_delta(&delta) else {
        mark_interrupted(state, turn, true);
        return;
    };
    let appended_exactly = appended.appended_exactly;
    state.assistant_body = appended.body;
    if !appended_exactly {
        // A separator entered the assembled body (a new provider part), so
        // the delta is not a byte-exact append: persist the whole body as one
        // replacement, which also covers any buffered fragments.
        if state.assistant_item.is_none() {
            start_assistant_item(context, state, turn).await;
        } else {
            replace_assistant_body(context, state, turn).await;
        }
        return;
    }
    if state.assistant_item.is_none() {
        // The first durable body commits on arrival: a held stream's first
        // bytes must be readable without waiting for a coalescing threshold.
        start_assistant_item(context, state, turn).await;
        return;
    }
    // Byte-exact appends coalesce in memory. The buffered run commits on a
    // structural byte/count threshold, at every full-body persistence path,
    // and at turn end, so subscribers still observe every byte in order.
    if state.coalescer.would_overflow(delta.delta())
        && !flush_pending_deltas(context, state, turn).await
    {
        return;
    }
    if state.coalescer.push(delta.delta()) {
        let _ = flush_pending_deltas(context, state, turn).await;
    }
}

fn resolve_terminal(
    forced_interrupted: bool,
    forced_cancelled: bool,
    terminal: Option<TerminalState>,
    owner_result: &TurnResult,
) -> Option<TerminalState> {
    if forced_interrupted {
        return Some(TerminalState::Interrupted);
    }
    if forced_cancelled {
        return Some(TerminalState::Cancelled);
    }
    if let Some(terminal) = terminal {
        return Some(terminal);
    }
    match owner_result {
        Ok(result) => Some(result.terminal()),
        Err(EngineOperationError::Cancelled) => Some(TerminalState::Cancelled),
        Err(EngineOperationError::Shutdown | EngineOperationError::Deadline) => {
            Some(TerminalState::Interrupted)
        }
        Err(
            EngineOperationError::ReapUnresolved
            | EngineOperationError::UnresolvedReapDuring { .. },
        ) => None,
        Err(_) => Some(TerminalState::Failed),
    }
}

pub(super) fn is_unresolved_reap(result: &TurnResult) -> bool {
    matches!(
        result,
        Err(EngineOperationError::ReapUnresolved
            | EngineOperationError::UnresolvedReapDuring { .. })
    )
}

struct TerminalSettlement<'a> {
    repository: &'a Repository,
    scope: &'a RunBatchScope<'a>,
    retries: std::num::NonZeroUsize,
    operated_at: UnixMillis,
    item_id: &'a ItemId,
    expected_revision: Revision,
    body: &'a AssistantBody,
    phase: AssistantMessagePhase,
    item_patch_id: &'a PatchId,
    turn_patch_id: &'a PatchId,
}

async fn settle_terminal(
    context: &TurnConsumptionContext<'_>,
    state: TurnConsumptionState<'_>,
    terminal: TerminalState,
) {
    // Pending rows are per-run: wipe them at settle so decisions never leak
    // across runs. Best-effort beside terminal settlement; idempotent.
    let _ = context
        .repository
        .settle_run_interactions(&state.scope.launched.run_id)
        .await;
    let TurnConsumptionState {
        scope,
        assistant_item: Some(item_id),
        assistant_revision,
        assistant_body,
        ..
    } = state
    else {
        return;
    };
    let Ok(body) = AssistantBody::parse(assistant_body) else {
        return;
    };
    let Some(item_patch_id) = mint_patch_id(context.origin) else {
        return;
    };
    let Some(turn_patch_id) = mint_patch_id(context.origin) else {
        return;
    };
    let Some(operated_at) = at_or_after(context.origin, scope.expected_updated_at) else {
        return;
    };
    let phase = if matches!(terminal, TerminalState::Completed) {
        AssistantMessagePhase::Final
    } else {
        AssistantMessagePhase::Unspecified
    };
    let settlement = TerminalSettlement {
        repository: context.repository,
        scope: &scope,
        retries: context.config.max_command_retries,
        operated_at,
        item_id: &item_id,
        expected_revision: assistant_revision,
        body: &body,
        phase,
        item_patch_id: &item_patch_id,
        turn_patch_id: &turn_patch_id,
    };
    if persist_terminal_settlement(&settlement, terminal).await {
        let _ = context.config.notifier.publish(&scope.launched.thread_id);
    }
}

async fn persist_terminal_settlement(
    settlement: &TerminalSettlement<'_>,
    terminal: TerminalState,
) -> bool {
    match terminal {
        TerminalState::Completed => persist_completed(settlement).await,
        TerminalState::Failed => persist_failed(settlement).await,
        TerminalState::Cancelled => persist_cancelled(settlement).await,
        TerminalState::Interrupted => persist_interrupted(settlement).await,
    }
}

async fn persist_completed(settlement: &TerminalSettlement<'_>) -> bool {
    for _ in 0..settlement.retries.get() {
        if settlement
            .repository
            .complete_run(CompleteRun {
                scope: copy_scope(settlement.scope),
                operated_at: settlement.operated_at,
                item_id: settlement.item_id,
                expected_revision: settlement.expected_revision,
                body: settlement.body,
                phase: settlement.phase,
                item_patch_id: settlement.item_patch_id,
                turn_patch_id: settlement.turn_patch_id,
            })
            .await
            .is_ok()
        {
            return true;
        }
    }
    false
}

async fn persist_failed(settlement: &TerminalSettlement<'_>) -> bool {
    let Ok(error_code) = RunErrorCode::parse(PROVIDER_FAILURE_CODE.to_owned()) else {
        return false;
    };
    let Ok(error_message) = RunErrorMessage::parse(PROVIDER_FAILURE_MESSAGE.to_owned()) else {
        return false;
    };
    for _ in 0..settlement.retries.get() {
        if settlement
            .repository
            .fail_run(artisan_database::FailRun {
                scope: copy_scope(settlement.scope),
                operated_at: settlement.operated_at,
                item_id: settlement.item_id,
                expected_revision: settlement.expected_revision,
                body: settlement.body,
                phase: settlement.phase,
                item_patch_id: settlement.item_patch_id,
                turn_patch_id: settlement.turn_patch_id,
                error_code: &error_code,
                error_message: &error_message,
            })
            .await
            .is_ok()
        {
            return true;
        }
    }
    false
}

async fn persist_cancelled(settlement: &TerminalSettlement<'_>) -> bool {
    for _ in 0..settlement.retries.get() {
        if settlement
            .repository
            .cancel_run(artisan_database::CancelRun {
                scope: copy_scope(settlement.scope),
                operated_at: settlement.operated_at,
                item_id: settlement.item_id,
                expected_revision: settlement.expected_revision,
                body: settlement.body,
                phase: settlement.phase,
                item_patch_id: settlement.item_patch_id,
                turn_patch_id: settlement.turn_patch_id,
            })
            .await
            .is_ok()
        {
            return true;
        }
    }
    false
}

async fn persist_interrupted(settlement: &TerminalSettlement<'_>) -> bool {
    let Ok(error_code) = RunErrorCode::parse(INTERRUPTED_CODE.to_owned()) else {
        return false;
    };
    let Ok(error_message) = RunErrorMessage::parse(INTERRUPTED_MESSAGE.to_owned()) else {
        return false;
    };
    for _ in 0..settlement.retries.get() {
        if settlement
            .repository
            .interrupt_run(InterruptRun {
                scope: copy_scope(settlement.scope),
                operated_at: settlement.operated_at,
                item_id: settlement.item_id,
                expected_revision: settlement.expected_revision,
                body: settlement.body,
                phase: settlement.phase,
                item_patch_id: settlement.item_patch_id,
                turn_patch_id: settlement.turn_patch_id,
                error_code: &error_code,
                error_message: &error_message,
            })
            .await
            .is_ok()
        {
            return true;
        }
    }
    false
}
