//! Finite Claude owner runtime over `claude -p --output-format stream-json`.
//!
//! Spawns the verified Claude Code CLI as a stream-JSON stdio process, writes
//! the first user message over stdin, captures the `system/init` session
//! identity behind the bind authorization gate, normalizes streaming frames
//! onto the shared S1a observation vocabulary (`TextDelta` / `Terminal`;
//! usage and title stay later packets), and supports steer/follow-up,
//! cancel, and close with exit classification. An inactivity deadline stalls
//! a silent turn; child-custody teardown reuses the owner `process` contract
//! and quarantines on unobserved reaps.
//!
//! The wire boundary is typed: inbound lines decode into [`ClaudeEvent`]
//! through bounded `serde_json::Value` extraction. Only validated identities
//! and text cross the boundary: `control_request` permission frames become
//! [`ClaudeApprovalRequest`] and map onto domain `ApprovalRequest`
//! constructors for the durable A-approve resolve path, while
//! `AskUserQuestion` permission frames are lifted out of the approval path
//! and canonicalized as [`ClaudeQuestionRequest`] (answering a question *is*
//! the authorization; gating it behind a second approval leaves the CLI
//! blocked on a terminal dialog Artisan never renders, per
//! `modules/engines/src/claude/cli-engine.ts`). Deny lands with no side
//! effect while the turn continues; allow answers through the same durable
//! path.
//!
//! Stream text phases are preserved verbatim on the typed event
//! (`unspecified` for streamed deltas, `commentary` for assistant text that
//! accompanies tool uses, mirroring the TypeScript normalizer); both fold to
//! `TextDelta` on the shared vocabulary. Encrypted-thinking estimates
//! (`system/thinking_tokens`) and reasoning settlement without delta text are
//! preserved in the tracker as plumbing for a later packet, never as root
//! text. Subagent lifecycle frames emit validated [`Observation::Subagent`]
//! rows (state `Discovered`, root plus agent thread identities) and child
//! transcript frames (`parent_tool_use_id`) project into validated
//! [`Observation::SubagentTranscript`] rows carrying renderer-safe message
//! content only; neither ever reaches the root turn. Terminal mapping
//! preserves interruption (external kill / EOF before `result`) vs cancel vs
//! failure distinctly.
//!
//! Rows accumulate in the tracker until drained: the fixture driver proves
//! emission plus sequencing here, and the dispatcher packet wires the drain
//! into the live pump beside the text channel.
//!
//! L3 notes: continuation resumes provider-owned state only (never invented
//! checkpoints) by reopening the stored native session through `--resume`
//! behind [`check_claude_native_continuation`]; usage is collected
//! best-effort alongside the turn (assistant usage gauges project to
//! cumulative [`RunUsageReport`] rows with a replacing context gauge, terminal
//! totals project without one) and never blocks it; the `/usage` CLI buckets
//! map to diagnostics with clamped percents and kind classification without
//! inventing quota; the generated session title is captured best-effort from
//! the transcript at the terminal fence into the terminal `summary_title`;
//! teardown terminates the whole process group so no claude grandchild
//! holding a pipe is orphaned; interrupted runs replay the durable prefix on
//! resume without duplicating provider effects.
//!
//! Later packets own: model inventory, catalog flag flip, frontend selection,
//! and dispatcher persistence of the captured title and quota diagnostics.

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use artisan_domain::{
    ApprovalMode, ApprovalRequest, ClaudePermissionMode, ClaudeSelection, EngineModelId,
    EngineRouteId, FilesystemAccess, MessagePhase, NetworkAccess, OBSERVATION_TITLE_MAX_BYTES,
    Observation, ObservationId, ObservationSequence, QuestionInput, QuestionOption, RootPath,
    RunId, RunUsageBasis, RunUsageReport, RunUsageReportInput, SubagentInput, SubagentObservation,
    SubagentState, SubagentTranscriptObservation, ThreadId, TranscriptAgentMessageDelta,
    TranscriptContent, UnixMillis,
};
use artisan_native_engine::CLAUDE_NATIVE_CONTINUATION_VERSION;
use serde_json::Value;
use thiserror::Error;
use tokio::io::{AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc;
use tokio::time::Instant;

#[cfg(test)]
use super::observation::TerminalObservation;
use super::observation::{EngineObservation, TerminalState, UsageObservation, chunk_text};

/// Maximum accepted stream-JSON line bytes (mirrors the TS 1 MiB frame cap).
pub(crate) const CLAUDE_MAX_FRAME_BYTES: usize = 1_048_576;

/// Maximum UTF-8 bytes retained for one approval/question text field.
const CLAUDE_MAX_TEXT_FIELD_BYTES: usize = 8 * 1024;

/// Maximum questions retained per `AskUserQuestion` frame.
const CLAUDE_MAX_QUESTIONS_PER_FRAME: usize = 32;

/// Maximum options retained per question (the domain observation ceiling).
const CLAUDE_MAX_OPTIONS_PER_QUESTION: usize = 16;

/// Maximum answers retained per question response (the domain ceiling).
#[cfg(test)]
pub(crate) const CLAUDE_MAX_ANSWERS: usize = 16;

/// Maximum identities accepted for one session/task/request id.
const CLAUDE_MAX_ID_BYTES: usize = 256;

/// Payload-free failure of the Claude wire boundary.
///
/// Transport mistakes (spawn, prompt, stream, stall, cancel, shutdown,
/// deadline, interruption, exit) surface as the owner
/// [`EngineOperationError`](super::operation::EngineOperationError) at the
/// dispatch arm; only typed-boundary mistakes originate here.
#[derive(Clone, Debug, Eq, PartialEq, Error)]
pub(crate) enum ClaudeTurnError {
    #[error("claude turn misconfigured")]
    Configuration,
    #[error("claude stdio write failed")]
    StreamFailed,
}

/// How the spawned CLI session is identified.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ClaudeSession {
    /// A fresh native session opened with `--session-id`.
    Start(String),
    /// A resumed native session opened with `--resume` (later packet).
    Resume(String),
}

impl ClaudeSession {
    /// Returns the native session identity carried on the wire.
    pub(crate) fn session_id(&self) -> &str {
        match self {
            Self::Start(id) | Self::Resume(id) => id,
        }
    }
}

/// Mints a fresh native session identity (32 lowercase hex characters).
///
/// Returns `None` when operating-system entropy is unavailable; the caller
/// maps that to its entropy failure without touching the child.
pub(crate) fn new_session_id() -> Option<String> {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).ok()?;
    let mut id = String::with_capacity(32);
    for byte in bytes {
        id.push(HEX[(byte >> 4) as usize] as char);
        id.push(HEX[(byte & 0x0f) as usize] as char);
    }
    Some(id)
}

/// Typed Claude settings derived from the durable selection.
///
/// Mirrors `ResolveRunOptions` in `modules/engines/src/claude/cli-engine.ts`
/// through the native single-policy shape: the durable selection *is* the
/// policy, so `never` approval and `bypassPermissions` mode both take the
/// dangerous-bypass flag, `default`/`bypassPermissions` modes omit the
/// `--permission-mode` flag, and every other native mode passes through
/// verbatim. The append-system-prompt file option is a launch-time concern
/// and stays out of the durable selection by domain design.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ClaudeSettings {
    profile_id: String,
    model: Option<String>,
    dangerous_bypass: bool,
    permission_mode: Option<String>,
    disable_tools: bool,
    safe_mode: bool,
    effort: Option<String>,
}

impl ClaudeSettings {
    /// Derives typed spawn settings from the durable selection.
    ///
    /// # Errors
    ///
    /// Returns [`ClaudeTurnError::Configuration`] when the permission
    /// relations violate the Claude adapter contract (read-only or offline
    /// policies fail closed instead of seating a wrong permission mode).
    pub(crate) fn from_selection(selection: &ClaudeSelection) -> Result<Self, ClaudeTurnError> {
        if selection.permission().filesystem() == FilesystemAccess::None {
            return Err(ClaudeTurnError::Configuration);
        }
        if selection.permission().network() != NetworkAccess::Enabled {
            return Err(ClaudeTurnError::Configuration);
        }
        let dangerous_bypass = selection.permission().approval() == ApprovalMode::Never
            || selection.permission_mode() == Some(ClaudePermissionMode::BypassPermissions);
        let permission_mode = match selection.permission_mode() {
            None
            | Some(ClaudePermissionMode::Default)
            | Some(ClaudePermissionMode::BypassPermissions) => None,
            Some(mode) => Some(mode.as_str().to_owned()),
        };
        Ok(Self {
            profile_id: selection.profile_id().as_str().to_owned(),
            model: selection.model_id().map(|model| model.as_str().to_owned()),
            dangerous_bypass,
            permission_mode,
            disable_tools: selection.disable_tools(),
            safe_mode: selection.safe_mode(),
            effort: selection.effort().map(|effort| effort.as_str().to_owned()),
        })
    }

