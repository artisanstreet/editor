//! Finite Codex owner runtime over `codex app-server --stdio`.
//!
//! Spawns the verified Codex CLI as a JSONL stdio app-server, runs the
//! `initialize` handshake with capability flags plus the opt-out
//! notification list, creates one thread from the typed [`CodexSettings`],
//! starts exactly one turn, normalizes streaming frames onto the shared S1a
//! observation vocabulary (`TextDelta` / `Terminal`; usage and continuation
//! stay later packets), and supports steer/follow-up, interrupt/cancel, and
//! close with exit classification. An inactivity deadline stalls a silent
//! turn; child-custody teardown reuses the owner `process` contract and
//! quarantines on unobserved reaps.
//!
//! The wire boundary is typed: inbound frames decode into [`CodexEvent`]
//! through bounded `serde_json::Value` extraction. Only validated identities
//! and text cross the boundary: approval and question frames become
//! [`CodexApprovalRequest`] / [`CodexQuestionRequest`] and map onto domain
//! `ApprovalRequest` / `QuestionInput` constructors for the durable A-approve
//! resolve path. Deny lands with no side effect while the turn continues;
//! allow answers through the same durable path.
//!
//! Subagent frames are tracked as discovery only and never adopt the root
//! turn: child-thread activity produces no root `TextDelta`. Terminal mapping
//! preserves interruption (external kill) vs cancel vs failure distinctly.

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::time::Duration;

