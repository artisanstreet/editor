use std::collections::HashMap;

use artisan_domain::{
    MessagePhase, Observation, ObservationId, ObservationSequence, RunId, SubagentInput,
    SubagentObservation, SubagentState, SubagentTranscriptObservation, TranscriptAgentMessageDelta,
    TranscriptContent,
};
use tokio::io::AsyncWrite;
use tokio::sync::mpsc;

#[cfg(test)]
use super::super::observation::TerminalObservation;
use super::super::observation::{EngineObservation, TerminalState, TextSnapshot, chunk_text};

use super::content::ClaudeAssistantContent;
use super::launch::ClaudeThinkingDisplay;
#[cfg(test)]
use super::protocol::{CLAUDE_MAX_ANSWERS, approval_response_line, question_response_line};
use super::protocol::{
    ClaudeApprovalRequest, ClaudeEvent, ClaudeQuestion, ClaudeQuestionRequest, ClaudeTurnError,
    user_message_line, write_line,
};
use super::text::{ClaudeTextLedger, ClaudeTextSettlement};
use super::thinking::ClaudeThinkingTracker;
use super::usage::{ClaudeUsageScope, project_usage_sample};

/// Builds one validated subagent discovery row.
///
/// Fails closed (no row, discovery still tracked) when a provider identity
/// exceeds the domain ceilings: row identities reuse the full native thread
/// identities verbatim and are never truncated into ambiguity.
fn discovered_subagent_row(
    run_id: &RunId,
    parent_session: &str,
    task_id: &str,
    frame_sequence: u64,
) -> Option<Observation> {
    let id = ObservationId::parse(format!(
        "{}:claude:subagent:{task_id}:{frame_sequence}",
        run_id.as_str()
    ))
    .ok()?;
    let sequence = ObservationSequence::new(frame_sequence).ok()?;
    let input = SubagentInput {
        agent_native_thread_id: ObservationId::parse(task_id).ok()?,
        parent_native_thread_id: ObservationId::parse(parent_session).ok()?,
        state: SubagentState::Discovered,
        activity: None,
        agent_path: None,
        turn_id: None,
    };
    SubagentObservation::new(id, sequence, input)
        .ok()
        .map(Observation::Subagent)
}

/// Projects one child text fragment into a validated transcript row.
///
/// The child stream is keyed by its provider tool invocation until the task
/// lineage packet correlates invocations to agent tasks; only message text
/// projects, never approvals, questions, results, or reasoning.
fn child_transcript_row(
    run_id: &RunId,
    parent_session: &str,
    parent_tool_use_id: &str,
    delta: &str,
    phase: &str,
    frame_sequence: u64,
) -> Option<Observation> {
    let agent_id = ObservationId::parse(parent_tool_use_id).ok()?;
    let parent_id = ObservationId::parse(parent_session).ok()?;
    let phase = MessagePhase::parse(phase).ok()?;
    let item_id = ObservationId::parse(format!(
        "{}:claude:childmsg:{parent_tool_use_id}:{frame_sequence}",
        run_id.as_str()
    ))
    .ok()?;
    let content = TranscriptAgentMessageDelta::new(item_id, phase, delta.to_owned()).ok()?;
    let id = ObservationId::parse(format!(
        "{}:claude:childrow:{parent_tool_use_id}:{frame_sequence}",
        run_id.as_str()
    ))
    .ok()?;
    let sequence = ObservationSequence::new(frame_sequence).ok()?;
    Some(Observation::SubagentTranscript(
        SubagentTranscriptObservation::new(
            id,
            sequence,
            agent_id,
            parent_id,
            TranscriptContent::AgentMessageDelta(content),
        ),
    ))
}