    /// Returns the managed profile identity.
    pub(crate) fn profile_id(&self) -> &str {
        &self.profile_id
    }

    /// Builds the exact `claude` argv for one session.
    ///
    /// Base flags mirror the TypeScript spawn (`-p`, stream-JSON stdio,
    /// `--permission-prompt-tool stdio`); `--thinking-display` is
    /// deliberately absent because continuation gating on the verified
    /// release is a later packet.
    pub(crate) fn spawn_args(&self, session: &ClaudeSession) -> Vec<String> {
        let mut args = vec![
            "-p".to_owned(),
            "--output-format".to_owned(),
            "stream-json".to_owned(),
            "--input-format".to_owned(),
            "stream-json".to_owned(),
            "--verbose".to_owned(),
            "--include-partial-messages".to_owned(),
            "--forward-subagent-text".to_owned(),
            "--permission-prompt-tool".to_owned(),
            "stdio".to_owned(),
        ];
        if self.dangerous_bypass {
            args.push("--dangerously-skip-permissions".to_owned());
        } else if let Some(mode) = self.permission_mode.as_deref() {
            args.push("--permission-mode".to_owned());
            args.push(mode.to_owned());
        }
        if self.disable_tools {
            args.push("--tools".to_owned());
            args.push(String::new());
        }
        if self.safe_mode {
            args.push("--safe-mode".to_owned());
        }
        if let Some(effort) = self.effort.as_deref() {
            args.push("--effort".to_owned());
            args.push(effort.to_owned());
        }
        match session {
            ClaudeSession::Start(id) => {
                args.push("--session-id".to_owned());
                args.push(id.clone());
            }
            ClaudeSession::Resume(id) => {
                args.push("--resume".to_owned());
                args.push(id.clone());
            }
        }
        if let Some(model) = self.model.as_deref() {
            args.push("--model".to_owned());
            args.push(model.to_owned());
        }
        args
    }

    /// Builds the first stdio user-message line for one session.
    ///
    /// Mirrors `ToUserMessage` in `cli-engine.ts` (text-only; image parts are
    /// a later packet).
    pub(crate) fn user_message_line(&self, session: &ClaudeSession, text: &str) -> String {
        user_message_line(session.session_id(), text)
    }
}

/// Builds one stdio user-message line (`stream-input` fold shape).
///
/// Steer-while-active reuses exactly this shape: the CLI folds stream-input
/// messages into the live turn on its own timing, so Artisan documents the
/// verb as experimental and never promises mid-turn interruption semantics.
pub(crate) fn user_message_line(session_id: &str, text: &str) -> String {
    serde_json::json!({
        "message": {
            "content": [{ "text": text, "type": "text" }],
            "role": "user",
        },
        "parent_tool_use_id": null,
        "session_id": session_id,
        "type": "user",
    })
    .to_string()
}

/// Builds one stdio permission response line (`control_response`).
///
/// Test-only until dispatcher delivery wiring lands.
#[cfg(test)]
pub(crate) fn approval_response_line(request_id: &str, approved: bool) -> String {
    serde_json::json!({
        "type": "control_response",
        "response": {
            "subtype": "success",
            "request_id": request_id,
            "response": { "behavior": if approved { "allow" } else { "deny" } },
        },
    })
    .to_string()
}

/// Builds one stdio question-answer line.
///
/// Claude collects answers through the permission response: the harness
/// allows the call and returns the same input amended with an `answers`
/// record keyed by question text, which the tool then reports as its result.
///
/// Test-only until dispatcher delivery wiring lands.
#[cfg(test)]
pub(crate) fn question_response_line(
    request_id: &str,
    input: &Value,
    answers: &[(String, String)],
) -> String {
    let mut amended = input.clone();
    let mut map = serde_json::Map::new();
    for (question_text, answer) in answers {
        map.insert(question_text.clone(), Value::String(answer.clone()));
    }
    if let Value::Object(object) = &mut amended {
        object.insert("answers".to_owned(), Value::Object(map));
    }
    serde_json::json!({
        "type": "control_response",
        "response": {
            "subtype": "success",
            "request_id": request_id,
            "response": { "behavior": "allow", "updatedInput": amended },
        },
    })
    .to_string()
}

/// One typed Claude approval request (permission request).
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ClaudeApprovalRequest {
    approval_id: String,
    description: String,
    command: Option<String>,
    cwd: Option<String>,
    reason: Option<String>,
    file_change: bool,
}

impl ClaudeApprovalRequest {
    /// Returns the provider approval identity.
    #[cfg(test)]
    pub(crate) fn approval_id(&self) -> &str {
        &self.approval_id
    }

    /// Returns the human description.
    #[cfg(test)]
    pub(crate) fn description(&self) -> &str {
        &self.description
    }

    /// Maps this request onto the domain approval vocabulary.
    ///
    /// The owner validates every request through this constructor before
    /// tracking it, so an out-of-bound provider frame never reaches the
    /// durable A-approve rows.
    ///
    /// # Errors
    ///
    /// Returns [`ClaudeTurnError::Configuration`] when the bounded fields
    /// violate domain ceilings.
    pub(crate) fn to_domain_request(&self) -> Result<ApprovalRequest, ClaudeTurnError> {
        if self.file_change {
            ApprovalRequest::file_change(self.reason.clone())
                .map_err(|_| ClaudeTurnError::Configuration)
        } else if let Some(command) = self.command.clone() {
            ApprovalRequest::command(command, self.cwd.clone(), self.reason.clone())
                .map_err(|_| ClaudeTurnError::Configuration)
        } else {
            ApprovalRequest::action(self.reason.clone()).map_err(|_| ClaudeTurnError::Configuration)
        }
    }
}

/// One offered answer to a Claude question.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ClaudeQuestionOption {
    label: String,
    description: Option<String>,
}

impl ClaudeQuestionOption {
    fn to_domain_option(&self) -> Result<QuestionOption, ClaudeTurnError> {
        QuestionOption::new(self.label.clone(), self.description.clone())
            .map_err(|_| ClaudeTurnError::Configuration)
    }
}

/// One typed Claude question (`AskUserQuestion` entry).
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ClaudeQuestion {
    question_id: String,
    text: String,
    header: Option<String>,
    multi_select: bool,
    options: Vec<ClaudeQuestionOption>,
}

impl ClaudeQuestion {
    /// Returns the provider question identity (`{request_id}:{index}`).
    pub(crate) fn question_id(&self) -> &str {
        &self.question_id
    }

    /// Returns the question text (the answer-record key).
    ///
    /// Test-only until dispatcher delivery wiring lands.
    #[cfg(test)]
    pub(crate) fn text(&self) -> &str {
        &self.text
    }

    /// Maps this question onto the domain question vocabulary.
    ///
    /// # Errors
    ///
    /// Returns [`ClaudeTurnError::Configuration`] when identities, text, or
    /// options violate domain ceilings.
    pub(crate) fn to_domain_input(&self) -> Result<QuestionInput, ClaudeTurnError> {
        let question_id = ObservationId::parse(self.question_id.clone())
            .map_err(|_| ClaudeTurnError::Configuration)?;
        if self.text.trim().is_empty() {
            return Err(ClaudeTurnError::Configuration);
        }
        let mut options = Vec::new();
        for option in &self.options {
            options.push(option.to_domain_option()?);
        }
        Ok(QuestionInput {
            question_id,
            text: self.text.clone(),
            header: self.header.clone(),
            multi_select: self.multi_select,
            options: if options.is_empty() {
                None
            } else {
                Some(options)
            },
        })
    }
}

/// One typed Claude question request frame (`AskUserQuestion` group).
///
/// Claude asks up to several questions in one request and its tool stays
/// blocked until every one of them has an answer; the group shares one
/// `request_id` and one verbatim input for the amended answer response.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ClaudeQuestionRequest {
    request_id: String,
    input: Value,
    questions: Vec<ClaudeQuestion>,
}