use artisan_domain::{
    ApprovalMode, ApprovalRequest, CodexSelection, FilesystemAccess, NetworkAccess, ObservationId,
    QuestionInput, RootPath, RunId,
};
use artisan_native_engine::CODEX_OPT_OUT_NOTIFICATION_METHODS;
use serde_json::Value;
use thiserror::Error;
use tokio::io::{AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc;
use tokio::time::Instant;

#[cfg(test)]
use super::observation::TerminalObservation;
use super::observation::{EngineObservation, TerminalState, chunk_text};

/// Maximum accepted JSONL frame bytes (mirrors the TS 8 MiB transport cap).
pub(crate) const CODEX_MAX_FRAME_BYTES: usize = 8 * 1024 * 1024;

/// Maximum UTF-8 bytes retained for one approval/question text field.
const CODEX_MAX_TEXT_FIELD_BYTES: usize = 8 * 1024;

/// Maximum questions retained per `requestUserInput` frame.
const CODEX_MAX_QUESTIONS_PER_FRAME: usize = 32;

/// Maximum answers retained per question response.
#[cfg(test)]
pub(crate) const CODEX_MAX_ANSWERS: usize = 32;

/// Payload-free failure of the Codex wire boundary.
///
/// Transport mistakes (spawn, handshake, prompt, stream, stall, cancel,
/// shutdown, deadline, interruption, exit) surface as the owner
/// [`EngineOperationError`](super::operation::EngineOperationError) at the
/// dispatch arm; only typed-boundary mistakes originate here.
#[derive(Clone, Debug, Eq, PartialEq, Error)]
pub(crate) enum CodexTurnError {
    #[error("codex turn misconfigured")]
    Configuration,
    #[error("codex stdio write failed")]
    StreamFailed,
}

/// Typed Codex settings derived from the durable selection.
///
/// Mirrors `MakeCodexAppServerThreadOptions` in
/// `modules/engines/src/codex/internal/permissions.ts`: `always` approval is
/// rejected, network access requires write access, and host scope requires
/// network access (already enforced by `CodexSelection::new`, rechecked here
/// so a corrupt selection fails closed instead of seating a wrong sandbox).
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CodexSettings {
    profile_id: String,
    model: Option<String>,
    approval_policy: &'static str,
    sandbox: &'static str,
    network_access: bool,
    reasoning_effort: Option<String>,
    service_tier: Option<String>,
    model_context_window: Option<u64>,
}

impl CodexSettings {
    /// Derives typed thread settings from the durable selection.
    ///
    /// # Errors
    ///
    /// Returns [`CodexTurnError::Configuration`] when the permission
    /// relations violate the Codex adapter contract.
    pub(crate) fn from_selection(selection: &CodexSelection) -> Result<Self, CodexTurnError> {
        if selection.permission().approval() == ApprovalMode::Always {
            return Err(CodexTurnError::Configuration);
        }
        let write_access = selection.permission().filesystem() != FilesystemAccess::None;
        if selection.permission().network() == NetworkAccess::Enabled && !write_access {
            return Err(CodexTurnError::Configuration);
        }
        if selection.permission().filesystem() == FilesystemAccess::Host
            && selection.permission().network() != NetworkAccess::Enabled
        {
            return Err(CodexTurnError::Configuration);
        }
        let approval_policy = match selection.permission().approval() {
            ApprovalMode::OnRequest => "on-request",
            ApprovalMode::Never => "never",
            ApprovalMode::Always => return Err(CodexTurnError::Configuration),
        };
        let sandbox = match selection.permission().filesystem() {
            FilesystemAccess::None => "read-only",
            FilesystemAccess::Workspace => "workspace-write",
            FilesystemAccess::Host => "danger-full-access",
        };
        Ok(Self {
            profile_id: selection.profile_id().as_str().to_owned(),
            model: selection.model_id().map(|model| model.as_str().to_owned()),
            approval_policy,
            sandbox,
            network_access: selection.permission().network() == NetworkAccess::Enabled,
            reasoning_effort: selection
                .reasoning_effort()
                .map(|effort| effort.as_str().to_owned()),
            service_tier: selection
                .service_tier()
                .map(|tier| tier.as_str().to_owned()),
            model_context_window: selection.model_context_window().map(|window| window.get()),
        })
    }

    /// Returns the managed profile identity.
    pub(crate) fn profile_id(&self) -> &str {
        &self.profile_id
    }

    /// Builds the `thread/start` params object for this turn.
    pub(crate) fn thread_params(&self, project_root: &RootPath) -> Value {
        let mut config = serde_json::Map::new();
        config.insert(
            "model_reasoning_summary".to_owned(),
            Value::String("auto".to_owned()),
        );
        if let Some(effort) = self.reasoning_effort.as_deref() {
            config.insert(
                "model_reasoning_effort".to_owned(),
                Value::String(effort.to_owned()),
            );
        }
        if let Some(window) = self.model_context_window {
            config.insert(
                "model_context_window".to_owned(),
                Value::Number(serde_json::Number::from(window)),
            );
        }
        if self.sandbox == "workspace-write" {
            config.insert(
                "sandbox_workspace_write".to_owned(),
                serde_json::json!({ "network_access": self.network_access }),
            );
        }
        let mut params = serde_json::Map::new();
        params.insert(
            "approvalPolicy".to_owned(),
            Value::String(self.approval_policy.to_owned()),
        );
        params.insert(
            "cwd".to_owned(),
            Value::String(project_root.as_str().to_owned()),
        );
        if let Some(model) = self.model.as_deref() {
            params.insert("model".to_owned(), Value::String(model.to_owned()));
        }
        if self.service_tier.as_deref() == Some("fast") {
            params.insert("serviceTier".to_owned(), Value::String("fast".to_owned()));
        }
        params.insert("sandbox".to_owned(), Value::String(self.sandbox.to_owned()));
        params.insert("config".to_owned(), Value::Object(config));
        Value::Object(params)
    }
}

/// Extracts the sanitized server request id from one inbound envelope.
///
/// The request id is the only provenance that crosses the boundary: method
/// names and frame order stay at the transport, raw payloads never leave it.
/// Approval and question decisions answer exactly this id.
fn native_id_of(envelope: &Value) -> Option<String> {
    envelope
        .get("id")
        .and_then(|id| {
            id.as_str()
                .map(str::to_owned)
                .or_else(|| id.as_i64().map(|number| number.to_string()))
        })
        .filter(|id| !id.is_empty() && id.len() <= 256)
}

/// One typed Codex approval request (permission request).
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CodexApprovalRequest {
    approval_id: String,
    description: String,
    command: Option<String>,
    cwd: Option<String>,
    reason: Option<String>,
    file_change: bool,
}

impl CodexApprovalRequest {
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
    /// Returns [`CodexTurnError::Configuration`] when the bounded fields
    /// violate domain ceilings.
    pub(crate) fn to_domain_request(&self) -> Result<ApprovalRequest, CodexTurnError> {
        if self.file_change {
            ApprovalRequest::file_change(self.reason.clone())
                .map_err(|_| CodexTurnError::Configuration)
        } else if let Some(command) = self.command.clone() {
            ApprovalRequest::command(command, self.cwd.clone(), self.reason.clone())
                .map_err(|_| CodexTurnError::Configuration)
        } else {
            ApprovalRequest::action(self.reason.clone()).map_err(|_| CodexTurnError::Configuration)
        }
    }
}

/// One typed Codex question (`requestUserInput` equivalent).
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CodexQuestion {
    question_id: String,
    text: String,
    header: Option<String>,
}

impl CodexQuestion {
    /// Returns the provider question identity.
    pub(crate) fn question_id(&self) -> &str {
        &self.question_id
    }

    /// Maps this question onto the domain question vocabulary.
    ///
    /// # Errors
    ///
    /// Returns [`CodexTurnError::Configuration`] when identities or text
    /// violate domain ceilings.
    pub(crate) fn to_domain_input(&self) -> Result<QuestionInput, CodexTurnError> {
        let question_id = ObservationId::parse(self.question_id.clone())
            .map_err(|_| CodexTurnError::Configuration)?;
        Ok(QuestionInput {
            question_id,
            text: self.text.clone(),
            header: self.header.clone(),
            multi_select: false,
            options: None,
        })
    }
}

/// One typed Codex question request frame.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CodexQuestionRequest {
    questions: Vec<CodexQuestion>,
}