/// In-memory pending interaction tracker for one live Claude turn.
///
/// Permission requests land as pending approvals and `AskUserQuestion` frames
/// land as pending questions. Resolutions apply through the durable resolve
/// path; a deny records the decision with no turn side effect while the run
/// continues. Subagent discoveries emit validated `Discovered` rows and child
/// transcript frames project validated transcript rows; both accumulate for
/// the consumer drain without ever reaching the root turn. Thinking
/// estimates stay plumbing; thinking stretches project through the run-local
/// [`ClaudeThinkingTracker`] and assistant text settles through the
/// [`ClaudeTextLedger`].
#[derive(Debug, Default)]
pub(crate) struct ClaudePendingTracker {
    approvals: HashMap<String, ClaudeApprovalRequest>,
    questions: HashMap<String, ClaudeQuestion>,
    subagents: Vec<String>,
    child_frames: Vec<(String, u64)>,
    subagent_rows: Vec<Observation>,
    thinking_tokens: Option<u64>,
    thinking: ClaudeThinkingTracker,
    text: ClaudeTextLedger,
    permission_denials: usize,
    stream_message_id: Option<String>,
    init_seen: bool,
    result_seen: bool,
    semantic_failure: bool,
    summary_title: Option<String>,
}

impl ClaudePendingTracker {
    /// Creates an empty tracker for one turn that projects no thinking text.
    #[cfg(test)]
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Creates an empty tracker for one turn under the launch's thinking
    /// display policy.
    pub(crate) fn with_thinking_display(display: ClaudeThinkingDisplay) -> Self {
        Self {
            thinking: ClaudeThinkingTracker::new(display),
            ..Self::default()
        }
    }

    /// Notes one approval request; re-noting the same id is a no-op.
    ///
    /// The request is validated through the domain constructor first, so an
    /// out-of-bound provider frame never reaches the durable A-approve rows.
    pub(crate) fn note_approval(&mut self, request: ClaudeApprovalRequest) -> bool {
        if request.to_domain_request().is_err() {
            return false;
        }
        if self.approvals.contains_key(&request.approval_id) {
            return false;
        }
        self.approvals.insert(request.approval_id.clone(), request);
        true
    }

    /// Notes one question request group; re-noting the same id is a no-op.
    ///
    /// Each question is validated through the domain constructor first, so
    /// an out-of-bound provider frame never reaches the durable rows.
    pub(crate) fn note_questions(&mut self, request: &ClaudeQuestionRequest) -> usize {
        let mut added = 0;
        for question in request.questions() {
            if question.to_domain_input().is_err() {
                continue;
            }
            if !self.questions.contains_key(question.question_id()) {
                self.questions
                    .insert(question.question_id().to_owned(), question.clone());
                added += 1;
            }
        }
        added
    }

    /// Notes one subagent lifecycle identity and emits its `Discovered` row.
    ///
    /// Re-noting the same identity is a no-op and emits nothing twice. The
    /// row carries the root session plus the agent thread identity with state
    /// `Discovered`; row construction fails closed (discovery still tracked)
    /// when a provider identity exceeds the domain ceilings.
    pub(crate) fn note_subagent(
        &mut self,
        run_id: &RunId,
        parent_session: &str,
        task_id: &str,
        frame_sequence: u64,
    ) {
        if self.subagents.iter().any(|known| known == task_id) {
            return;
        }
        self.subagents.push(task_id.to_owned());
        if let Some(row) = discovered_subagent_row(run_id, parent_session, task_id, frame_sequence)
        {
            self.subagent_rows.push(row);
        }
    }

