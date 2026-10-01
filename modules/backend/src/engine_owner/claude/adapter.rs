use std::collections::HashMap;

use artisan_domain::{
    MessagePhase, Observation, ObservationId, ObservationSequence, QuestionObservation, RunId,
    SubagentInput, SubagentObservation, SubagentState, SubagentTranscriptObservation,
    TranscriptAgentMessageDelta, TranscriptContent,
};
use tokio::io::AsyncWrite;
use tokio::sync::mpsc;

#[cfg(test)]
use super::super::observation::TerminalObservation;
use super::super::observation::{EngineObservation, TerminalState, TextSnapshot, chunk_text};

use super::content::ClaudeAssistantContent;
use super::launch::ClaudeThinkingDisplay;
use super::protocol::{
    CLAUDE_MAX_ANSWERS, ClaudeApprovalRequest, ClaudeEvent, ClaudeQuestion, ClaudeQuestionRequest,
    ClaudeTurnError, approval_response_line, question_response_line, user_message_with_images,
    write_line,
};
use super::text::{ClaudeTextLedger, ClaudeTextSettlement};
use super::thinking::ClaudeThinkingTracker;
use super::tools::ClaudeToolTracker;
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

/// Builds the requested question rows for one `AskUserQuestion` request,
/// every question in the request's questionnaire. A question that fails the
/// domain bounds drops fail-closed.
fn question_rows(
    run_id: &RunId,
    frame_sequence: u64,
    request_id: &str,
    questions: &[ClaudeQuestion],
) -> Vec<Observation> {
    let Ok(sequence) = ObservationSequence::new(frame_sequence) else {
        return Vec::new();
    };
    let Ok(group) = ObservationId::parse(request_id.to_owned()) else {
        return Vec::new();
    };
    questions
        .iter()
        .enumerate()
        .filter_map(|(index, question)| {
            let id = ObservationId::parse(format!(
                "{}:claude:{frame_sequence}:question-{index}",
                run_id.as_str()
            ))
            .ok()?;
            let input = question.to_domain_input().ok()?;
            QuestionObservation::requested(id, sequence, input)
                .ok()
                .map(|row| Observation::Question(row.with_group(group.clone())))
        })
        .collect()
}

/// One open `AskUserQuestion` request with the answers recorded so far.
#[derive(Debug)]
struct PendingQuestionRequest {
    request_id: String,
    /// The verbatim request input the answer response amends.
    input: serde_json::Value,
    /// The validated questions of the request.
    questions: Vec<ClaudeQuestion>,
    /// Answers by provider question id; a skipped question records an
    /// empty list.
    answers: HashMap<String, Vec<String>>,
}

impl PendingQuestionRequest {
    /// Whether the request holds the question and it has no answer yet.
    fn is_open(&self, question_id: &str) -> bool {
        self.questions
            .iter()
            .any(|question| question.question_id() == question_id)
            && !self.answers.contains_key(question_id)
    }

    /// The control response that answers the request: the verbatim input
    /// amended with every answered question keyed by its text, skipped
    /// questions left out. A request with no answer at all (every question
    /// skipped) is denied instead, so the tool reports that the user
    /// disregarded the question rather than inventing answers.
    fn reply_line(&self) -> String {
        let joined: Vec<(String, String)> = self
            .questions
            .iter()
            .filter_map(|question| {
                let answers = self.answers.get(question.question_id())?;
                (!answers.is_empty()).then(|| (question.text().to_owned(), answers.join(", ")))
            })
            .take(CLAUDE_MAX_ANSWERS)
            .collect();
        if joined.is_empty() {
            serde_json::json!({
                "type": "control_response",
                "response": {
                    "subtype": "success",
                    "request_id": self.request_id,
                    "response": {
                        "behavior": "deny",
                        "message": "User disregarded your question",
                    },
                },
            })
            .to_string()
        } else {
            question_response_line(&self.request_id, &self.input, &joined)
        }
    }
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
    /// Open question requests by request id, each with the answers
    /// recorded so far.
    question_requests: HashMap<String, PendingQuestionRequest>,
    subagents: Vec<String>,
    child_frames: Vec<(String, u64)>,
    subagent_rows: Vec<Observation>,
    thinking_tokens: Option<u64>,
    thinking: ClaudeThinkingTracker,
    text: ClaudeTextLedger,
    tools: ClaudeToolTracker,
    permission_denials: usize,
    stream_message_id: Option<String>,
    init_seen: bool,
    result_seen: bool,
    root_turn: RootTurn,
    /// Background agents and workflows still running behind the root turn.
    background_tasks: usize,
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