impl ClaudeQuestionRequest {
    /// Returns the provider request identity answered by the response line.
    ///
    /// Test-only until dispatcher delivery wiring lands.
    #[cfg(test)]
    pub(crate) fn request_id(&self) -> &str {
        &self.request_id
    }

    /// Returns the questions in this request group.
    pub(crate) fn questions(&self) -> &[ClaudeQuestion] {
        &self.questions
    }
}

/// Typed Claude stream event after bounded boundary decoding.
///
/// Every variant carries only validated identities and text; raw payloads
/// never leave the boundary. Bookkeeping frames (`compact_boundary`,
/// `api_retry`, hooks, stream lifecycle, opaque content deltas, `user` tool
/// results) stay observable as [`ClaudeEvent::Unknown`] without disturbing
/// the turn; their canonical projections are later packets.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ClaudeEvent {
    Init {
        session_id: String,
    },
    MessageStart {
        message_id: String,
    },
    TextDelta {
        delta: String,
        /// Verbatim TypeScript phase (`unspecified` or `commentary`).
        phase: &'static str,
        /// Per-response usage from the same assistant frame, if any disclosed
        /// a measurable sample. Projects best-effort beside the delta and
        /// never disturbs it.
        usage: Option<ClaudeUsageSample>,
    },
    /// Encrypted-thinking estimate: preserved in the tracker, never root text.
    ThinkingTokens {
        estimated_tokens: u64,
    },
    /// Non-empty thinking delta: counted in the tracker, never root text.
    ReasoningDelta,
    /// Buffered thinking block settled (possibly with empty encrypted text).
    ReasoningSettled {
        /// Per-response usage from the same assistant frame, if any.
        usage: Option<ClaudeUsageSample>,
    },
    /// Usage-only assistant frame: no public text, but the provider disclosed
    /// a measurable per-response sample, so the gauge still projects
    /// best-effort instead of being dropped with the bookkeeping.
    Usage {
        sample: ClaudeUsageSample,
    },
    ApprovalRequested(ClaudeApprovalRequest),
    QuestionRequested(ClaudeQuestionRequest),
    SubagentLifecycle {
        task_id: String,
    },
    ChildTranscript {
        parent_tool_use_id: String,
        /// Renderer-safe display text when the child frame carried any.
        text: Option<(String, &'static str)>,
    },
    TurnResult {
        success: bool,
        session_id: Option<String>,
        permission_denials: usize,
        /// Terminal usage totals from the same result frame, if any disclosed
        /// a measurable sample. Projects best-effort beside settlement and
        /// never disturbs it.
        usage: Option<ClaudeUsageSample>,
    },
    Unknown,
}

/// Failure decoding one bounded stream-JSON line.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ClaudeFrameError {
    TooLarge,
    InvalidJson,
    InvalidEnvelope,
}

fn bounded_id(value: &Value, field: &str) -> Option<String> {
    let text = value.get(field)?.as_str()?;
    if text.is_empty() || text.len() > CLAUDE_MAX_ID_BYTES {
        return None;
    }
    Some(text.to_owned())
}

fn bounded_text(value: &Value, field: &str) -> Option<String> {
    let text = value.get(field)?.as_str()?;
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed.len() > CLAUDE_MAX_TEXT_FIELD_BYTES {
        return None;
    }
    Some(trimmed.to_owned())
}

/// Decodes one bounded stream-JSON line into a typed event.
///
/// Oversized lines reject with [`ClaudeFrameError::TooLarge`]; invalid JSON
/// or envelopes reject with typed errors and never panic. Child transcript
/// frames route first (any object carrying `parent_tool_use_id`), mirroring
/// the TypeScript child-before-root routing. The frame sequence is accepted
/// for call-site ordering and does not cross into the event: native decision
/// identity travels as the validated request id.
pub(crate) fn parse_frame(
    line: &str,
    frame_sequence: u64,
) -> Result<ClaudeEvent, ClaudeFrameError> {
    let _ = frame_sequence;
    if line.len() > CLAUDE_MAX_FRAME_BYTES {
        return Err(ClaudeFrameError::TooLarge);
    }
    let envelope: Value = serde_json::from_str(line).map_err(|_| ClaudeFrameError::InvalidJson)?;
    let object = envelope
        .as_object()
        .ok_or(ClaudeFrameError::InvalidEnvelope)?;
    if let Some(parent) = object.get("parent_tool_use_id").and_then(Value::as_str) {
        if !parent.is_empty() && parent.len() <= CLAUDE_MAX_ID_BYTES {
            return Ok(ClaudeEvent::ChildTranscript {
                parent_tool_use_id: parent.to_owned(),
                text: child_display_text(&envelope),
            });
        }
        return Err(ClaudeFrameError::InvalidEnvelope);
    }
    // The current CLI emits its terminal summary without an envelope `type`;
    // it is recognized by its own terminal fields instead.
    if !object.contains_key("type") {
        if let Some(is_error) = object.get("is_error").and_then(Value::as_bool) {
            let permission_denials = match object.get("permission_denials") {
                Some(Value::Array(denials)) => denials.len(),
                _ => 0,
            };
            return Ok(ClaudeEvent::TurnResult {
                success: !is_error,
                session_id: object
                    .get("session_id")
                    .and_then(Value::as_str)
                    .filter(|id| !id.is_empty() && id.len() <= CLAUDE_MAX_ID_BYTES)
                    .map(str::to_owned),
                permission_denials,
                usage: object.get("usage").and_then(parse_claude_result_usage),
            });
        }
        return Err(ClaudeFrameError::InvalidEnvelope);
    }
    let kind = object
        .get("type")
        .and_then(Value::as_str)
        .ok_or(ClaudeFrameError::InvalidEnvelope)?;
    if kind.is_empty() || kind.len() > 128 {
        return Err(ClaudeFrameError::InvalidEnvelope);
    }
    Ok(decode_typed(kind, &envelope))
}

fn decode_typed(kind: &str, envelope: &Value) -> ClaudeEvent {
    match kind {
        "system" => decode_system(envelope),
        "stream_event" => decode_stream_event(envelope),
        "assistant" => decode_assistant(envelope),
        "user" => ClaudeEvent::Unknown,
        "result" => decode_result(envelope),
        "control_request" => decode_control(envelope),
        _ => ClaudeEvent::Unknown,
    }
}

/// Extracts renderer-safe display text from one child transcript envelope.
///
/// Reuses the root decoders and keeps only message text: approvals,
/// questions, results, reasoning, and bookkeeping never project into a child
/// transcript, following the S1a projection pattern. Empty text projects to
/// nothing.
fn child_display_text(envelope: &Value) -> Option<(String, &'static str)> {
    let kind = envelope.get("type")?.as_str()?;
    match decode_typed(kind, envelope) {
        ClaudeEvent::TextDelta { delta, phase, .. } if !delta.is_empty() => Some((delta, phase)),
        _ => None,
    }
}

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

fn decode_system(envelope: &Value) -> ClaudeEvent {
    let subtype = envelope
        .get("subtype")
        .and_then(Value::as_str)
        .unwrap_or("");
    match subtype {
        "init" => match bounded_id(envelope, "session_id") {
            Some(session_id) => ClaudeEvent::Init { session_id },
            None => ClaudeEvent::Unknown,
        },
        "task_started" | "task_progress" | "task_updated" | "task_notification" => {
            match bounded_id(envelope, "task_id") {
                Some(task_id) => ClaudeEvent::SubagentLifecycle { task_id },
                None => ClaudeEvent::Unknown,
            }
        }
        "thinking_tokens" => match envelope.get("estimated_tokens").and_then(Value::as_u64) {
            Some(estimated_tokens) => ClaudeEvent::ThinkingTokens { estimated_tokens },
            None => ClaudeEvent::Unknown,
        },
        _ => ClaudeEvent::Unknown,
    }
}