    /// Projects one child transcript frame into an isolated transcript row.
    ///
    /// The frame is always counted; a row is stored only when the frame
    /// carried renderer-safe message text. The row never reaches the root
    /// turn: it accumulates for the consumer drain with its own durable
    /// identity and sequencing.
    pub(crate) fn note_child_frame(
        &mut self,
        run_id: &RunId,
        parent_session: &str,
        parent_tool_use_id: &str,
        text: Option<(String, &'static str)>,
        frame_sequence: u64,
    ) {
        self.child_frames
            .push((parent_tool_use_id.to_owned(), frame_sequence));
        let Some((delta, phase)) = text else {
            return;
        };
        if let Some(row) = child_transcript_row(
            run_id,
            parent_session,
            parent_tool_use_id,
            &delta,
            phase,
            frame_sequence,
        ) {
            self.subagent_rows.push(row);
        }
    }

    /// Drains validated subagent rows for the consumer in emission order.
    ///
    /// The fixture driver proves emission plus sequencing here; the
    /// dispatcher packet wires this drain into the live pump beside the text
    /// channel.
    pub(crate) fn take_subagent_rows(&mut self) -> Vec<Observation> {
        std::mem::take(&mut self.subagent_rows)
    }

    /// Preserves the encrypted-thinking estimate (never root text).
    pub(crate) fn note_thinking_tokens(&mut self, estimated_tokens: u64) {
        self.thinking_tokens = Some(estimated_tokens);
    }

    /// Retains the denied-permission count without approval semantics.
    pub(crate) fn note_permission_denials(&mut self, count: usize) {
        self.permission_denials += count;
    }

    /// Retains the generated session title captured at the terminal fence.
    ///
    /// Later captures replace earlier ones, mirroring the TypeScript reader
    /// that keeps the newest `ai-title` record; the title never disturbs the
    /// turn and settles onto the terminal `summary_title`.
    pub(crate) fn note_summary_title(&mut self, title: String) {
        self.summary_title = Some(title);
    }

    /// Returns the captured generated session title, if any arrived.
    pub(crate) fn summary_title(&self) -> Option<&str> {
        self.summary_title.as_deref()
    }

    /// Returns whether the terminal `result` frame arrived (pump-only).
    pub(crate) fn result_seen(&self) -> bool {
        self.result_seen
    }

    /// Returns whether a semantic failure was classified (pump-only).
    pub(crate) fn semantic_failure(&self) -> bool {
        self.semantic_failure
    }

    /// Resolves one approval; returns whether it was pending.
    ///
    /// Test-only until dispatcher delivery wiring lands.
    #[cfg(test)]
    pub(crate) fn resolve_approval(&mut self, approval_id: &str) -> bool {
        self.approvals.remove(approval_id).is_some()
    }

    /// Resolves one question; returns whether it was pending.
    ///
    /// Test-only until dispatcher delivery wiring lands.
    #[cfg(test)]
    pub(crate) fn resolve_question(&mut self, question_id: &str) -> bool {
        self.questions.remove(question_id).is_some()
    }

    /// Returns the number of pending approvals.
    #[cfg(test)]
    pub(crate) fn pending_approvals(&self) -> usize {
        self.approvals.len()
    }

    /// Returns the number of pending questions.
    #[cfg(test)]
    pub(crate) fn pending_questions(&self) -> usize {
        self.questions.len()
    }

    /// Returns the number of discovered subagent lifecycle identities.
    #[cfg(test)]
    pub(crate) fn subagent_count(&self) -> usize {
        self.subagents.len()
    }

    /// Returns the number of isolated child transcript frames.
    #[cfg(test)]
    pub(crate) fn child_frame_count(&self) -> usize {
        self.child_frames.len()
    }

    /// Returns the preserved thinking-token estimate, if any arrived.
    #[cfg(test)]
    pub(crate) fn thinking_tokens(&self) -> Option<u64> {
        self.thinking_tokens
    }

    /// Returns whether the init gate accepted the spawned session.
    #[cfg(test)]
    pub(crate) fn init_seen(&self) -> bool {
        self.init_seen
    }

    /// Returns the retained denied-permission count.
    #[cfg(test)]
    pub(crate) fn permission_denial_count(&self) -> usize {
        self.permission_denials
    }
}

/// How one applied event continues the pump.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ClaudeApplyOutcome {
    /// Keep pumping; `end_input` closes stdin exactly once (result seen).
    Continue { end_input: bool },
    /// Settle the turn now with this terminal state.
    Terminal(TerminalState),
}

fn message_phase(phase: &str) -> artisan_domain::AssistantMessagePhase {
    match phase {
        "commentary" => artisan_domain::AssistantMessagePhase::Commentary,
        "final" => artisan_domain::AssistantMessagePhase::Final,
        _ => artisan_domain::AssistantMessagePhase::Unspecified,
    }
}

/// Chunks one text part onto the shared vocabulary with its verbatim phase.
///
/// The current stream message id (when announced) becomes the explicit part
/// identity so one message and its completion stay grouped. Returns the
/// terminal state when the observation sink closed mid-send.
async fn emit_text(
    observations: &mpsc::Sender<EngineObservation>,
    run_id: &RunId,
    part_id: Option<&str>,
    frame_sequence: u64,
    delta: &str,
    phase: &str,
) -> Result<(), TerminalState> {
    let phase = message_phase(phase);
    let native_id = format!("claude:{frame_sequence}");
    for chunk in chunk_text(run_id, frame_sequence, &native_id, delta) {
        let chunk = chunk.with_phase(phase);
        let chunk = match part_id {
            Some(part) => chunk.with_part_id(part.to_owned()),
            None => chunk,
        };
        if observations
            .send(EngineObservation::TextDelta(chunk))
            .await
            .is_err()
        {
            return Err(TerminalState::Interrupted);
        }
    }
    Ok(())
}