impl CodexQuestionRequest {
    /// Returns the questions in this request group.
    pub(crate) fn questions(&self) -> &[CodexQuestion] {
        &self.questions
    }
}

/// Typed Codex stream event after bounded boundary decoding.
///
/// Every variant carries only validated identities and text; raw payloads
/// never leave the boundary. Opted-out bookkeeping and unknown methods stay
/// observable as unit variants without disturbing the turn.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum CodexEvent {
    AgentMessageDelta {
        item_id: String,
        turn_id: String,
        delta: String,
    },
    TurnState {
        turn_id: String,
        state: CodexTurnState,
    },
    ApprovalRequested(CodexApprovalRequest),
    QuestionRequested(CodexQuestionRequest),
    SubagentDiscovered {
        agent_thread_id: String,
        parent_thread_id: String,
    },
    ThreadClosed,
    OptedOut,
    UnknownMethod,
}

/// Typed Codex turn lifecycle state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CodexTurnState {
    Started,
    Completed,
    Failed,
    Cancelled,
}

/// Failure decoding one bounded JSONL frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CodexFrameError {
    TooLarge,
    InvalidJson,
    InvalidEnvelope,
}

fn bounded_text(value: &Value, field: &str) -> Option<String> {
    let text = value.get(field)?.as_str()?;
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed.len() > CODEX_MAX_TEXT_FIELD_BYTES {
        return None;
    }
    Some(trimmed.to_owned())
}

fn raw_text(value: &Value, field: &str) -> Option<String> {
    let text = value.get(field)?.as_str()?;
    if text.is_empty() || text.len() > CODEX_MAX_TEXT_FIELD_BYTES {
        return None;
    }
    Some(text.to_owned())
}

/// Decodes one bounded JSONL frame into a typed event.
///
/// Oversized lines reject with [`CodexFrameError::TooLarge`]; invalid JSON or
/// envelopes reject with typed errors and never panic. Unknown methods
/// project to [`CodexEvent::UnknownMethod`] so future server additions stay
/// observable without disturbing the turn. The frame sequence is accepted for
/// call-site ordering and does not cross into the event: native decision
/// identity travels as the validated request id.
pub(crate) fn parse_frame(line: &str, frame_sequence: u64) -> Result<CodexEvent, CodexFrameError> {
    let _ = frame_sequence;
    if line.len() > CODEX_MAX_FRAME_BYTES {
        return Err(CodexFrameError::TooLarge);
    }
    let envelope: Value = serde_json::from_str(line).map_err(|_| CodexFrameError::InvalidJson)?;
    let object = envelope
        .as_object()
        .ok_or(CodexFrameError::InvalidEnvelope)?;
    let method = object
        .get("method")
        .and_then(Value::as_str)
        .ok_or(CodexFrameError::InvalidEnvelope)?;
    if method.is_empty() || method.len() > 128 {
        return Err(CodexFrameError::InvalidEnvelope);
    }
    let params = object.get("params").cloned().unwrap_or(Value::Null);
    Ok(decode_method(method, &envelope, &params))
}