fn decode_stream_event(envelope: &Value) -> ClaudeEvent {
    let event = match envelope.get("event").and_then(Value::as_object) {
        Some(event) => event,
        None => return ClaudeEvent::Unknown,
    };
    let event_type = event.get("type").and_then(Value::as_str).unwrap_or("");
    match event_type {
        "content_block_delta" => {
            let delta = match event.get("delta").and_then(Value::as_object) {
                Some(delta) => delta,
                None => return ClaudeEvent::Unknown,
            };
            let delta_type = delta.get("type").and_then(Value::as_str).unwrap_or("");
            match delta_type {
                "text_delta" => match delta.get("text").and_then(Value::as_str) {
                    Some(text) => ClaudeEvent::TextDelta {
                        delta: text.to_owned(),
                        phase: "unspecified",
                        usage: None,
                    },
                    None => ClaudeEvent::Unknown,
                },
                "thinking_delta" => match delta.get("thinking").and_then(Value::as_str) {
                    Some(thinking) if !thinking.is_empty() => ClaudeEvent::ReasoningDelta,
                    _ => ClaudeEvent::Unknown,
                },
                _ => ClaudeEvent::Unknown,
            }
        }
        "message_start" => match event
            .get("message")
            .and_then(|message| message.get("id"))
            .and_then(Value::as_str)
        {
            Some(id) if !id.is_empty() && id.len() <= CLAUDE_MAX_ID_BYTES => {
                ClaudeEvent::MessageStart {
                    message_id: id.to_owned(),
                }
            }
            _ => ClaudeEvent::Unknown,
        },
        _ => ClaudeEvent::Unknown,
    }
}

fn decode_assistant(envelope: &Value) -> ClaudeEvent {
    let content = match envelope
        .get("message")
        .and_then(|message| message.get("content"))
        .and_then(Value::as_array)
    {
        Some(content) => content,
        None => return ClaudeEvent::Unknown,
    };
    let mut text_parts = Vec::new();
    let mut has_tool_use = false;
    let mut has_thinking = false;
    for item in content {
        let item_type = item.get("type").and_then(Value::as_str).unwrap_or("");
        match item_type {
            "text" => {
                if let Some(text) = item.get("text").and_then(Value::as_str) {
                    text_parts.push(text.to_owned());
                }
            }
            "tool_use" => {
                if item.get("id").and_then(Value::as_str).is_some() {
                    has_tool_use = true;
                }
            }
            "thinking" => {
                has_thinking = true;
            }
            _ => {}
        }
    }
    let message = text_parts.join("");
    // Per-response usage rides the same frame and projects beside whatever
    // else the frame carried; see `parse_claude_assistant_usage`.
    let usage = envelope
        .get("message")
        .and_then(|message| message.get("usage"))
        .and_then(parse_claude_assistant_usage);
    if !message.is_empty() {
        return ClaudeEvent::TextDelta {
            delta: message,
            phase: if has_tool_use {
                "commentary"
            } else {
                "unspecified"
            },
            usage,
        };
    }
    if has_thinking {
        return ClaudeEvent::ReasoningSettled { usage };
    }
    match usage {
        Some(sample) => ClaudeEvent::Usage { sample },
        None => ClaudeEvent::Unknown,
    }
}