/// Sends validated activity rows (reasoning observations) in order.
async fn emit_rows(
    observations: &mpsc::Sender<EngineObservation>,
    rows: Vec<Observation>,
) -> Result<(), TerminalState> {
    for row in rows {
        if observations
            .send(EngineObservation::Activity(row))
            .await
            .is_err()
        {
            return Err(TerminalState::Interrupted);
        }
    }
    Ok(())
}

/// Projects one buffered text block onto the message part it settles.
///
/// The buffered text is authoritative: streamed deltas that already equal it
/// emit nothing (only a non-default phase re-attributes the part), a missing
/// suffix appends, and diverging text replaces the whole message part as one
/// snapshot. A replacement needs the part identity the deltas carried; a
/// stream that never announced its message has none to correct.
async fn settle_buffered_text(
    observations: &mpsc::Sender<EngineObservation>,
    run_id: &RunId,
    part_id: Option<&str>,
    frame_sequence: u64,
    settlement: ClaudeTextSettlement,
    body: &str,
    phase: &str,
) -> Result<(), TerminalState> {
    let replacement = match settlement {
        ClaudeTextSettlement::Append(missing) => {
            return emit_text(
                observations,
                run_id,
                part_id,
                frame_sequence,
                &missing,
                phase,
            )
            .await;
        }
        ClaudeTextSettlement::Settled if phase == "unspecified" => return Ok(()),
        ClaudeTextSettlement::Settled => body.to_owned(),
        ClaudeTextSettlement::Replace(body) => body,
    };
    let Some(part_id) = part_id else {
        return Ok(());
    };
    let snapshot = TextSnapshot::new(
        run_id.clone(),
        frame_sequence,
        part_id.to_owned(),
        replacement,
    )
    .with_phase(message_phase(phase));
    observations
        .send(EngineObservation::TextSnapshot(snapshot))
        .await
        .map_err(|_| TerminalState::Interrupted)
}

/// Projects one buffered assistant frame: every supported part in provider
/// order, then its usage sample exactly once.
async fn apply_assistant_frame(
    frame: super::content::ClaudeAssistantFrame,
    run_id: &RunId,
    tracker: &mut ClaudePendingTracker,
    observations: &mpsc::Sender<EngineObservation>,
    frame_sequence: u64,
    usage: Option<&ClaudeUsageScope<'_>>,
) -> Result<(), TerminalState> {
    let message_id = frame
        .message_id
        .clone()
        .or_else(|| tracker.stream_message_id.clone());
    for part in &frame.content {
        match part {
            ClaudeAssistantContent::Text { text, phase } => {
                let settlement = tracker.text.buffered(frame.message_id.as_deref(), text);
                let part_id = tracker.stream_message_id.as_deref();
                settle_buffered_text(
                    observations,
                    run_id,
                    part_id,
                    frame_sequence,
                    settlement,
                    tracker.text.body(),
                    phase,
                )
                .await?;
            }
            ClaudeAssistantContent::Thinking { text } => {
                let rows =
                    tracker
                        .thinking
                        .buffered(run_id, frame_sequence, message_id.as_deref(), text);
                emit_rows(observations, rows).await?;
            }
        }
    }
    match frame.usage.as_ref() {
        Some(sample) => {
            match project_usage_sample(observations, run_id, usage, frame_sequence, sample).await {
                Some(state) => Err(state),
                None => Ok(()),
            }
        }
        None => Ok(()),
    }
}

