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
    AssistantBody, AssistantMessagePhase, EngineId, ErrorChain, InteractionKind, ItemId,
    Observation, ObservationId, ObservationSequence, PatchId, RequestId, RespondApproval,
    RespondQuestion, Revision, RunId, UnixMillis,
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
use super::diagnostics::{RunLabel, StepError, report_interrupted, report_step_failure};
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
    pub(super) assistant_phase: AssistantMessagePhase,
    pub(super) active_part: Option<String>,
    pub(super) last_part: Option<String>,
    pub(super) parked_parts:
        std::collections::BTreeMap<String, super::message_parts::MessageCursor>,
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
            assistant_phase: AssistantMessagePhase::Unspecified,
            active_part: None,
            last_part: None,
            parked_parts: std::collections::BTreeMap::new(),
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
                    mark_interrupted(&mut state, &turn, false, &DISPATCHER_STOPPED);
                }
                () = context.process_cancel.wait() => {
                    cancel_signalled = true;
                    mark_interrupted(&mut state, &turn, false, &FORGE_SHUTTING_DOWN);
                }
                () = context.run_cancel.wait() => {
                    cancel_signalled = true;
                    mark_cancelled(&mut state, &turn);
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
        if state.progress_uncertain && !cancel_signalled {
            cancel_signalled = true;
            turn.cancel();
        }
    }
    forget_live_thinking(&context, &state);
    persist_tail(&context, &mut state, &mut turn).await;
    let owner_result = turn.finish().await;
    report_engine_turn_failure(&state, &owner_result);
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
    if let Err(error) = ensure_assistant_item(&context, &mut state).await {
        report_unsettled(&state, terminal, &error);
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

/// Persists the coalesced tail before ownership resolution and terminal
/// settlement: an abort, an owner failure, or a held stream must not drop
/// bytes that a per-delta commit would already have written.
async fn persist_tail(
    context: &TurnConsumptionContext<'_>,
    state: &mut TurnConsumptionState<'_>,
    turn: &mut AcceptedTurn,
) {
    if !state.progress_uncertain {
        let _ = flush_pending_deltas(context, state, turn).await;
    }
    if !state.progress_uncertain {
        let _ = super::message_parts::finish_history(context, state, turn).await;
    }
    close_open_questions(context, state).await;
}

/// The dispatcher's own stop reached a live turn.
const DISPATCHER_STOPPED: StepError =
    StepError::refused("the run dispatcher was stopped while the turn was live");

/// The Forge process began shutting down under a live turn.
const FORGE_SHUTTING_DOWN: StepError =
    StepError::refused("the Forge process is shutting down while the turn was live");

/// Ends the live turn as cancelled on the user's request.
fn mark_cancelled(state: &mut TurnConsumptionState<'_>, turn: &AcceptedTurn) {
    eprintln!(
        "native run cancelled on request ({}, engine {:?}, batch {})",
        RunLabel(&state.scope),
        state.engine,
        state.batch_sequence
    );
    state.forced_cancelled = true;
    turn.cancel();
}

/// Reports an engine turn that ended with a typed error other than the
/// caller's own cancellation.
///
/// Engine errors are payload-free by design: the typed cause chain is the
/// whole diagnosis the owner exposes.
fn report_engine_turn_failure(state: &TurnConsumptionState<'_>, result: &TurnResult) {
    let Err(error) = result else {
        return;
    };
    if matches!(error, EngineOperationError::Cancelled) {
        return;
    }
    eprintln!(
        "native run engine turn failed ({}, engine {:?}, batch {}, observed terminal {:?}): {}",
        RunLabel(&state.scope),
        state.engine,
        state.batch_sequence,
        state.terminal,
        ErrorChain(error)
    );
}

/// Reports a run whose terminal state could not be prepared for storage.
/// The run stays unsettled; lease recovery owns it from then on.
fn report_unsettled(state: &TurnConsumptionState<'_>, terminal: TerminalState, error: &StepError) {
    eprintln!(
        "native run could not be settled as {terminal:?} ({}, engine {:?}): {}",
        RunLabel(&state.scope),
        state.engine,
        ErrorChain(error)
    );
}

/// Ends the live turn as interrupted because `error`'s step failed.
///
/// Every call is reported with the run, the failed step, and the whole cause
/// chain: the run's stored outcome is the fixed `provider_interrupted`, so
/// this line is the only place the actual reason is written down.
pub(super) fn mark_interrupted(
    state: &mut TurnConsumptionState<'_>,
    turn: &AcceptedTurn,
    progress_uncertain: bool,
    error: &StepError,
) {
    report_interrupted(
        &state.scope,
        state.engine,
        state.batch_sequence,
        progress_uncertain,
        error,
    );
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
    // Once a commit loses its fence, drain only to release provider custody.
    // Further observations cannot be committed with the obsolete scope.
    if state.progress_uncertain {
        if let EngineObservation::Terminal(observation) = observation
            && observation.run_id() == &state.scope.launched.run_id
        {
            state.terminal = Some(observation.state());
        }
        return;
    }
    match observation {
        EngineObservation::SummaryTitle { run_id, title } => {
            if run_id == state.scope.launched.run_id {
                let _ = context
                    .repository
                    .record_generated_thread_title(&state.scope.launched.thread_id, &title)
                    .await;
                let _ = context
                    .config
                    .notifier
                    .publish(&state.scope.launched.thread_id);
            }
        }
        EngineObservation::TextDelta(delta) => {
            if delta.run_id() == &state.scope.launched.run_id {
                state.streaming_speed.push(&delta);
            }
            forget_live_thinking(context, state);
            handle_text_delta(context, state, turn, delta).await;
        }
        EngineObservation::TextSnapshot(snapshot) => {
            state.streaming_speed.end_interval();
            forget_live_thinking(context, state);
            handle_text_snapshot(context, state, turn, snapshot).await;
        }
        EngineObservation::Usage(usage) => {
            handle_usage(context, state, turn, usage).await;
        }
        EngineObservation::Terminal(observation) => {
            if observation.run_id() != &state.scope.launched.run_id {
                return;
            }
            if let Some(title) = observation.summary_title()
                && let Ok(title) = artisan_domain::ThreadTitle::parse(title.to_owned())
            {
                let _ = context
                    .repository
                    .record_generated_thread_title(&state.scope.launched.thread_id, &title)
                    .await;
            }
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
            if note_live_thinking(context, state, &observation) {
                return;
            }
            handle_activity_observation(context, state, turn, observation).await;
        }
    }
}

/// Keeps a thinking summary in memory instead of committing it, and returns
/// whether `observation` was one.
///
/// A summary is shown only while it is the newest thing the run produced, so
/// it is never written to the observation ledger: the current block lives on
/// the dispatcher's board and subscribers are woken to read it. Any other
/// visible activity means the run moved on, which drops the block.
fn note_live_thinking(
    context: &TurnConsumptionContext<'_>,
    state: &TurnConsumptionState<'_>,
    observation: &Observation,
) -> bool {
    let launched = state.scope.launched;
    let board = &context.config.live_thinking;
    let source = |item_id| crate::live_thinking::ThinkingSource {
        thread_id: &launched.thread_id,
        run_id: &launched.run_id,
        turn_id: &launched.turn_id,
        item_id,
        at: at_or_after(context.origin, state.scope.expected_updated_at)
            .unwrap_or(state.scope.expected_updated_at),
    };
    match observation {
        Observation::ReasoningSummaryDelta(row) => {
            board.append(&source(row.item_id().as_str()), row.delta());
        }
        Observation::ReasoningSummaryCompleted(row) => {
            board.complete(&source(row.item_id().as_str()), row.text());
        }
        // Bookkeeping rows are not work a reader sees, so a block that is
        // still streaming survives them whole.
        Observation::ProcessDiagnostic(_)
        | Observation::ProtocolDiagnostic(_)
        | Observation::RunState(_)
        | Observation::TurnState(_)
        | Observation::Usage(_) => return false,
        _ => {
            forget_live_thinking(context, state);
            return false;
        }
    }
    let _ = context.config.notifier.publish(&launched.thread_id);
    true
}

/// Drops the run's thinking block once it is no longer the newest thing the
/// run produced, and wakes subscribers when one was showing.
fn forget_live_thinking(context: &TurnConsumptionContext<'_>, state: &TurnConsumptionState<'_>) {
    let launched = state.scope.launched;
    if context
        .config
        .live_thinking
        .clear(&launched.thread_id, &launched.run_id)
    {
        let _ = context.config.notifier.publish(&launched.thread_id);
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
        mut command,
        respond,
    } = envelope;
    // The registry routed the exact pair, but the scope owns the fence:
    // never resolve for a run this turn does not own.
    if thread_id != state.scope.launched.thread_id || run_id != state.scope.launched.run_id {
        let _ = respond.send(RunInteractionAck::WrongRun);
        return;
    }
    sync_turn_ledger(context, state, turn, &run_id).await;
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
    let outcome = match &mut command {
        OwnedInteractionCommand::RespondApproval {
            request_id,
            approval_id,
            approved,
        } => {
            let approval = RespondApproval::new(
                request_id.clone(),
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
            request_id,
            question_id,
            answers,
        } => {
            let Ok(question) = RespondQuestion::new(
                request_id.clone(),
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
            images,
        } => {
            // Steers complete through `handle_steer` (provider write +
            // projection + row completion under the original request id),
            // never through the approval/question resolve path. The steer
            // returns here, so its payload moves out instead of cloning
            // image bytes.
            handle_steer(
                context,
                state,
                turn,
                thread_id,
                run_id,
                request_id.clone(),
                message_id.clone(),
                std::mem::take(text),
                std::mem::take(images),
                respond,
            )
            .await;
            return;
        }
    };
    let outcome = match outcome {
        Ok(outcome) => outcome,
        Err(error) => {
            // A resolve failure leaves durability unknown, so the run fails
            // safe instead of presenting a stream as durably completed.
            mark_interrupted(
                state,
                turn,
                true,
                &StepError::failed("storing the answer to an approval or question", error),
            );
            let _ = respond.send(RunInteractionAck::Unavailable);
            return;
        }
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

/// Closes every question the run still holds open as it ends.
///
/// A question lives exactly as long as the provider request that asked it:
/// once the run is over nothing can answer it, so each open question
/// resolves as skipped through the same durable path an answer takes and
/// its resolution row reaches the thread. Best-effort beside terminal
/// settlement: a question this fails to close stays open on the thread and
/// answers as "no longer open" later.
async fn close_open_questions(
    context: &TurnConsumptionContext<'_>,
    state: &mut TurnConsumptionState<'_>,
) {
    // A turn whose durable commit path already failed commits nothing more.
    if state.progress_uncertain {
        return;
    }
    let run_id = state.scope.launched.run_id.clone();
    let pending = match context.repository.pending_interactions(&run_id).await {
        Ok(pending) => pending,
        Err(error) => {
            report_step_failure(
                &state.scope,
                &StepError::failed(
                    "reading the run's open questions to close them at turn end",
                    error,
                ),
            );
            return;
        }
    };
    let open: Vec<ObservationId> = pending
        .into_iter()
        .filter(|view| view.requested && view.kind == InteractionKind::Question)
        .map(|view| view.interaction_id)
        .collect();
    for question_id in open {
        if let Err(error) = close_open_question(context, state, &run_id, question_id).await {
            // The question stays open on the run; settlement wipes the
            // run's pending rows either way.
            report_step_failure(&state.scope, &error);
            return;
        }
    }
}

/// Answers one still-open question as skipped and commits that resolution.
async fn close_open_question(
    context: &TurnConsumptionContext<'_>,
    state: &mut TurnConsumptionState<'_>,
    run_id: &RunId,
    question_id: ObservationId,
) -> Result<(), StepError> {
    let minted = context.origin.mint_identity().map_err(|error| {
        StepError::failed("minting the request id that skips an open question", error)
    })?;
    let request_id = RequestId::parse(minted).map_err(|error| {
        StepError::failed(
            "validating the request id that skips an open question",
            error,
        )
    })?;
    let skipped = RespondQuestion::new(
        request_id,
        state.scope.launched.thread_id.clone(),
        run_id.clone(),
        question_id,
        Vec::new(),
    )
    .map_err(|error| {
        StepError::failed("building the skipped answer for an open question", error)
    })?;
    let responded_at = context
        .origin
        .acceptance_instant()
        .map_err(|error| StepError::failed("reading the clock to skip an open question", error))?;
    let scope = artisan_database::ResolveScope {
        binding_version: state.scope.bound.binding_version,
        responded_at,
    };
    let outcome = context
        .repository
        .resolve_question_response(&skipped, &scope)
        .await
        .map_err(|error| {
            StepError::failed("storing the skipped answer for an open question", error)
        })?;
    if let ResolveInteractionOutcome::Applied(applied) = outcome {
        commit_resolution_observation(context, state, &applied).await?;
    }
    Ok(())
}

/// Seeds the accepted turn ledger from durable pending state.
///
/// Runs before the resolve transaction so the later delivery agrees with
/// what the transaction settles. A seeding failure leaves the ledger as-is:
/// the resolve transaction stays authoritative, and a delivery that then
/// disagrees fails the run safe in the caller.
async fn sync_turn_ledger(
    context: &TurnConsumptionContext<'_>,
    state: &TurnConsumptionState<'_>,
    turn: &mut AcceptedTurn,
    run_id: &RunId,
) {
    let pending = match context.repository.pending_interactions(run_id).await {
        Ok(pending) => pending,
        Err(error) => {
            report_step_failure(
                &state.scope,
                &StepError::failed(
                    "reading the run's pending approvals and questions before an answer",
                    error,
                ),
            );
            return;
        }
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
        mark_interrupted(
            state,
            turn,
            true,
            &StepError::refused(
                "deriving the stored answer's intent key: the answer is already durable but cannot be handed to the engine",
            ),
        );
        let _ = respond.send(RunInteractionAck::Settled(applied.receipt));
        return;
    };
    if let Err(error) = turn.deliver_interaction_response(
        applied.receipt.request_id.as_str(),
        target_id,
        target,
        &intent,
    ) {
        mark_interrupted(
            state,
            turn,
            true,
            &StepError::failed("delivering the stored answer to the engine turn", error),
        );
        let _ = respond.send(RunInteractionAck::Settled(applied.receipt));
        return;
    }
    // Engines whose pump holds the provider request open hand the decision
    // back to it: the approval reply, or the question answer the pump
    // accumulates until its request is whole. A failed handoff fails the
    // run safe rather than leaving the provider waiting forever.
    if matches!(state.engine, EngineId::Codex | EngineId::Claude) {
        let request_id = applied.receipt.request_id.as_str();
        let handed = match command {
            OwnedInteractionCommand::RespondApproval {
                approval_id,
                approved,
                ..
            } => {
                let pending =
                    turn.answer_provider_approval(request_id, approval_id.as_str(), *approved);
                super::steer::drive_steer_ack(context, state, turn, pending).await
            }
            OwnedInteractionCommand::RespondQuestion {
                question_id,
                answers,
                ..
            } => {
                let pending =
                    turn.answer_provider_question(request_id, question_id.as_str(), answers);
                super::steer::drive_steer_ack(context, state, turn, pending).await
            }
            OwnedInteractionCommand::Steer { .. } => super::steer::SteerDriveOutcome::Acked(Ok(())),
        };
        let refusal = match handed {
            super::steer::SteerDriveOutcome::Acked(Ok(())) => None,
            super::steer::SteerDriveOutcome::Acked(Err(error)) => Some(StepError::failed(
                "handing the answer back to the provider request",
                error,
            )),
            super::steer::SteerDriveOutcome::Cancelled => Some(StepError::refused(
                "handing the answer back to the provider request: the run was cancelled while waiting for the provider's acknowledgement",
            )),
            super::steer::SteerDriveOutcome::TurnEnded => Some(StepError::refused(
                "handing the answer back to the provider request: the turn ended before the provider acknowledged it",
            )),
        };
        if let Some(refusal) = refusal {
            mark_interrupted(state, turn, true, &refusal);
            let _ = respond.send(RunInteractionAck::Settled(applied.receipt));
            return;
        }
    }
    if let Err(error) = commit_resolution_observation(context, state, &applied).await {
        mark_interrupted(state, turn, true, &error);
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
    applied: &artisan_database::AppliedInteraction,
) -> Result<(), StepError> {
    let base = context
        .repository
        .last_committed_observation_sequence(&state.scope.launched.run_id)
        .await
        .map_err(|error| {
            StepError::failed("reading the last committed observation sequence", error)
        })?;
    let observation_id = context.origin.mint_identity().map_err(|error| {
        StepError::failed("minting the resolution's observation identity", error)
    })?;
    let observation_id = ObservationId::parse(observation_id)
        .map_err(|error| StepError::failed("validating the minted observation identity", error))?;
    let checkpoint = resolution_checkpoint(
        state.engine,
        state.scope.bound.binding_version,
        base,
        applied,
        &observation_id,
    )
    .ok_or(StepError::refused(
        "encoding the resolved answer as an observation checkpoint: the stored request carries neither an approval nor a question, or its fields no longer validate",
    ))?;
    artisan_database::validate_observation_bind(
        state.scope.bound.binding_version,
        state.scope.bound,
    )
    .map_err(|error| StepError::failed("validating the run's observation binding", error))?;
    let body = AssistantBody::parse(state.assistant_body.clone())
        .map_err(|error| StepError::failed("validating the assembled assistant body", error))?;
    let patch_id = mint_patch_id(context.origin).ok_or(StepError::refused(
        "minting the item patch id: entropy or identifier validation failed",
    ))?;
    let operated_at = at_or_after(context.origin, state.scope.expected_updated_at).ok_or(
        StepError::refused("reading the clock for the resolution batch"),
    )?;
    let committed = if let Some(item_id) = state.assistant_item.clone() {
        commit_resolution_replace(
            context,
            state,
            checkpoint,
            operated_at,
            &item_id,
            &body,
            &patch_id,
        )
        .await
    } else {
        commit_resolution_start(context, state, checkpoint, operated_at, &body, &patch_id).await
    };
    if committed.is_ok() {
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
async fn commit_resolution_replace(
    context: &TurnConsumptionContext<'_>,
    state: &mut TurnConsumptionState<'_>,
    checkpoint: artisan_database::EngineCheckpoint,
    operated_at: UnixMillis,
    item_id: &ItemId,
    body: &AssistantBody,
    patch_id: &PatchId,
) -> Result<(), StepError> {
    let changes = [AssistantChange::Replace {
        item_id,
        expected_revision: state.assistant_revision,
        body,
        phase: AssistantMessagePhase::Unspecified,
        patch_id,
    }];
    commit_batch_with_retry(CommitBatchRequest {
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
    .map_err(|error| {
        StepError::failed(
            "committing the resolution beside the existing assistant item",
            error,
        )
    })?;
    let next_revision = state.assistant_revision.checked_next().map_err(|error| {
        StepError::failed("advancing the assistant revision after the commit", error)
    })?;
    state.assistant_revision = next_revision;
    state.scope.expected_updated_at = operated_at;
    let next_sequence = state
        .batch_sequence
        .checked_add(1)
        .ok_or(StepError::refused(
            "advancing the batch sequence after the commit: the counter overflowed",
        ))?;
    state.batch_sequence = next_sequence;
    Ok(())
}

/// Commits a resolution checkpoint while opening the assistant item.
///
/// Used only when the response arrived before any text: the item opens with
/// the current (possibly empty) body exactly like the text path opens it,
/// including turn activation.
async fn commit_resolution_start(
    context: &TurnConsumptionContext<'_>,
    state: &mut TurnConsumptionState<'_>,
    checkpoint: artisan_database::EngineCheckpoint,
    operated_at: UnixMillis,
    body: &AssistantBody,
    patch_id: &PatchId,
) -> Result<(), StepError> {
    let item_id = mint_item_id(context.origin).ok_or(StepError::refused(
        "minting the assistant item id: entropy or identifier validation failed",
    ))?;
    let activation_patch_id = mint_patch_id(context.origin).ok_or(StepError::refused(
        "minting the turn activation patch id: entropy or identifier validation failed",
    ))?;
    let changes = [AssistantChange::Start {
        item_id: &item_id,
        phase: AssistantMessagePhase::Unspecified,
        body,
        patch_id,
    }];
    commit_batch_with_retry(CommitBatchRequest {
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
    .map_err(|error| {
        StepError::failed(
            "committing the resolution while opening the assistant item",
            error,
        )
    })?;
    state.assistant_item = Some(item_id);
    state.assistant_revision = Revision::new(0);
    state.scope.expected_updated_at = operated_at;
    let next_sequence = state
        .batch_sequence
        .checked_add(1)
        .ok_or(StepError::refused(
            "advancing the batch sequence after the commit: the counter overflowed",
        ))?;
    state.batch_sequence = next_sequence;
    Ok(())
}

/// Assigns the resolution a transcript sequence, independently of the request ledger.
pub(crate) fn resolution_checkpoint(
    engine: EngineId,
    binding_version: i64,
    base: Option<u64>,
    applied: &artisan_database::AppliedInteraction,
    observation_id: &ObservationId,
) -> Option<artisan_database::EngineCheckpoint> {
    // Provider activity can advance the transcript while an answer is delivered.
    let sequence = ObservationSequence::new(base.unwrap_or(0).checked_add(1)?).ok()?;
    let resolved = build_resolved_observation(applied, observation_id, sequence)?;
    artisan_database::encode_observation_checkpoint(engine, binding_version, base, &[resolved]).ok()
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
        assistant_phase: state.assistant_phase,
    };
    if let Err(error) = commit_subagent_observation(
        context.repository,
        context.config,
        context.origin,
        &mut cursor,
        observation,
    )
    .await
    {
        mark_interrupted(state, turn, true, &error);
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
        assistant_phase: state.assistant_phase,
    };
    if let Err(error) = commit_activity_observation(
        context.repository,
        context.config,
        context.origin,
        &mut cursor,
        observation,
    )
    .await
    {
        mark_interrupted(state, turn, true, &error);
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
    // A valid text stream must not be presented as durably completed when
    // its authenticated usage observation could not be fenced/persisted.
    if report.run_id() != &state.scope.launched.run_id
        || report.thread_id() != &state.scope.launched.thread_id
    {
        mark_interrupted(
            state,
            turn,
            true,
            &StepError::refused(
                "recording run usage: the engine reported usage for a different run or thread",
            ),
        );
        return;
    }
    if let Err(error) = context
        .repository
        .record_run_usage(RecordRunUsage {
            run_id: &state.scope.launched.run_id,
            thread_id: &state.scope.launched.thread_id,
            report,
        })
        .await
    {
        mark_interrupted(
            state,
            turn,
            true,
            &StepError::failed("recording run usage", error),
        );
    }
}

async fn handle_text_snapshot(
    context: &TurnConsumptionContext<'_>,
    state: &mut TurnConsumptionState<'_>,
    turn: &mut AcceptedTurn,
    snapshot: TextSnapshot,
) {
    if snapshot.run_id() != &state.scope.launched.run_id {
        mark_interrupted(
            state,
            turn,
            true,
            &StepError::refused(
                "accepting a text snapshot: the engine addressed it to a different run",
            ),
        );
        return;
    }
    // A snapshot rewrites the assembled body, so the buffered append run must
    // reach durable state first; otherwise the snapshot's replace would
    // silently drop the uncommitted suffix from the append history.
    if !flush_pending_deltas(context, state, turn).await {
        return;
    }
    if state.assistant_parts.replace_snapshot(&snapshot).is_none() {
        mark_interrupted(
            state,
            turn,
            true,
            &StepError::refused(
                "applying a text snapshot to the assembled body: the part order or the body bound refused it",
            ),
        );
        return;
    }
    if !super::message_parts::select_part(context, state, turn, snapshot.part_id()).await {
        return;
    }
    let next_body = state
        .assistant_parts
        .part_body(snapshot.part_id())
        .to_owned();
    let next_phase = snapshot.phase().unwrap_or(state.assistant_phase);
    if next_body == state.assistant_body
        && next_phase == state.assistant_phase
        && state.assistant_item.is_some()
    {
        return;
    }
    state.assistant_phase = next_phase;
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
            mark_interrupted(
                state,
                turn,
                true,
                &StepError::refused(
                    "accepting a text delta: the engine addressed it to a different run",
                ),
            );
        }
        return;
    }
    if state.assistant_parts.append_delta(&delta).is_none() {
        mark_interrupted(
            state,
            turn,
            true,
            &StepError::refused(
                "appending a text delta to the assembled body: the part order or the body bound refused it",
            ),
        );
        return;
    }
    let part = delta.part_id().unwrap_or("fixture-text-part");
    if !super::message_parts::select_part(context, state, turn, part).await {
        return;
    }
    let next_body = state.assistant_parts.part_body(part).to_owned();
    let phase = delta.phase().unwrap_or(state.assistant_phase);
    let appended_exactly = phase == state.assistant_phase
        && next_body.strip_prefix(&state.assistant_body) == Some(delta.delta());
    state.assistant_phase = phase;
    state.assistant_body = next_body;
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
    if let Err(error) = context
        .repository
        .settle_run_interactions(&state.scope.launched.run_id)
        .await
    {
        report_step_failure(
            &state.scope,
            &StepError::failed("clearing the run's pending approvals and questions", error),
        );
    }
    let engine = state.engine;
    let assistant_phase = state.assistant_phase;
    let label = RunLabel(&state.scope).to_string();
    let TurnConsumptionState {
        scope,
        assistant_item: Some(item_id),
        assistant_revision,
        assistant_body,
        ..
    } = state
    else {
        // Nothing opened an assistant item, so there is no item to settle
        // the run on; lease recovery owns the run from here.
        eprintln!(
            "native run left to recovery instead of settling as {terminal:?} ({label}, engine {engine:?}): the turn never opened an assistant item"
        );
        return;
    };
    let prepared = prepare_settlement(context, &scope, assistant_body);
    let (body, item_patch_id, turn_patch_id, operated_at) = match prepared {
        Ok(prepared) => prepared,
        Err(error) => {
            eprintln!(
                "native run could not be settled as {terminal:?} ({label}, engine {engine:?}): {}",
                ErrorChain(&error)
            );
            return;
        }
    };
    let phase = if assistant_phase == AssistantMessagePhase::Commentary {
        AssistantMessagePhase::Commentary
    } else if matches!(terminal, TerminalState::Completed) {
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
    match persist_terminal_settlement(&settlement, terminal).await {
        Ok(()) => {
            let _ = context.config.notifier.publish(&scope.launched.thread_id);
        }
        Err(error) => {
            // The run keeps its live row; lease recovery settles it later.
            eprintln!(
                "native run could not be settled as {terminal:?} ({label}, engine {engine:?}, {} attempt(s)): {}",
                context.config.max_command_retries,
                ErrorChain(&error)
            );
        }
    }
}

/// Validates the body and mints the identities and instant one terminal
/// settlement needs.
fn prepare_settlement(
    context: &TurnConsumptionContext<'_>,
    scope: &RunBatchScope<'_>,
    assistant_body: String,
) -> Result<(AssistantBody, PatchId, PatchId, UnixMillis), StepError> {
    let body = AssistantBody::parse(assistant_body)
        .map_err(|error| StepError::failed("validating the final assistant body", error))?;
    let item_patch_id = mint_patch_id(context.origin).ok_or(StepError::refused(
        "minting the item patch id: entropy or identifier validation failed",
    ))?;
    let turn_patch_id = mint_patch_id(context.origin).ok_or(StepError::refused(
        "minting the turn patch id: entropy or identifier validation failed",
    ))?;
    let operated_at = at_or_after(context.origin, scope.expected_updated_at).ok_or(
        StepError::refused("reading the clock for the terminal settlement"),
    )?;
    Ok((body, item_patch_id, turn_patch_id, operated_at))
}

/// Persists the terminal state, retrying up to the configured attempts.
///
/// # Errors
///
/// Returns the last attempt's failure.
async fn persist_terminal_settlement(
    settlement: &TerminalSettlement<'_>,
    terminal: TerminalState,
) -> Result<(), StepError> {
    match terminal {
        TerminalState::Completed => persist_completed(settlement).await,
        TerminalState::Failed => persist_failed(settlement).await,
        TerminalState::Cancelled => persist_cancelled(settlement).await,
        TerminalState::Interrupted => persist_interrupted(settlement).await,
    }
}

async fn persist_completed(settlement: &TerminalSettlement<'_>) -> Result<(), StepError> {
    let mut last = StepError::refused("storing the completed run: no attempt ran");
    for _ in 0..settlement.retries.get() {
        match settlement
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
        {
            Ok(_) => return Ok(()),
            Err(error) => last = StepError::failed("storing the completed run", error),
        }
    }
    Err(last)
}

async fn persist_failed(settlement: &TerminalSettlement<'_>) -> Result<(), StepError> {
    let error_code = RunErrorCode::parse(PROVIDER_FAILURE_CODE.to_owned())
        .map_err(|error| StepError::failed("validating the provider failure code", error))?;
    let error_message = RunErrorMessage::parse(PROVIDER_FAILURE_MESSAGE.to_owned())
        .map_err(|error| StepError::failed("validating the provider failure message", error))?;
    let mut last = StepError::refused("storing the failed run: no attempt ran");
    for _ in 0..settlement.retries.get() {
        match settlement
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
        {
            Ok(_) => return Ok(()),
            Err(error) => last = StepError::failed("storing the failed run", error),
        }
    }
    Err(last)
}

async fn persist_cancelled(settlement: &TerminalSettlement<'_>) -> Result<(), StepError> {
    let mut last = StepError::refused("storing the cancelled run: no attempt ran");
    for _ in 0..settlement.retries.get() {
        match settlement
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
        {
            Ok(_) => return Ok(()),
            Err(error) => last = StepError::failed("storing the cancelled run", error),
        }
    }
    Err(last)
}

async fn persist_interrupted(settlement: &TerminalSettlement<'_>) -> Result<(), StepError> {
    let error_code = RunErrorCode::parse(INTERRUPTED_CODE.to_owned())
        .map_err(|error| StepError::failed("validating the interrupted run code", error))?;
    let error_message = RunErrorMessage::parse(INTERRUPTED_MESSAGE.to_owned())
        .map_err(|error| StepError::failed("validating the interrupted run message", error))?;
    let mut last = StepError::refused("storing the interrupted run: no attempt ran");
    for _ in 0..settlement.retries.get() {
        match settlement
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
        {
            Ok(_) => return Ok(()),
            Err(error) => last = StepError::failed("storing the interrupted run", error),
        }
    }
    Err(last)
}