fn decode_result(envelope: &Value) -> ClaudeEvent {
    let subtype = envelope
        .get("subtype")
        .and_then(Value::as_str)
        .unwrap_or("");
    if subtype.is_empty() {
        return ClaudeEvent::Unknown;
    }
    let is_error = envelope
        .get("is_error")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let permission_denials = match envelope.get("permission_denials") {
        Some(Value::Array(denials)) => denials.len(),
        _ => 0,
    };
    ClaudeEvent::TurnResult {
        success: subtype == "success" && !is_error,
        session_id: envelope
            .get("session_id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty() && id.len() <= CLAUDE_MAX_ID_BYTES)
            .map(str::to_owned),
        permission_denials,
        usage: envelope.get("usage").and_then(parse_claude_result_usage),
    }
}

fn decode_control(envelope: &Value) -> ClaudeEvent {
    let request_id = match envelope.get("request_id").and_then(Value::as_str) {
        Some(id) if !id.is_empty() && id.len() <= CLAUDE_MAX_ID_BYTES => id,
        _ => return ClaudeEvent::Unknown,
    };
    let request = match envelope.get("request").and_then(Value::as_object) {
        Some(request) => request,
        None => return ClaudeEvent::Unknown,
    };
    if request.get("subtype").and_then(Value::as_str) != Some("can_use_tool") {
        return ClaudeEvent::Unknown;
    }
    let tool_name = match request.get("tool_name").and_then(Value::as_str) {
        Some(name) if !name.is_empty() && name.len() <= 128 => name,
        _ => return ClaudeEvent::Unknown,
    };
    let input = match request.get("input") {
        Some(Value::Object(_)) => request.get("input").cloned().unwrap_or(Value::Null),
        _ => return ClaudeEvent::Unknown,
    };
    let request_value = Value::Object(request.clone());
    if tool_name == "AskUserQuestion" {
        if let Some(questions) = decode_questions(request_id, &input) {
            return ClaudeEvent::QuestionRequested(questions);
        }
        // A question request that carries no question is not answerable, so
        // it stays on the approval path rather than becoming a card with
        // nothing to choose.
    }
    let title = bounded_text(&request_value, "title");
    let description_value = bounded_text(&request_value, "description");
    let command = bounded_text(&input, "command");
    let cwd = bounded_text(&input, "cwd");
    let reason = title.clone().or_else(|| description_value.clone());
    let description = title
        .clone()
        .or_else(|| description_value.clone())
        .unwrap_or_else(|| format!("Claude requests permission for {tool_name}"));
    ClaudeEvent::ApprovalRequested(ClaudeApprovalRequest {
        approval_id: request_id.to_owned(),
        description,
        command: if tool_name == "Bash" || tool_name == "PowerShell" {
            command
        } else {
            None
        },
        cwd,
        reason,
        file_change: matches!(tool_name, "Edit" | "Write" | "MultiEdit" | "NotebookEdit"),
    })
}

fn decode_questions(request_id: &str, input: &Value) -> Option<ClaudeQuestionRequest> {
    let items = input.get("questions")?.as_array()?;
    let mut questions = Vec::new();
    for (index, item) in items
        .iter()
        .take(CLAUDE_MAX_QUESTIONS_PER_FRAME)
        .enumerate()
    {
        let text = bounded_text(item, "question")?;
        let header = bounded_text(item, "header");
        let multi_select = item
            .get("multiSelect")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let mut options = Vec::new();
        if let Some(raw_options) = item.get("options").and_then(Value::as_array) {
            for raw in raw_options.iter().take(CLAUDE_MAX_OPTIONS_PER_QUESTION) {
                let label = bounded_text(raw, "label")?;
                let description = bounded_text(raw, "description");
                options.push(ClaudeQuestionOption { label, description });
            }
        }
        questions.push(ClaudeQuestion {
            question_id: format!("{request_id}:{index}"),
            text: text.to_owned(),
            header,
            multi_select,
            options,
        });
    }
    if questions.is_empty() {
        return None;
    }
    Some(ClaudeQuestionRequest {
        request_id: request_id.to_owned(),
        input: input.clone(),
        questions,
    })
}

/// In-memory pending interaction tracker for one live Claude turn.
///
/// Permission requests land as pending approvals and `AskUserQuestion` frames
/// land as pending questions. Resolutions apply through the durable resolve
/// path; a deny records the decision with no turn side effect while the run
/// continues. Subagent discoveries emit validated `Discovered` rows and child
/// transcript frames project validated transcript rows; both accumulate for
/// the consumer drain without ever reaching the root turn. Thinking
/// estimates and reasoning settlement are retained as plumbing.
#[derive(Debug, Default)]
pub(crate) struct ClaudePendingTracker {
    approvals: HashMap<String, ClaudeApprovalRequest>,
    questions: HashMap<String, ClaudeQuestion>,
    subagents: Vec<String>,
    child_frames: Vec<(String, u64)>,
    subagent_rows: Vec<Observation>,
    thinking_tokens: Option<u64>,
    thinking_deltas: u64,
    reasoning_settled: bool,
    permission_denials: usize,
    stream_message_id: Option<String>,
    init_seen: bool,
    result_seen: bool,
    semantic_failure: bool,
    summary_title: Option<String>,
}

impl ClaudePendingTracker {
    /// Creates an empty tracker for one turn.
    pub(crate) fn new() -> Self {
        Self::default()
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

    /// Counts one non-empty thinking delta (never root text).
    pub(crate) fn note_reasoning_delta(&mut self) {
        self.thinking_deltas += 1;
    }

    /// Marks reasoning settled by a buffered thinking block, even when its
    /// text arrived empty (encrypted reasoning has no delta to complete).
    pub(crate) fn note_reasoning_settled(&mut self) {
        self.reasoning_settled = true;
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

    /// Returns whether reasoning settled without delta text.
    #[cfg(test)]
    pub(crate) fn reasoning_settled(&self) -> bool {
        self.reasoning_settled
    }

    /// Returns whether the init gate accepted the spawned session.
    #[cfg(test)]
    pub(crate) fn init_seen(&self) -> bool {
        self.init_seen
    }

    /// Returns how many non-empty thinking deltas were counted.
    #[cfg(test)]
    pub(crate) fn thinking_deltas(&self) -> u64 {
        self.thinking_deltas
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

/// Writes one stream-JSON line plus its terminator over the stdio transport.
pub(crate) async fn write_line<W: AsyncWrite + Unpin>(
    stdin: &mut W,
    line: &str,
) -> Result<(), ClaudeTurnError> {
    stdin
        .write_all(line.as_bytes())
        .await
        .map_err(|_| ClaudeTurnError::StreamFailed)?;
    stdin
        .write_all(b"\n")
        .await
        .map_err(|_| ClaudeTurnError::StreamFailed)?;
    stdin
        .flush()
        .await
        .map_err(|_| ClaudeTurnError::StreamFailed)?;
    Ok(())
}

/// Returns whether a silent active turn has stalled past its inactivity
/// deadline. Idle sessions between turns are silent by design and never
/// stall; only a turn already in flight owes output.
pub(crate) fn has_stalled(
    turn_active: bool,
    last_activity: Instant,
    inactivity: Duration,
    now: Instant,
) -> bool {
    turn_active && now.saturating_duration_since(last_activity) >= inactivity
}

// ---------------------------------------------------------------------------
// L3: native continuation gate, resume, usage, quota diagnostics, and title
// ---------------------------------------------------------------------------

/// Minimum Claude CLI for native continuation.
///
/// The transport floor and the continuation floor are the same verified
/// release (`2.1.220`): the launch authority already refuses older CLIs at
/// probe time, and this gate re-checks the recorded constant
/// ([`CLAUDE_NATIVE_CONTINUATION_VERSION`], mirroring
/// `claude_native_continuation_version` in
/// `modules/engines/src/claude/probe.ts`) so a stale capability can never
/// authorize a resume the installed CLI no longer honors.
pub(crate) const CLAUDE_CONTINUATION_MINIMUM_CLI_VERSION: &str = CLAUDE_NATIVE_CONTINUATION_VERSION;

/// Returns whether Claude teardown must terminate the whole process group.
///
/// Always true: the owner spawns Claude with whole-group custody (Job Object
/// on Windows), so teardown kills claude grandchildren that still hold pipes
/// instead of orphaning them. Unobserved reaps quarantine through the shared
/// `cleanup_after_abort` / `finish_turn_result` path.
pub(crate) const fn claude_requires_group_termination() -> bool {
    true
}

/// Compares two `X.Y.Z` CLI spellings by their numeric core.
///
/// A leading name and any trailing pre-release/build suffix are ignored, so
/// `2.1.220 (Claude Code)` compares equal to `2.1.220`. Returns `None` when
/// either side has no parseable triple; callers fail closed on `None`.
pub(crate) fn compare_claude_cli_versions(left: &str, right: &str) -> Option<std::cmp::Ordering> {
    Some(parse_cli_triple(left)?.cmp(&parse_cli_triple(right)?))
}

/// Returns whether a probed CLI version meets a minimum floor.
pub(crate) fn claude_cli_meets_minimum(version: &str, minimum: &str) -> bool {
    matches!(
        compare_claude_cli_versions(version, minimum),
        Some(std::cmp::Ordering::Equal | std::cmp::Ordering::Greater)
    )
}

fn parse_cli_triple(text: &str) -> Option<[u64; 3]> {
    let start = text.find(|character: char| character.is_ascii_digit())?;
    let run: String = text[start..]
        .chars()
        .take_while(|character| character.is_ascii_digit() || *character == '.')
        .collect();
    let mut parts = run.split('.');
    Some([
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
    ])
}

/// Native-continuation decision for one Claude turn.
///
/// `Compatible` authorizes `--resume` against the stored native session;
/// `Incompatible` carries the stable reason the dispatcher surfaces instead
/// of silently starting fresh or resuming across engines.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ClaudeContinuationDecision {
    Compatible,
    Incompatible { reason: &'static str },
}

/// Bounded input for [`check_claude_native_continuation`].
pub(crate) struct ClaudeContinuationGateInput<'a> {
    /// Probed CLI version (`VerifiedClaudeLaunch::version`).
    pub cli_version: &'a str,
    /// Explicit target model from the current selection; `None` fails closed.
    pub target_model: Option<&'a str>,
    /// Advertised models when a `model/list` inventory was read; `None`
    /// skips advertisement validation (live inventory is deferred) but never
    /// skips the explicit-model or CLI gates.
    pub advertised_models: Option<&'a [&'a str]>,
    /// Whether the stored binding names the same `claude` engine. The
    /// dispatcher scopes continuation reads to `EngineId::Claude`, so a false
    /// value fails closed instead of resuming across engines.
    pub same_engine: bool,
}

/// Gates one Claude native continuation without touching provider state.
///
/// Order is contractual: same-engine first, then the explicit target model
/// (pre-validated before resume), then the `2.1.220` CLI floor, then model
/// advertisement when an inventory is supplied. Any failure is a typed
/// incompatible — never a silent fresh start and never a cross-engine resume.
pub(crate) fn check_claude_native_continuation(
    input: &ClaudeContinuationGateInput<'_>,
) -> ClaudeContinuationDecision {
    if !input.same_engine {
        return ClaudeContinuationDecision::Incompatible {
            reason: "Claude native continuation cannot resume across engines",
        };
    }
    let Some(target) = input.target_model.filter(|model| !model.is_empty()) else {
        return ClaudeContinuationDecision::Incompatible {
            reason: "Claude native continuation requires an explicit target model",
        };
    };
    if !claude_cli_meets_minimum(input.cli_version, CLAUDE_CONTINUATION_MINIMUM_CLI_VERSION) {
        return ClaudeContinuationDecision::Incompatible {
            reason: "Claude native continuation requires CLI 2.1.220 or newer",
        };
    }
    if let Some(advertised) = input.advertised_models
        && !advertised.contains(&target)
    {
        return ClaudeContinuationDecision::Incompatible {
            reason: "Claude does not currently advertise the target model",
        };
    }
    ClaudeContinuationDecision::Compatible
}

/// Reopens the stored native session for one authorized continuation.
///
/// Mirrors the TypeScript open path (`--resume` with the stored session id
/// over the same flags a fresh start would use): the resume reopens
/// provider-owned state only and never invents checkpoints. Returns `None`
/// when the stored session id is outside its bounded route-segment grammar
/// so the caller fails closed instead of resuming a corrupt session. The
/// init gate then requires the CLI to announce exactly this session.
pub(crate) fn claude_resume_session(stored_session_id: &str) -> Option<ClaudeSession> {
    if stored_session_id.is_empty() || stored_session_id.len() > CLAUDE_MAX_ID_BYTES {
        return None;
    }
    Some(ClaudeSession::Resume(stored_session_id.to_owned()))
}

/// Cumulative usage sample from one frame's `usage` object.
///
/// Mirrors the TypeScript `UsageSchema` shape: terminal `result.usage`
/// totals carry the running counters, while an assistant frame's
/// per-response usage gauges the current window (see
/// [`parse_claude_assistant_usage`]).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct ClaudeUsageSample {
    pub input: Option<u64>,
    pub cached_input: Option<u64>,
    pub output: Option<u64>,
    /// Window gauge from one assistant response only — never a sum and never
    /// taken from terminal totals, which re-count the context on every model
    /// call. Absent stays absent rather than becoming a wrong zero.
    pub context: Option<u64>,
}

fn claude_token_number(usage: &serde_json::Map<String, Value>, field: &str) -> Option<u64> {
    usage.get(field).and_then(Value::as_u64)
}

/// Extracts terminal usage totals from a `result` frame's `usage` object.
///
/// Non-`u64` numerics (negatives, fractions) fail closed to absent for that
/// field; absent stays absent rather than becoming zero. Returns `None` when
/// no counter carries a value: an empty measurement is not a report. The
/// terminal totals never become a context gauge: they accumulate input across
/// every model call in the turn, re-counting the context each call resent.
pub(crate) fn parse_claude_result_usage(usage: &Value) -> Option<ClaudeUsageSample> {
    let object = usage.as_object()?;
    let sample = ClaudeUsageSample {
        input: claude_token_number(object, "input_tokens"),
        cached_input: claude_token_number(object, "cache_read_input_tokens"),
        output: claude_token_number(object, "output_tokens"),
        context: None,
    };
    if sample.input.is_none() && sample.cached_input.is_none() && sample.output.is_none() {
        return None;
    }
    Some(sample)
}

/// Extracts the per-response sample from an `assistant` frame's `usage`
/// object, including the context-window gauge.
///
/// The gauge is the response's input plus the cache reads and writes that
/// carried the prior conversation — what actually occupies the window right
/// now. Field rules match [`parse_claude_result_usage`].
pub(crate) fn parse_claude_assistant_usage(usage: &Value) -> Option<ClaudeUsageSample> {
    let object = usage.as_object()?;
    let input = claude_token_number(object, "input_tokens");
    let creation = claude_token_number(object, "cache_creation_input_tokens");
    let read = claude_token_number(object, "cache_read_input_tokens");
    let context = input.and_then(|tokens| {
        tokens
            .checked_add(creation.unwrap_or(0))?
            .checked_add(read.unwrap_or(0))
    });
    let sample = ClaudeUsageSample {
        input,
        cached_input: read,
        output: claude_token_number(object, "output_tokens"),
        context,
    };
    if sample.input.is_none()
        && sample.cached_input.is_none()
        && sample.output.is_none()
        && sample.context.is_none()
    {
        return None;
    }
    Some(sample)
}

/// Immutable attribution for one Claude usage report.
///
/// Model and thread come from the immutable launch snapshot; the provider
/// session is the authenticated native session, never an envelope claim.
pub(crate) struct ClaudeUsageContext<'a> {
    pub run_id: &'a RunId,
    pub thread_id: &'a ThreadId,
    pub provider_session_id: &'a str,
    pub model_id: &'a EngineModelId,
    pub observed_at: UnixMillis,
}

/// Best-effort usage scope carried beside the text channel.
///
/// `None` (no explicit model or no thread scope) skips usage projection
/// without disturbing the turn: usage never blocks turns.
#[derive(Clone, Debug)]
pub(crate) struct ClaudeUsageAttribution {
    pub thread_id: ThreadId,
    pub model_id: EngineModelId,
}

/// Borrowed usage scope for one pump loop.
pub(crate) struct ClaudeUsageScope<'a> {
    pub thread_id: &'a ThreadId,
    pub model_id: &'a EngineModelId,
    pub provider_session_id: &'a str,
}