    /// Notes one question request; re-noting the same request id is a
    /// no-op. Returns the questions newly asked.
    ///
    /// Each question is validated through the domain constructor first, so
    /// an out-of-bound provider frame never reaches the durable rows; a
    /// request whose questions all fail validation is not tracked, since
    /// nothing could ever answer it.
    pub(crate) fn note_questions(
        &mut self,
        request: &ClaudeQuestionRequest,
    ) -> Vec<ClaudeQuestion> {
        if self.question_requests.contains_key(request.request_id()) {
            return Vec::new();
        }
        let asked: Vec<ClaudeQuestion> = request
            .questions()
            .iter()
            .filter(|question| question.to_domain_input().is_ok())
            .cloned()
            .collect();
        if asked.is_empty() {
            return Vec::new();
        }
        self.question_requests.insert(
            request.request_id().to_owned(),
            PendingQuestionRequest {
                request_id: request.request_id().to_owned(),
                input: request.input.clone(),
                questions: asked.clone(),
                answers: HashMap::new(),
            },
        );
        asked
    }

    /// Records the durable answer to one question of an open request.
    ///
    /// Returns the control response line once every question of the
    /// request has an answer, and [`None`] while others are still open; the
    /// request then leaves the tracker. Unknown or already answered
    /// questions are refused.
    ///
    /// # Errors
    ///
    /// Returns [`ClaudeTurnError::Configuration`] when no open request holds
    /// the question.
    pub(crate) fn record_question_answer(
        &mut self,
        question_id: &str,
        answers: &[String],
    ) -> Result<Option<String>, ClaudeTurnError> {
        let Some((request_id, pending)) = self
            .question_requests
            .iter_mut()
            .find(|(_, pending)| pending.is_open(question_id))
        else {
            return Err(ClaudeTurnError::Configuration);
        };
        pending
            .answers
            .insert(question_id.to_owned(), answers.to_vec());
        if pending.answers.len() < pending.questions.len() {
            return Ok(None);
        }
        let request_id = request_id.clone();
        let pending = self
            .question_requests
            .remove(&request_id)
            .ok_or(ClaudeTurnError::Configuration)?;
        Ok(Some(pending.reply_line()))
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

    /// Notes root turn activity: the session is inside a turn again.
    pub(crate) fn note_turn_opened(&mut self) {
        self.root_turn = RootTurn::Open;
    }

    /// Returns whether the session has nothing left to say (pump-only): its
    /// last root turn reported a `result`, none opened since, and no
    /// background agent or workflow still runs. Only then may stdin close;
    /// until then a follow-up message can still be written.
    pub(crate) fn input_settled(&self) -> bool {
        self.root_turn == RootTurn::Reported && self.background_tasks == 0
    }

    /// Returns whether a semantic failure was classified (pump-only).
    pub(crate) fn semantic_failure(&self) -> bool {
        self.semantic_failure
    }

    /// Resolves one approval; returns whether it was pending.
    pub(crate) fn resolve_approval(&mut self, approval_id: &str) -> bool {
        self.approvals.remove(approval_id).is_some()
    }

    /// Returns the number of pending approvals.
    #[cfg(test)]
    pub(crate) fn pending_approvals(&self) -> usize {
        self.approvals.len()
    }

    /// Returns the number of questions still waiting for an answer.
    #[cfg(test)]
    pub(crate) fn pending_questions(&self) -> usize {
        self.question_requests
            .values()
            .map(|pending| pending.questions.len() - pending.answers.len())
            .sum()
    }

    /// Returns whether the provider is waiting on the user: an open
    /// approval or question keeps the turn alive however long the user
    /// takes.
    pub(crate) fn waiting_on_user(&self) -> bool {
        !self.approvals.is_empty() || !self.question_requests.is_empty()
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

    /// Returns whether a requested highlights display was silently omitted.
    pub(crate) fn highlights_refused(&self) -> bool {
        self.thinking.highlights_refused()
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

/// Where the root session stands relative to its `result` frames.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum RootTurn {
    /// A root turn is running, or none has reported yet.
    #[default]
    Open,
    /// The last root turn reported its `result` and none has opened since.
    Reported,
}

/// How one applied event continues the pump.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ClaudeApplyOutcome {
    /// Keep pumping; `end_input` closes stdin exactly once: a `result`
    /// arrived with no background agent or workflow still running.
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

/// Sends validated activity rows (reasoning and tool work) in order.
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

/// Re-phases the current message as commentary the first time it calls a
/// tool, replacing its part with the same body under the new phase.
async fn mark_commentary(
    observations: &mpsc::Sender<EngineObservation>,
    run_id: &RunId,
    tracker: &mut ClaudePendingTracker,
    frame_sequence: u64,
) -> Result<(), TerminalState> {
    let Some(body) = tracker.text.mark_commentary().map(str::to_owned) else {
        return Ok(());
    };
    let Some(part_id) = tracker.stream_message_id.clone() else {
        return Ok(());
    };
    let snapshot = TextSnapshot::new(run_id.clone(), frame_sequence, part_id, body)
        .with_phase(artisan_domain::AssistantMessagePhase::Commentary);
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
                let phase = if tracker.text.phase() == "commentary" {
                    "commentary"
                } else {
                    phase
                };
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
            ClaudeAssistantContent::Thinking { text, title } => {
                let rows = tracker.thinking.buffered(
                    run_id,
                    frame_sequence,
                    message_id.as_deref(),
                    text,
                    title.clone(),
                );
                emit_rows(observations, rows).await?;
            }
            ClaudeAssistantContent::ToolUse(tool) => {
                mark_commentary(observations, run_id, tracker, frame_sequence).await?;
                let rows = tracker.tools.started(run_id, frame_sequence, tool);
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
            tracker.note_turn_opened();
            tracker.text.message_started(&message_id);
            tracker.stream_message_id = Some(message_id);
            tracker.thinking.message_started();
            continued
        }
        ClaudeEvent::TextDelta { delta, phase } => {
            tracker.note_turn_opened();
            if active_turn.is_none() {
                *active_turn = Some(expected_session.to_owned());
            }
            tracker.text.streamed(&delta);
            let phase = if tracker.text.phase() == "commentary" {
                "commentary"
            } else {
                phase
            };
            let part_id = tracker.stream_message_id.as_deref();
            settle(emit_text(observations, run_id, part_id, frame_sequence, &delta, phase).await)
        }
        ClaudeEvent::Assistant(frame) => {
            tracker.note_turn_opened();
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
        ClaudeEvent::ThinkingStarted { index, title } => {
            let message_id = tracker.stream_message_id.clone();
            tracker.thinking.start(message_id.as_deref(), index, title);
            settle(
                emit_rows(
                    observations,
                    tracker.thinking.started(run_id, frame_sequence),
                )
                .await,
            )
        }
        ClaudeEvent::ThinkingDelta { index, text } => {
            let rows = tracker.thinking.delta(run_id, frame_sequence, index, &text);
            settle(emit_rows(observations, rows).await)
        }
        ClaudeEvent::ToolUseStarted => {
            settle(mark_commentary(observations, run_id, tracker, frame_sequence).await)
        }
        ClaudeEvent::ToolResults(results) => {
            let rows = results
                .iter()
                .flat_map(|result| tracker.tools.finished(run_id, frame_sequence, result))
                .collect();
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
            // The request stays open in the tracker until the user answers
            // every question; the questions present as one questionnaire.
            let asked = tracker.note_questions(&request);
            settle(
                emit_rows(
                    observations,
                    question_rows(run_id, frame_sequence, request.request_id(), &asked),
                )
                .await,
            )
        }
        ClaudeEvent::SubagentLifecycle { task_id } => {
            // Discovery emits its row; the root turn is never adopted and no
            // root text is emitted.
            tracker.note_subagent(run_id, expected_session, &task_id, frame_sequence);
            ClaudeApplyOutcome::Continue { end_input: false }
        }
        ClaudeEvent::BackgroundTasks { waited } => {
            // A level, not an edge: each payload replaces the count. Closing
            // stdin stays with the `result` arm and the pump's settle grace,
            // since the last task ending queues a follow-up turn.
            tracker.background_tasks = waited;
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
            tracker.root_turn = RootTurn::Reported;
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
            // Background agents and workflows report back into this session:
            // stdin stays open for them, and for steers, until they finish.
            ClaudeApplyOutcome::Continue {
                end_input: tracker.background_tasks == 0,
            }
        }
        ClaudeEvent::Unknown => ClaudeApplyOutcome::Continue { end_input: false },
    }
}

/// Steers a live turn with a follow-up message (stream-input fold).
///
/// Production verb behind [`AcceptedTurn::steer_message`](super::operation::AcceptedTurn::steer_message):
/// the pump writes the fold line over its owned stdin and the write outcome
/// resolves the delivery. Proves the fold verb against the fixture stdio
/// script without disturbing the authorize-once production flow.
/// Experimental per the adapter: the CLI owns fold timing. Images ride as
/// native base64 content blocks after the text, exactly like a fresh send;
/// an image-only steer carries no empty text block.
///
/// # Errors
///
/// Returns [`ClaudeTurnError`] when the write fails.
pub(crate) async fn steer_live_turn<W: AsyncWrite + Unpin>(
    stdin: &mut W,
    session_id: &str,
    text: &str,
    images: &[artisan_domain::ImageAttachment],
) -> Result<(), ClaudeTurnError> {
    let line = user_message_with_images(session_id, (!text.is_empty()).then_some(text), images);
    write_line(stdin, &line).await
}

/// Answers one pending approval through the durable decision.
///
/// Deny carries no turn side effect and the run continues either way.
///
/// # Errors
///
/// Returns [`ClaudeTurnError::Configuration`] for an unknown or resolved
/// target and [`ClaudeTurnError::StreamFailed`] when the write fails.
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
