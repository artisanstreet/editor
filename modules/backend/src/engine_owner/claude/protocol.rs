use std::time::Duration;

use artisan_domain::{ApprovalRequest, ObservationId, QuestionInput, QuestionOption};
use serde_json::Value;
use thiserror::Error;
use tokio::io::{AsyncWrite, AsyncWriteExt};
use tokio::time::Instant;

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
pub(crate) const CLAUDE_MAX_ID_BYTES: usize = 256;

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
    pub(crate) approval_id: String,
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
    pub(crate) input: Value,
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
            let usage = match object.get("usage") {
                None => None,
                Some(value) => match parse_claude_result_usage(value) {
                    Ok(sample) => sample,
                    Err(_) => return Ok(ClaudeEvent::Unknown),
                },
            };
            return Ok(ClaudeEvent::TurnResult {
                success: !is_error,
                session_id: object
                    .get("session_id")
                    .and_then(Value::as_str)
                    .filter(|id| !id.is_empty() && id.len() <= CLAUDE_MAX_ID_BYTES)
                    .map(str::to_owned),
                permission_denials,
                usage,
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
    let Some(event) = envelope.get("event").and_then(Value::as_object) else {
        return ClaudeEvent::Unknown;
    };
    let event_type = event.get("type").and_then(Value::as_str).unwrap_or("");
    match event_type {
        "content_block_delta" => {
            let Some(delta) = event.get("delta").and_then(Value::as_object) else {
                return ClaudeEvent::Unknown;
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
    let Some(content) = envelope
        .get("message")
        .and_then(|message| message.get("content"))
        .and_then(Value::as_array)
    else {
        return ClaudeEvent::Unknown;
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
    // else the frame carried; see `parse_claude_assistant_usage`. A corrupt
    // usage value poisons the frame exactly like the TypeScript schema
    // decode failing: nothing canonical is emitted from it.
    let usage = match envelope
        .get("message")
        .and_then(|message| message.get("usage"))
    {
        None => None,
        Some(value) => match parse_claude_assistant_usage(value) {
            Ok(sample) => sample,
            Err(_) => return ClaudeEvent::Unknown,
        },
    };
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
    // Corrupt totals poison the frame exactly like the TypeScript schema
    // decode failing: the turn never settles on corrupt provider numbers.
    let usage = match envelope.get("usage") {
        None => None,
        Some(value) => match parse_claude_result_usage(value) {
            Ok(sample) => sample,
            Err(_) => return ClaudeEvent::Unknown,
        },
    };
    ClaudeEvent::TurnResult {
        success: subtype == "success" && !is_error,
        session_id: envelope
            .get("session_id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty() && id.len() <= CLAUDE_MAX_ID_BYTES)
            .map(str::to_owned),
        permission_denials,
        usage,
    }
}

fn decode_control(envelope: &Value) -> ClaudeEvent {
    let request_id = match envelope.get("request_id").and_then(Value::as_str) {
        Some(id) if !id.is_empty() && id.len() <= CLAUDE_MAX_ID_BYTES => id,
        _ => return ClaudeEvent::Unknown,
    };
    let Some(request) = envelope.get("request").and_then(Value::as_object) else {
        return ClaudeEvent::Unknown;
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
    if tool_name == "AskUserQuestion"
        && let Some(questions) = decode_questions(request_id, &input)
    {
        return ClaudeEvent::QuestionRequested(questions);
    }
    // A question request that carries no question is not answerable, so
    // it stays on the approval path rather than becoming a card with
    // nothing to choose.
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
            text: text.clone(),
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

fn claude_token_field(
    usage: &serde_json::Map<String, Value>,
    field: &str,
) -> Result<Option<u64>, ClaudeUsageCorrupt> {
    match usage.get(field) {
        None => Ok(None),
        Some(value) => value.as_u64().map(Some).ok_or(ClaudeUsageCorrupt),
    }
}

/// Marker: one frame's `usage` value was present but uninterpretable.
///
/// A present-but-corrupt usage object poisons its whole frame (which then
/// decodes `Unknown` at every entry point): the TypeScript adapter fails the
/// frame's schema decode the same way, so no text, gauge, or terminal ever
/// settles on corrupt provider numbers. Absent or empty usage stays
/// `Ok(None)` and never disturbs its frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ClaudeUsageCorrupt;

/// Extracts terminal usage totals from a `result` frame's `usage` object.
///
/// A non-object value or a present field outside `u64` (string, negative,
/// fraction, boolean, null, or container) fails the whole sample closed:
/// corrupt provider numbers never become a report and the frame carries
/// none. Absent stays absent rather than becoming zero, and an empty
/// measurement (`Ok(None)`) is not a report. The terminal totals never
/// become a context gauge: they accumulate input across every model call in
/// the turn, re-counting the context each call resent.
///
/// # Errors
///
/// Returns [`ClaudeUsageCorrupt`] when the usage value is present but
/// uninterpretable.
pub(crate) fn parse_claude_result_usage(
    usage: &Value,
) -> Result<Option<ClaudeUsageSample>, ClaudeUsageCorrupt> {
    let object = usage.as_object().ok_or(ClaudeUsageCorrupt)?;
    let sample = ClaudeUsageSample {
        input: claude_token_field(object, "input_tokens")?,
        cached_input: claude_token_field(object, "cache_read_input_tokens")?,
        output: claude_token_field(object, "output_tokens")?,
        context: None,
    };
    Ok(
        if sample.input.is_none() && sample.cached_input.is_none() && sample.output.is_none() {
            None
        } else {
            Some(sample)
        },
    )
}

/// Extracts the per-response sample from an `assistant` frame's `usage`
/// object, including the context-window gauge.
///
/// The gauge is the response's input plus the cache reads and writes that
/// carried the prior conversation — what actually occupies the window right
/// now. Corruption rules match [`parse_claude_result_usage`]: a present but
/// uninterpretable value fails the whole sample closed.
///
/// # Errors
///
/// Returns [`ClaudeUsageCorrupt`] when the usage value is present but
/// uninterpretable.
pub(crate) fn parse_claude_assistant_usage(
    usage: &Value,
) -> Result<Option<ClaudeUsageSample>, ClaudeUsageCorrupt> {
    let object = usage.as_object().ok_or(ClaudeUsageCorrupt)?;
    let input = claude_token_field(object, "input_tokens")?;
    let creation = claude_token_field(object, "cache_creation_input_tokens")?;
    let read = claude_token_field(object, "cache_read_input_tokens")?;
    let context = input.and_then(|tokens| {
        tokens
            .checked_add(creation.unwrap_or(0))?
            .checked_add(read.unwrap_or(0))
    });
    let sample = ClaudeUsageSample {
        input,
        cached_input: read,
        output: claude_token_field(object, "output_tokens")?,
        context,
    };
    Ok(
        if sample.input.is_none()
            && sample.cached_input.is_none()
            && sample.output.is_none()
            && sample.context.is_none()
        {
            None
        } else {
            Some(sample)
        },
    )
}