/// Builds the cumulative usage report for one sample.
///
/// Fails closed (`None`) when identities or bounds reject: usage is never
/// synthesized from partial identities. The context gauge always replaces
/// the previous report regardless of basis — the codec performs no
/// arithmetic at all. Claude names no provider route, so reports attribute
/// to the engine's own `claude` route namespace; quota windows are never
/// copied here, so no quota is invented. Claude discloses no provider turn
/// identity on usage frames, so none is attributed rather than synthesized.
pub(crate) fn claude_usage_report(
    context: &ClaudeUsageContext<'_>,
    source_sequence: u64,
    sample: &ClaudeUsageSample,
) -> Option<RunUsageReport> {
    let provider_route_id = EngineRouteId::parse("claude").ok()?;
    RunUsageReport::new(RunUsageReportInput {
        run_id: context.run_id.clone(),
        thread_id: context.thread_id.clone(),
        provider_session_id: context.provider_session_id.to_owned(),
        source_sequence,
        model_id: context.model_id.clone(),
        provider_route_id,
        variant_id: None,
        basis: RunUsageBasis::Cumulative,
        provider_turn_id: None,
        input_tokens: sample.input,
        cached_input_tokens: sample.cached_input,
        output_tokens: sample.output,
        context_tokens: sample.context,
        context_window_tokens: None,
        observed_at: context.observed_at,
    })
    .ok()
}

fn current_unix_millis() -> Option<UnixMillis> {
    use std::time::{SystemTime, UNIX_EPOCH};
    let duration = SystemTime::now().duration_since(UNIX_EPOCH).ok()?;
    let millis = i64::try_from(duration.as_millis()).ok()?;
    Some(UnixMillis::from_millis(millis))
}

/// Projects one usage sample best-effort onto the shared usage vocabulary.
///
/// Returns `Some(TerminalState::Interrupted)` only when the observation sink
/// closed mid-send. Skipping (no scope, no clock, or unattributable sample)
/// is never terminal: usage never blocks turns.
async fn project_usage_sample(
    observations: &mpsc::Sender<EngineObservation>,
    run_id: &RunId,
    scope: Option<&ClaudeUsageScope<'_>>,
    source_sequence: u64,
    sample: &ClaudeUsageSample,
) -> Option<TerminalState> {
    let scope = scope?;
    let observed_at = current_unix_millis()?;
    let report = claude_usage_report(
        &ClaudeUsageContext {
            run_id,
            thread_id: scope.thread_id,
            provider_session_id: scope.provider_session_id,
            model_id: scope.model_id,
            observed_at,
        },
        source_sequence,
        sample,
    )?;
    if observations
        .send(EngineObservation::Usage(UsageObservation::new(report)))
        .await
        .is_err()
    {
        return Some(TerminalState::Interrupted);
    }
    None
}

/// Kind of one Claude quota window, classified from its provider window.
///
/// Mirrors `parse_claude_cli_usage_windows` in
/// `modules/engines/src/claude/usage.ts`: 300 minutes is a session window,
/// 10,080 a weekly window; anything else is unknown rather than guessed.
/// Claude names no monthly bucket.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ClaudeQuotaWindowKind {
    Session,
    Weekly,
    Unknown,
}

/// Classifies one quota window duration.
pub(crate) fn classify_claude_quota_window_kind(
    window_minutes: Option<u64>,
) -> ClaudeQuotaWindowKind {
    match window_minutes {
        Some(300) => ClaudeQuotaWindowKind::Session,
        Some(10_080) => ClaudeQuotaWindowKind::Weekly,
        _ => ClaudeQuotaWindowKind::Unknown,
    }
}

/// Clamps one percent reading into `0..=100`.
///
/// Absent or non-finite readings become `0`: usage display never blocks on a
/// corrupt gauge and never invents quota from it.
pub(crate) fn clamp_claude_percent_used(used_percent: Option<f64>) -> f64 {
    match used_percent {
        Some(value) if value.is_finite() => value.clamp(0.0, 100.0),
        _ => 0.0,
    }
}

/// The exact non-billable CLI invocation that reads account usage.
///
/// Mirrors `claude_cli_usage_args` in `modules/engines/src/claude/usage.ts`
/// (`-p /usage` over JSON): the slash command travels in argv and stdin
/// closes immediately, so no prompt is ever billed.
pub(crate) fn claude_cli_usage_args() -> [&'static str; 4] {
    ["-p", "/usage", "--output-format", "json"]
}

/// One provider-neutral Claude quota window: diagnostics only, never quota.
///
/// Quota windows are read through the non-billable `/usage` surface and
/// classified here; they are never copied into [`RunUsageReport`] and never
/// gate a turn.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ClaudeQuotaWindow {
    pub id: String,
    pub kind: ClaudeQuotaWindowKind,
    pub label: Option<String>,
    pub percent_used: f64,
    pub resets_at: Option<String>,
    pub window_minutes: Option<u64>,
    /// `"shared"` for the session and all-models weekly buckets, `"model"`
    /// for per-model weekly buckets. Mirrors the TypeScript scope rule
    /// without inventing quota attribution.
    pub scope: &'static str,
}

/// Turns a provider-supplied weekly label into a stable id fragment.
///
/// Mirrors `slugify_claude_cli_label` in `modules/engines/src/claude/usage.ts`:
/// lowercase, non-alphanumeric runs become one dash, edge dashes trimmed.
pub(crate) fn slugify_claude_cli_label(label: &str) -> String {
    let mut slug = String::with_capacity(label.len());
    let mut pending_dash = false;
    for character in label.chars().flat_map(char::to_lowercase) {
        if character.is_ascii_alphanumeric() {
            if pending_dash && !slug.is_empty() {
                slug.push('-');
            }
            pending_dash = false;
            slug.push(character);
        } else if !slug.is_empty() || pending_dash {
            pending_dash = true;
        }
    }
    slug
}

const CLAUDE_CLI_MONTHS: [(&str, i64); 12] = [
    ("jan", 1),
    ("feb", 2),
    ("mar", 3),
    ("apr", 4),
    ("may", 5),
    ("jun", 6),
    ("jul", 7),
    ("aug", 8),
    ("sep", 9),
    ("oct", 10),
    ("nov", 11),
    ("dec", 12),
];