fn decode_method(method: &str, envelope: &Value, params: &Value) -> CodexEvent {
    match method {
        "item/agentMessage/delta" => {
            let item_id = raw_text(params, "itemId").unwrap_or_default();
            let turn_id = raw_text(params, "turnId").unwrap_or_default();
            let delta = params
                .get("delta")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            if item_id.is_empty() || turn_id.is_empty() || delta.is_empty() {
                return CodexEvent::UnknownMethod;
            }
            CodexEvent::AgentMessageDelta {
                item_id,
                turn_id,
                delta,
            }
        }
        "turn/started" | "turn/completed" => {
            let turn_id = params
                .get("turn")
                .and_then(|turn| turn.get("id"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned();
            let status = params
                .get("turn")
                .and_then(|turn| turn.get("status"))
                .and_then(Value::as_str)
                .unwrap_or("inProgress");
            let state = match status {
                "completed" => CodexTurnState::Completed,
                "failed" => CodexTurnState::Failed,
                "interrupted" => CodexTurnState::Cancelled,
                _ => CodexTurnState::Started,
            };
            if turn_id.is_empty() {
                return CodexEvent::UnknownMethod;
            }
            CodexEvent::TurnState { turn_id, state }
        }
        "item/commandExecution/requestApproval" => {
            let item_id = raw_text(params, "itemId").unwrap_or_default();
            let command = bounded_text(params, "command");
            let cwd = bounded_text(params, "cwd");
            let reason = bounded_text(params, "reason");
            let approval_id = native_id_of(envelope).unwrap_or_else(|| item_id.clone());
            if approval_id.is_empty() {
                return CodexEvent::UnknownMethod;
            }
            CodexEvent::ApprovalRequested(CodexApprovalRequest {
                approval_id,
                description: reason.clone().unwrap_or_else(|| "Run a command".to_owned()),
                command,
                cwd,
                reason,
                file_change: false,
            })
        }
        "item/fileChange/requestApproval" => {
            let item_id = raw_text(params, "itemId").unwrap_or_default();
            let reason = bounded_text(params, "reason");
            let approval_id = native_id_of(envelope).unwrap_or_else(|| item_id.clone());
            if approval_id.is_empty() {
                return CodexEvent::UnknownMethod;
            }
            CodexEvent::ApprovalRequested(CodexApprovalRequest {
                approval_id,
                description: reason
                    .clone()
                    .unwrap_or_else(|| "Apply file changes".to_owned()),
                command: None,
                cwd: None,
                reason,
                file_change: true,
            })
        }
        "item/tool/requestUserInput" => {
            let questions = params
                .get("questions")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .take(CODEX_MAX_QUESTIONS_PER_FRAME)
                        .filter_map(|item| {
                            let id = item.get("id")?.as_str()?;
                            let text = item.get("question")?.as_str()?;
                            if id.is_empty() || text.is_empty() {
                                return None;
                            }
                            Some(CodexQuestion {
                                question_id: id.to_owned(),
                                text: text.to_owned(),
                                header: item
                                    .get("header")
                                    .and_then(Value::as_str)
                                    .map(str::to_owned),
                            })
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            if questions.is_empty() {
                return CodexEvent::UnknownMethod;
            }
            CodexEvent::QuestionRequested(CodexQuestionRequest { questions })
        }
        "item/subAgent/discovered" | "item/collabAgent/discovered" => {
            let agent_thread_id = raw_text(params, "agentThreadId").unwrap_or_default();
            let parent_thread_id = raw_text(params, "parentThreadId")
                .or_else(|| raw_text(params, "senderThreadId"))
                .unwrap_or_default();
            if agent_thread_id.is_empty() || parent_thread_id.is_empty() {
                return CodexEvent::UnknownMethod;
            }
            CodexEvent::SubagentDiscovered {
                agent_thread_id,
                parent_thread_id,
            }
        }
        "thread/closed" => CodexEvent::ThreadClosed,
        _ => {
            if CODEX_OPT_OUT_NOTIFICATION_METHODS.contains(&method) {
                return CodexEvent::OptedOut;
            }
            CodexEvent::UnknownMethod
        }
    }
}
/// In-memory pending interaction tracker for one live Codex turn.
///
/// Permission requests land as pending approvals and `requestUserInput`
/// frames land as pending questions. Resolutions apply through the durable
/// resolve path; a deny records the decision with no turn side effect while
/// the run continues. Subagent discoveries are retained for transcript
/// projection but never adopt the root turn.
#[derive(Debug, Default)]
pub(crate) struct CodexPendingTracker {
    approvals: HashMap<String, CodexApprovalRequest>,
    questions: HashMap<String, CodexQuestion>,
    subagents: Vec<(String, String)>,
}

impl CodexPendingTracker {
    /// Creates an empty tracker for one turn.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Notes one approval request; re-noting the same id is a no-op.
    ///
    /// The request is validated through the domain constructor first, so an
    /// out-of-bound provider frame never reaches the durable A-approve rows.
    pub(crate) fn note_approval(&mut self, request: CodexApprovalRequest) -> bool {
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
    pub(crate) fn note_questions(&mut self, request: &CodexQuestionRequest) -> usize {
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

    /// Notes one subagent discovery without adopting the root turn.
    pub(crate) fn note_subagent(&mut self, agent_thread_id: &str, parent_thread_id: &str) {
        self.subagents
            .push((agent_thread_id.to_owned(), parent_thread_id.to_owned()));
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

    /// Returns the number of discovered subagents.
    #[cfg(test)]
    pub(crate) fn subagent_count(&self) -> usize {
        self.subagents.len()
    }
}

/// Builds the `initialize` request params with capability flags plus the
/// opt-out notification list.
pub(crate) fn initialize_params(client_name: &str, client_version: &str) -> Value {
    serde_json::json!({
        "capabilities": {
            "experimentalApi": false,
            "optOutNotificationMethods": CODEX_OPT_OUT_NOTIFICATION_METHODS,
            "requestAttestation": false,
        },
        "clientInfo": {
            "name": client_name,
            "version": client_version,
        },
    })
}

/// Builds one JSON-RPC request line for the stdio transport.
pub(crate) fn request_line(id: u64, method: &str, params: &Value) -> String {
    serde_json::json!({ "id": id, "method": method, "params": params }).to_string()
}

/// Writes one JSONL line plus its terminator over the stdio transport.
pub(crate) async fn write_line<W: AsyncWrite + Unpin>(
    stdin: &mut W,
    line: &str,
) -> Result<(), CodexTurnError> {
    stdin
        .write_all(line.as_bytes())
        .await
        .map_err(|_| CodexTurnError::StreamFailed)?;
    stdin
        .write_all(b"\n")
        .await
        .map_err(|_| CodexTurnError::StreamFailed)?;
    stdin
        .flush()
        .await
        .map_err(|_| CodexTurnError::StreamFailed)?;
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

/// Applies one typed event; returns the terminal state when the turn ends.
pub(crate) async fn apply_event(
    event: CodexEvent,
    run_id: &RunId,
    tracker: &mut CodexPendingTracker,
    active_turn: &mut Option<String>,
    observations: &mpsc::Sender<EngineObservation>,
    frame_sequence: u64,
) -> Option<TerminalState> {
    match event {
        CodexEvent::AgentMessageDelta {
            item_id,
            turn_id,
            delta,
        } => {
            if active_turn.is_none() {
                *active_turn = Some(turn_id.clone());
            }
            let native_id = format!("codex:{frame_sequence}");
            for chunk in chunk_text(run_id, frame_sequence, &native_id, &delta) {
                let part_item = item_id.clone();
                let chunk = chunk.with_part_id(part_item);
                if observations
                    .send(EngineObservation::TextDelta(chunk))
                    .await
                    .is_err()
                {
                    return Some(TerminalState::Interrupted);
                }
            }
            None
        }
        CodexEvent::TurnState { turn_id, state } => {
            *active_turn = Some(turn_id);
            match state {
                CodexTurnState::Completed => Some(TerminalState::Completed),
                CodexTurnState::Failed => Some(TerminalState::Failed),
                CodexTurnState::Cancelled => Some(TerminalState::Cancelled),
                CodexTurnState::Started => None,
            }
        }
        CodexEvent::ApprovalRequested(request) => {
            tracker.note_approval(request);
            None
        }
        CodexEvent::QuestionRequested(request) => {
            tracker.note_questions(&request);
            None
        }
        CodexEvent::SubagentDiscovered {
            agent_thread_id,
            parent_thread_id,
        } => {
            // Discovery only: never adopt the root turn, never emit root text.
            tracker.note_subagent(&agent_thread_id, &parent_thread_id);
            None
        }
        CodexEvent::ThreadClosed | CodexEvent::OptedOut | CodexEvent::UnknownMethod => None,
    }
}

/// Steers a live turn with follow-up text (`turn/steer`).
///
/// Test-only until dispatcher steer wiring lands: proves the follow-up verb
/// against the fixture stdio script without disturbing the authorize-once
/// production flow.
///
/// # Errors
///
/// Returns [`CodexTurnError`] when the write fails.
#[cfg(test)]
pub(crate) async fn steer_live_turn<W: AsyncWrite + Unpin>(
    stdin: &mut W,
    request_id: &mut u64,
    thread_id: &str,
    turn_id: &str,
    text: &str,
) -> Result<(), CodexTurnError> {
    let params = serde_json::json!({
        "expectedTurnId": turn_id,
        "input": [{ "text": text, "text_elements": [], "type": "text" }],
        "threadId": thread_id,
    });
    let line = request_line(*request_id, "turn/steer", &params);
    *request_id += 1;
    write_line(stdin, &line).await
}

/// Interrupts a live turn (`turn/interrupt`) before cancelling the driver.
pub(crate) async fn interrupt_live_turn<W: AsyncWrite + Unpin>(
    stdin: &mut W,
    request_id: &mut u64,
    thread_id: &str,
    turn_id: &str,
) -> Result<(), CodexTurnError> {
    let params = serde_json::json!({ "threadId": thread_id, "turnId": turn_id });
    let line = request_line(*request_id, "turn/interrupt", &params);
    *request_id += 1;
    write_line(stdin, &line).await
}

/// Answers one pending approval through the durable decision.
///
/// Test-only until dispatcher delivery wiring lands: deny carries no turn
/// side effect and the run continues either way.
///
/// # Errors
///
/// Returns [`CodexTurnError::Configuration`] for an unknown or resolved
/// target and [`CodexTurnError::StreamFailed`] when the write fails.
#[cfg(test)]
pub(crate) async fn answer_approval<W: AsyncWrite + Unpin>(
    stdin: &mut W,
    tracker: &mut CodexPendingTracker,
    native_id: &str,
    approval_id: &str,
    approved: bool,
) -> Result<(), CodexTurnError> {
    if !tracker.resolve_approval(approval_id) {
        return Err(CodexTurnError::Configuration);
    }
    let decision = if approved { "approved" } else { "denied" };
    let line =
        serde_json::json!({ "id": native_id, "result": { "decision": decision } }).to_string();
    write_line(stdin, &line).await
}

/// Answers one question request group through the durable answers.
///
/// Test-only until dispatcher delivery wiring lands.
///
/// # Errors
///
/// Returns [`CodexTurnError::Configuration`] for an unknown or resolved
/// target and [`CodexTurnError::StreamFailed`] when the write fails.
#[cfg(test)]
pub(crate) async fn answer_questions<W: AsyncWrite + Unpin>(
    stdin: &mut W,
    tracker: &mut CodexPendingTracker,
    native_id: &str,
    answers: &[(String, Vec<String>)],
) -> Result<(), CodexTurnError> {
    for (question_id, _) in answers {
        if !tracker.resolve_question(question_id) {
            return Err(CodexTurnError::Configuration);
        }
    }
    let mut map = serde_json::Map::new();
    for (question_id, options) in answers.iter().take(CODEX_MAX_ANSWERS) {
        map.insert(
            question_id.clone(),
            serde_json::json!({ "answers": options }),
        );
    }
    let line = serde_json::json!({ "id": native_id, "result": { "answers": Value::Object(map) } })
        .to_string();
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