/// Applies one typed event; returns how the pump continues.
///
/// Text chunks onto the shared vocabulary with the verbatim phase carried on
/// the event. Buffered assistant frames project text and thinking in order
/// plus one usage sample; thinking stretches project onto the shared
/// reasoning observations through the run-local thinking tracker. Usage
/// samples project best-effort onto the shared usage vocabulary when a usage
/// scope travels with the pump; without one (or on any attribution failure)
/// they are diagnostics that never disturb the turn. Usage collection never
/// blocks turns: only the observation sink closing is terminal. Session
/// identity mismatches fail the turn closed: only the exact spawned session
/// may speak for it.
#[expect(
    clippy::too_many_lines,
    reason = "single event dispatch table projecting typed events; extracting arms would thread the same sink and tracker"
)]
#[allow(clippy::too_many_arguments)]
pub(crate) async fn apply_event(
    event: ClaudeEvent,
    run_id: &RunId,
    expected_session: &str,
    tracker: &mut ClaudePendingTracker,
    active_turn: &mut Option<String>,
    observations: &mpsc::Sender<EngineObservation>,
    frame_sequence: u64,
    usage: Option<&ClaudeUsageScope<'_>>,
) -> ClaudeApplyOutcome {
    let continued = ClaudeApplyOutcome::Continue { end_input: false };
    let settle = |sent: Result<(), TerminalState>| match sent {
        Ok(()) => continued,
        Err(state) => ClaudeApplyOutcome::Terminal(state),
    };
    match event {
        ClaudeEvent::Init { session_id } => {
            if session_id != expected_session {
                return ClaudeApplyOutcome::Terminal(TerminalState::Failed);
            }
            tracker.init_seen = true;
            continued
        }
        ClaudeEvent::MessageStart { message_id } => {
            tracker.text.message_started(&message_id);
            tracker.stream_message_id = Some(message_id);
            tracker.thinking.message_started();
            continued
        }
        ClaudeEvent::TextDelta { delta, phase } => {
            if active_turn.is_none() {
                *active_turn = Some(expected_session.to_owned());
            }
            tracker.text.streamed(&delta);
            let part_id = tracker.stream_message_id.as_deref();
            settle(emit_text(observations, run_id, part_id, frame_sequence, &delta, phase).await)
        }
        ClaudeEvent::Assistant(frame) => {
            if frame.text().is_some() && active_turn.is_none() {
                *active_turn = Some(expected_session.to_owned());
            }
            settle(
                apply_assistant_frame(frame, run_id, tracker, observations, frame_sequence, usage)
                    .await,
            )
        }
        ClaudeEvent::ThinkingTokens { estimated_tokens } => {
            tracker.note_thinking_tokens(estimated_tokens);
            continued
        }
        ClaudeEvent::ThinkingStarted { index } => {
            let message_id = tracker.stream_message_id.clone();
            tracker.thinking.start(message_id.as_deref(), index);
            continued
        }
        ClaudeEvent::ThinkingDelta { index, text } => {
            let rows = tracker.thinking.delta(run_id, frame_sequence, index, &text);
            settle(emit_rows(observations, rows).await)
        }
        ClaudeEvent::ContentBlockStopped { index } => {
            let rows = tracker.thinking.stop(run_id, frame_sequence, index);
            settle(emit_rows(observations, rows).await)
        }
        ClaudeEvent::ApprovalRequested(request) => {
            tracker.note_approval(request);
            ClaudeApplyOutcome::Continue { end_input: false }
        }
        ClaudeEvent::QuestionRequested(request) => {
            tracker.note_questions(&request);
            ClaudeApplyOutcome::Continue { end_input: false }
        }
        ClaudeEvent::SubagentLifecycle { task_id } => {
            // Discovery emits its row; the root turn is never adopted and no
            // root text is emitted.
            tracker.note_subagent(run_id, expected_session, &task_id, frame_sequence);
            ClaudeApplyOutcome::Continue { end_input: false }
        }
        ClaudeEvent::ChildTranscript {
            parent_tool_use_id,
            text,
        } => {
            // Projection isolates the child row; the root turn is never
            // adopted and no root text is emitted.
            tracker.note_child_frame(
                run_id,
                expected_session,
                &parent_tool_use_id,
                text,
                frame_sequence,
            );
            ClaudeApplyOutcome::Continue { end_input: false }
        }
        ClaudeEvent::TurnResult {
            success,
            session_id,
            permission_denials,
            usage: sample,
        } => {
            if let Some(session_id) = session_id
                && session_id != expected_session
            {
                return ClaudeApplyOutcome::Terminal(TerminalState::Failed);
            }
            tracker.result_seen = true;
            if !success {
                tracker.semantic_failure = true;
            }
            tracker.note_permission_denials(permission_denials);
            if let Some(sample) = sample.as_ref()
                && let Some(state) =
                    project_usage_sample(observations, run_id, usage, frame_sequence, sample).await
            {
                return ClaudeApplyOutcome::Terminal(state);
            }
            ClaudeApplyOutcome::Continue { end_input: true }
        }
        ClaudeEvent::Unknown => ClaudeApplyOutcome::Continue { end_input: false },
    }
}