fn claude_cli_month(name: &str) -> Option<i64> {
    let prefix: String = name.chars().take(3).flat_map(char::to_lowercase).collect();
    CLAUDE_CLI_MONTHS
        .iter()
        .find(|(month, _)| *month == prefix)
        .map(|(_, number)| *number)
}

/// Converts a civil date to days since the Unix epoch (Howard Hinnant's
/// algorithm), mirroring the epoch math behind the TypeScript reset parse
/// without a date dependency.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let adjusted_year = if month <= 2 { year - 1 } else { year };
    let era = adjusted_year.div_euclid(400);
    let year_of_era = adjusted_year.rem_euclid(400);
    let month_index = (month + 9) % 12;
    let day_of_year = (153 * month_index + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// Converts days since the Unix epoch back to a civil date.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_pair = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_pair + 2) / 5 + 1;
    let month = if month_pair < 10 {
        month_pair + 3
    } else {
        month_pair - 9
    };
    (if month <= 2 { year + 1 } else { year }, month, day)
}

/// Parses the provider's English wall-clock reset clause to a UTC instant.
///
/// Mirrors `parse_claude_cli_reset_at` in `modules/engines/src/claude/usage.ts`
/// (`resets <Mon> <D>, <H>[:<MM>] <am|pm> (<Zone>)` at end of line). Only
/// `UTC`/`GMT`/`UT` zones resolve to a real instant: named IANA zones need a
/// zone database this owner does not carry, so they stay `None` rather than
/// becoming an invented instant. `at_ms` is the caller-observed now used for
/// year inference (December rolling into a January reset).
pub(crate) fn parse_claude_cli_reset_at(line: &str, at_ms: i64) -> Option<String> {
    // Scan left to right for the first `resets <clause>` that fully parses,
    // mirroring the unanchored regex scan in the TypeScript reader. The scan
    // is ASCII-boundary safe: `resets ` is pure ASCII and the cursor always
    // rests on a character boundary.
    let bytes = line.as_bytes();
    let mut index = 0;
    while index + 7 <= bytes.len() {
        if bytes[index..index + 7].eq_ignore_ascii_case(b"resets ")
            && (index == 0 || !bytes[index - 1].is_ascii_alphanumeric())
            && let Some(instant) = parse_reset_clause(&line[index + 7..], at_ms)
        {
            return Some(instant);
        }
        index += line[index..].chars().next()?.len_utf8();
    }
    None
}

fn parse_reset_clause(clause: &str, at_ms: i64) -> Option<String> {
    let clause = clause.trim_end();
    let (date_part, zone_part) = clause.rsplit_once('(')?;
    let zone = zone_part.strip_suffix(')')?;
    if !zone.eq_ignore_ascii_case("UTC")
        && !zone.eq_ignore_ascii_case("GMT")
        && !zone.eq_ignore_ascii_case("UT")
    {
        return None;
    }
    let mut tokens = date_part.split_whitespace();
    let month_token = tokens.next()?;
    if !month_token
        .chars()
        .all(|character| character.is_ascii_alphabetic())
    {
        return None;
    }
    let day_token = tokens.next()?.strip_suffix(',')?;
    let month = claude_cli_month(month_token)?;
    let day: i64 = day_token.parse().ok()?;
    let (hour_token, minute_token, meridiem_token) = match (tokens.next(), tokens.next()) {
        (Some(time), Some(meridiem)) => {
            let (hour, minute) = match time.split_once(':') {
                Some((hour, minute)) => {
                    if minute.len() != 2 {
                        return None;
                    }
                    (hour, Some(minute))
                }
                None => (time, None),
            };
            (hour, minute, meridiem)
        }
        _ => return None,
    };
    if tokens.next().is_some() {
        return None;
    }
    let hour12: i64 = hour_token.parse().ok()?;
    let minute: i64 = match minute_token {
        Some(text) => text.parse().ok()?,
        None => 0,
    };
    let meridiem = meridiem_token.to_ascii_lowercase();
    if day < 1 || day > 31 || hour12 < 1 || hour12 > 12 || minute < 0 || minute > 59 {
        return None;
    }
    let hour = match meridiem.as_str() {
        "am" => hour12 % 12,
        "pm" => hour12 % 12 + 12,
        _ => return None,
    };
    let (current_year, current_month, _) = civil_from_days(at_ms.div_euclid(86_400_000));
    let year = current_year + i64::from(current_month == 12 && month == 1);
    if civil_from_days(days_from_civil(year, month, day)) != (year, month, day) {
        return None;
    }
    Some(format!(
        "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:00Z"
    ))
}

fn claude_usage_bucket_window(
    id: String,
    label: Option<String>,
    percent_text: &str,
    line: &str,
    at_ms: i64,
    scope: &'static str,
    window_minutes: u64,
) -> ClaudeQuotaWindow {
    ClaudeQuotaWindow {
        id,
        kind: classify_claude_quota_window_kind(Some(window_minutes)),
        label,
        percent_used: clamp_claude_percent_used(percent_text.parse::<f64>().ok()),
        resets_at: parse_claude_cli_reset_at(line, at_ms),
        window_minutes: Some(window_minutes),
        scope,
    }
}

fn match_percent_tail(text: &str) -> Option<&str> {
    let end = text
        .find(|character: char| !character.is_ascii_digit())
        .unwrap_or(text.len());
    if end == 0 {
        return None;
    }
    let (digits, rest) = (&text[..end], &text[end..]);
    let rest = rest.strip_prefix('%')?;
    let rest = rest.trim_start();
    let after_used = rest.strip_prefix("used")?;
    if after_used
        .chars()
        .next()
        .is_some_and(|character| character.is_ascii_alphanumeric() || character == '_')
    {
        return None;
    }
    Some(digits)
}

/// Parses provider-owned `/usage` CLI text into quota windows.
///
/// Mirrors `parse_claude_cli_usage_windows` in
/// `modules/engines/src/claude/usage.ts` without retaining credentials or raw
/// account data: one `five_hour` session window, one shared `seven_day`
/// weekly window, plus one per-model `seven_day:<slug>` weekly window.
/// Duplicate ids keep the first row; malformed lines yield nothing.
pub(crate) fn parse_claude_cli_usage_windows(
    result_text: &str,
    at_ms: i64,
) -> Vec<ClaudeQuotaWindow> {
    let mut windows: Vec<ClaudeQuotaWindow> = Vec::new();
    for raw_line in result_text.split('\n') {
        let line = raw_line.trim();
        if let Some(rest) = line.strip_prefix("Current session:") {
            let rest = rest.trim_start();
            if let Some(percent) = match_percent_tail(rest) {
                push_quota_window(
                    &mut windows,
                    claude_usage_bucket_window(
                        "five_hour".to_owned(),
                        None,
                        percent,
                        line,
                        at_ms,
                        "shared",
                        300,
                    ),
                );
            }
            continue;
        }
        // The all-models bucket is checked before the labeled pattern below.
        if let Some(rest) = line.strip_prefix("Current week (all models):") {
            let rest = rest.trim_start();
            if let Some(percent) = match_percent_tail(rest) {
                push_quota_window(
                    &mut windows,
                    claude_usage_bucket_window(
                        "seven_day".to_owned(),
                        None,
                        percent,
                        line,
                        at_ms,
                        "shared",
                        10_080,
                    ),
                );
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix("Current week (") {
            let Some(close) = rest.find(')') else {
                continue;
            };
            // The labeled shape requires the exact `(<Label>): N% used`
            // form: the colon follows the parenthesis immediately.
            let label = &rest[..close];
            if label.is_empty() {
                continue;
            }
            let Some(after) = rest[close + 1..].strip_prefix(':') else {
                continue;
            };
            let after = after.trim_start();
            if let Some(percent) = match_percent_tail(after) {
                let slug = slugify_claude_cli_label(label);
                push_quota_window(
                    &mut windows,
                    claude_usage_bucket_window(
                        format!("seven_day:{slug}"),
                        Some(label.to_owned()),
                        percent,
                        line,
                        at_ms,
                        "model",
                        10_080,
                    ),
                );
            }
        }
    }
    windows
}

fn push_quota_window(windows: &mut Vec<ClaudeQuotaWindow>, window: ClaudeQuotaWindow) {
    if windows.iter().any(|known| known.id == window.id) {
        return;
    }
    windows.push(window);
}

/// The directory Claude Code files a working directory's transcripts under.
///
/// Mirrors `claude_project_directory_name` in
/// `modules/engines/src/claude/session-title.ts`: every character outside
/// `[A-Za-z0-9]` becomes a dash, drive colon and path separators included.
pub(crate) fn claude_project_directory_name(working_directory: &str) -> String {
    working_directory
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                '-'
            }
        })
        .collect()
}

/// The session transcript's path inside one Claude config home.
///
/// Mirrors `claude_session_transcript_path` in
/// `modules/engines/src/claude/session-title.ts`.
pub(crate) fn claude_session_transcript_path(
    home: &str,
    working_directory: &str,
    session_id: &str,
) -> PathBuf {
    Path::new(home)
        .join("projects")
        .join(claude_project_directory_name(working_directory))
        .join(format!("{session_id}.jsonl"))
}

/// Maximum transcript bytes read for a generated title.
///
/// Mirrors `maximum_transcript_bytes` in
/// `modules/engines/src/claude/session-title.ts`: settles are seconds apart
/// at their fastest, so the read is rare, but an unbounded read of a runaway
/// transcript would trade a nicety for memory pressure.
pub(crate) const CLAUDE_MAX_TRANSCRIPT_BYTES: u64 = 64 * 1024 * 1024;

/// Environment variable naming the Claude config home whose transcripts carry
/// generated titles. Mirrors the TypeScript spawn-override resolution
/// (`CLAUDE_CONFIG_DIR` over the ambient CLI default).
pub(crate) const CLAUDE_CONFIG_DIR_ENV_VAR: &str = "CLAUDE_CONFIG_DIR";

/// Returns the newest generated title across transcript lines, if any.
///
/// Mirrors `claude_session_title_from_lines` in
/// `modules/engines/src/claude/session-title.ts`: the CLI appends its
/// model-written title as `ai-title` records within the first turn and again
/// as the conversation evolves, and resolves the current name by taking the
/// newest — so this reader does the same. Malformed records are skipped, and
/// titles outside the domain title bound never become observations.
pub(crate) fn claude_session_title_from_lines(lines: &[&str]) -> Option<String> {
    for line in lines.iter().rev() {
        if !line.contains("ai-title") {
            continue;
        }
        let Ok(record) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if record.get("type").and_then(Value::as_str) != Some("ai-title") {
            continue;
        }
        let Some(title) = record.get("aiTitle").and_then(Value::as_str) else {
            continue;
        };
        let title = title.trim();
        if title.is_empty() || title.len() > OBSERVATION_TITLE_MAX_BYTES {
            continue;
        }
        return Some(title.to_owned());
    }
    None
}

/// Reads the newest generated title from one session transcript.
///
/// Deliberately total: a missing transcript, an unreadable file, an oversize
/// file, or malformed records all mean "no title yet" — a run must never
/// fail, or even complain, because a nicety could not be read.
pub(crate) fn read_claude_session_title(transcript_path: &Path) -> Option<String> {
    let size = std::fs::metadata(transcript_path).ok()?.len();
    if size > CLAUDE_MAX_TRANSCRIPT_BYTES {
        return None;
    }
    let text = std::fs::read_to_string(transcript_path).ok()?;
    if u64::try_from(text.len()).ok()? > CLAUDE_MAX_TRANSCRIPT_BYTES {
        return None;
    }
    let lines: Vec<&str> = text.lines().collect();
    claude_session_title_from_lines(&lines)
}

/// Captures the generated title for one native session, if the managed config
/// home names a readable transcript for it.
///
/// Best-effort beside terminal settlement: any failure means "no title yet".
/// Only the managed `CLAUDE_CONFIG_DIR` override is consulted — ambient home
/// resolution stays with the CLI until a home-directory source exists.
pub(crate) fn claude_transcript_title_for_session(
    project_root: &RootPath,
    session_id: &str,
) -> Option<String> {
    let home = std::env::var(CLAUDE_CONFIG_DIR_ENV_VAR)
        .ok()
        .filter(|value| !value.trim().is_empty())?;
    let path = claude_session_transcript_path(home.trim(), project_root.as_str(), session_id);
    read_claude_session_title(&path)
}

/// Applies one typed event; returns how the pump continues.
///
/// Text deltas chunk onto the shared vocabulary with the verbatim phase
/// carried on the event; the current stream message id (when announced)
/// becomes the explicit part identity so one message and its completion stay
/// grouped. Usage samples project best-effort onto the shared usage
/// vocabulary when a usage scope travels with the pump; without one (or on
/// any attribution failure) they are diagnostics that never disturb the turn.
/// Usage collection never blocks turns: only the observation sink closing is
/// terminal. Session identity mismatches fail the turn closed: only the exact
/// spawned session may speak for it.
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
    match event {
        ClaudeEvent::Init { session_id } => {
            if session_id != expected_session {
                return ClaudeApplyOutcome::Terminal(TerminalState::Failed);
            }
            tracker.init_seen = true;
            ClaudeApplyOutcome::Continue { end_input: false }
        }
        ClaudeEvent::MessageStart { message_id } => {
            tracker.stream_message_id = Some(message_id);
            ClaudeApplyOutcome::Continue { end_input: false }
        }
        ClaudeEvent::TextDelta {
            delta,
            phase,
            usage: sample,
        } => {
            // Both verbatim phases fold to the shared delta vocabulary here;
            // the phase stays on the typed event for later routing packets.
            let _ = phase;
            if active_turn.is_none() {
                *active_turn = Some(expected_session.to_owned());
            }
            let native_id = format!("claude:{frame_sequence}");
            let part_id = tracker.stream_message_id.clone();
            for chunk in chunk_text(run_id, frame_sequence, &native_id, &delta) {
                let chunk = match part_id.clone() {
                    Some(part) => chunk.with_part_id(part),
                    None => chunk,
                };
                if observations
                    .send(EngineObservation::TextDelta(chunk))
                    .await
                    .is_err()
                {
                    return ClaudeApplyOutcome::Terminal(TerminalState::Interrupted);
                }
            }
            if let Some(sample) = sample.as_ref() {
                if let Some(state) =
                    project_usage_sample(observations, run_id, usage, frame_sequence, sample).await
                {
                    return ClaudeApplyOutcome::Terminal(state);
                }
            }
            ClaudeApplyOutcome::Continue { end_input: false }
        }
        ClaudeEvent::ThinkingTokens { estimated_tokens } => {
            tracker.note_thinking_tokens(estimated_tokens);
            ClaudeApplyOutcome::Continue { end_input: false }
        }
        ClaudeEvent::ReasoningDelta => {
            tracker.note_reasoning_delta();
            ClaudeApplyOutcome::Continue { end_input: false }
        }
        ClaudeEvent::ReasoningSettled { usage: sample } => {
            tracker.note_reasoning_settled();
            if let Some(sample) = sample.as_ref() {
                if let Some(state) =
                    project_usage_sample(observations, run_id, usage, frame_sequence, sample).await
                {
                    return ClaudeApplyOutcome::Terminal(state);
                }
            }
            ClaudeApplyOutcome::Continue { end_input: false }
        }
        ClaudeEvent::Usage { sample } => {
            if let Some(state) =
                project_usage_sample(observations, run_id, usage, frame_sequence, &sample).await
            {
                return ClaudeApplyOutcome::Terminal(state);
            }
            ClaudeApplyOutcome::Continue { end_input: false }
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
            if let Some(session_id) = session_id {
                if session_id != expected_session {
                    return ClaudeApplyOutcome::Terminal(TerminalState::Failed);
                }
            }
            tracker.result_seen = true;
            if !success {
                tracker.semantic_failure = true;
            }
            tracker.note_permission_denials(permission_denials);
            if let Some(sample) = sample.as_ref() {
                if let Some(state) =
                    project_usage_sample(observations, run_id, usage, frame_sequence, sample).await
                {
                    return ClaudeApplyOutcome::Terminal(state);
                }
            }
            ClaudeApplyOutcome::Continue { end_input: true }
        }
        ClaudeEvent::Unknown => ClaudeApplyOutcome::Continue { end_input: false },
    }
}

/// Steers a live turn with follow-up text (stream-input fold).
///
/// Test-only until dispatcher steer wiring lands: proves the fold verb
/// against the fixture stdio script without disturbing the authorize-once
/// production flow. Experimental per the adapter: the CLI owns fold timing.
///
/// # Errors
///
/// Returns [`ClaudeTurnError`] when the write fails.
#[cfg(test)]
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