/// Steers a live turn with follow-up text (stream-input fold).
///
/// Production verb behind [`AcceptedTurn::steer_text`](super::operation::AcceptedTurn::steer_text):
/// the pump writes the fold line over its owned stdin and the write outcome
/// resolves the delivery. Proves the fold verb against the fixture stdio
/// script without disturbing the authorize-once production flow.
/// Experimental per the adapter: the CLI owns fold timing.
///
/// # Errors
///
/// Returns [`ClaudeTurnError`] when the write fails.
pub(crate) async fn steer_live_turn<W: AsyncWrite + Unpin>(
    stdin: &mut W,
    session_id: &str,
    text: &str,
) -> Result<(), ClaudeTurnError> {
    write_line(stdin, &user_message_line(session_id, text)).await
}

/// Answers one pending approval through the durable decision.
///
/// Test-only until dispatcher delivery wiring lands: deny carries no turn
/// side effect and the run continues either way.
///
/// # Errors
///
/// Returns [`ClaudeTurnError::Configuration`] for an unknown or resolved
/// target and [`ClaudeTurnError::StreamFailed`] when the write fails.
#[cfg(test)]
pub(crate) async fn answer_approval<W: AsyncWrite + Unpin>(
    stdin: &mut W,
    tracker: &mut ClaudePendingTracker,
    request_id: &str,
    approval_id: &str,
    approved: bool,
) -> Result<(), ClaudeTurnError> {
    if !tracker.resolve_approval(approval_id) {
        return Err(ClaudeTurnError::Configuration);
    }
    write_line(stdin, &approval_response_line(request_id, approved)).await
}

/// Answers one question request group through the durable answers.
///
/// Test-only until dispatcher delivery wiring lands. Answers accumulate per
/// question text; the response amends the verbatim request input.
///
/// # Errors
///
/// Returns [`ClaudeTurnError::Configuration`] for an unknown or resolved
/// target and [`ClaudeTurnError::StreamFailed`] when the write fails.
#[cfg(test)]
pub(crate) async fn answer_questions<W: AsyncWrite + Unpin>(
    stdin: &mut W,
    tracker: &mut ClaudePendingTracker,
    request: &ClaudeQuestionRequest,
    answers: &[(String, Vec<String>)],
) -> Result<(), ClaudeTurnError> {
    for (question_id, _) in answers {
        if !tracker.resolve_question(question_id) {
            return Err(ClaudeTurnError::Configuration);
        }
    }
    let mut joined = Vec::new();
    for (question_id, options) in answers.iter().take(CLAUDE_MAX_ANSWERS) {
        let Some(question) = request
            .questions()
            .iter()
            .find(|known| known.question_id() == question_id)
        else {
            return Err(ClaudeTurnError::Configuration);
        };
        joined.push((question.text().to_owned(), options.join(", ")));
    }
    let line = question_response_line(request.request_id(), &request.input, &joined);
    write_line(stdin, &line).await
}

/// Classifies a reaped child exit after close.
///
/// Test-only until the close path reports it: code 0 is a clean close and
/// nonzero is a provider failure. Cancellation and interruption are reported
/// by the driver, never inferred from the code.
#[cfg(test)]
pub(crate) fn classify_exit(status: std::process::ExitStatus) -> TerminalState {
    if status.success() {
        TerminalState::Completed
    } else {
        TerminalState::Failed
    }
}

/// Builds a terminal observation preserving caller identity and state.
///
/// Test-only observation helper for the fixture lifecycle assertions.
#[cfg(test)]
pub(crate) fn terminal_observation(
    run_id: &RunId,
    sequence: u64,
    state: TerminalState,
) -> TerminalObservation {
    TerminalObservation::new(run_id.clone(), sequence, state, None, None)
}
