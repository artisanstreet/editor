use std::collections::HashMap;
use std::time::Duration;

use artisan_domain::{
    ApprovalMode, ApprovalRequest, CodexSelection, FilesystemAccess, NetworkAccess, ObservationId,
    QuestionInput, RootPath,
};
use artisan_native_engine::CODEX_OPT_OUT_NOTIFICATION_METHODS;
use serde_json::Value;
use thiserror::Error;
use tokio::io::{AsyncWrite, AsyncWriteExt};
use tokio::time::Instant;

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
            model_context_window: selection
                .model_context_window()
                .map(artisan_domain::CodexModelContextWindow::get),
        })
    }

    /// Returns the managed profile identity.
    pub(crate) fn profile_id(&self) -> &str {
        &self.profile_id
    }

    /// Builds the `turn/start` params object for this turn.
    ///
    /// Mirrors `session.Request("turn/start", ...)` in
    /// `modules/engines/src/codex/engine.ts`: the request always binds the
    /// native `threadId` returned by `thread/start` (or `thread/resume`),
    /// carries the prompt as the single text input part, and repeats
    /// `serviceTier: "fast"` exactly when the durable selection requests the
    /// fast tier. A missing `threadId` is rejected by the real server with
    /// `-32600 Invalid request: missing field threadId`, so omitting it
    /// stalls the turn until the lease expires instead of failing fast.
    pub(crate) fn turn_start_params(&self, thread_id: &str, prompt_text: &str) -> Value {
        let mut params = serde_json::Map::new();
        params.insert(
            "input".to_owned(),
            Value::Array(vec![serde_json::json!({
                "text": prompt_text,
                "text_elements": [],
                "type": "text",
            })]),
        );
        params.insert("threadId".to_owned(), Value::String(thread_id.to_owned()));
        if self.service_tier.as_deref() == Some("fast") {
            params.insert("serviceTier".to_owned(), Value::String("fast".to_owned()));
        }
        Value::Object(params)
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
    /// Cumulative token usage from one `thread/tokenUsage/updated` frame.
    ///
    /// Projected best-effort onto the shared usage vocabulary; an empty
    /// measurement never becomes a report (see [`parse_thread_token_usage`]).
    TokenUsage {
        turn_id: String,
        sample: CodexTokenUsageSample,
    },
    ApprovalRequested(CodexApprovalRequest),
    QuestionRequested(CodexQuestionRequest),
    SubagentDiscovered {
        agent_thread_id: String,
        parent_thread_id: String,
    },
    /// One published reasoning-summary fragment (`summaryTextDelta`).
    ///
    /// Carries only public summary text, never private reasoning content:
    /// `item/reasoning/textDelta` stays [`CodexEvent::ActivitySilent`].
    ReasoningSummaryDelta {
        thread_id: String,
        turn_id: String,
        item_id: String,
        summary_index: u64,
        delta: String,
    },
    /// One structural separator between public reasoning-summary sections
    /// (`summaryPartAdded` with a nonzero index, mirroring the TypeScript
    /// `"\n\n"` boundary).
    ReasoningSummaryBoundary {
        thread_id: String,
        turn_id: String,
        item_id: String,
        summary_index: u64,
    },
    /// One settled public reasoning phase (`item/completed` on a reasoning
    /// item). `text` is the joined authoritative summary, or [`None`] when
    /// the provider supplied no summary text at all.
    ReasoningSettled {
        thread_id: String,
        turn_id: String,
        item_id: String,
        text: Option<String>,
    },
    /// One tool lifecycle step (`mcpToolCall` / `dynamicToolCall` started,
    /// progress, or completion). `detail` carries only the provider progress
    /// message, when disclosed and bounded.
    ToolLifecycle {
        thread_id: String,
        turn_id: String,
        item_id: String,
        tool_name: String,
        action: CodexToolAction,
        detail: Option<String>,
    },
    /// One terminal output chunk (`commandExecution/outputDelta`).
    TerminalOutput {
        thread_id: String,
        turn_id: String,
        item_id: String,
        delta: String,
    },
    /// One command lifecycle step (`item/started` / `item/completed` on a
    /// `commandExecution` item).
    TerminalLifecycle {
        thread_id: String,
        turn_id: String,
        item_id: String,
        command: Option<String>,
        output: Option<String>,
        exit_code: Option<i32>,
        state: CodexTerminalLifecycle,
    },
    /// One completed `fileChange` item with its per-file payloads.
    FileCompleted {
        thread_id: String,
        turn_id: String,
        item_id: String,
        changes: Vec<CodexFileChange>,
    },
    /// One search lifecycle step (`item/started` / `item/completed` on a
    /// `webSearch` item).
    SearchLifecycle {
        thread_id: String,
        turn_id: String,
        item_id: String,
        query: String,
        state: CodexSearchLifecycle,
    },
    /// One provider-neutral plan update (`turn/plan/updated` or an
    /// `item/started` / `item/completed` plan item).
    PlanUpdated {
        thread_id: String,
        turn_id: String,
        entries: Vec<CodexPlanEntry>,
    },
    /// A decoded-known frame that intentionally emits no observation: started
    /// reasoning items, the section-zero `summaryPartAdded` opener, and
    /// private `item/reasoning/textDelta` content (never surfaced).
    ActivitySilent,
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

/// Lifecycle action of one normalized Codex tool step.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CodexToolAction {
    Started,
    Progress,
    Completed,
    Failed,
}

/// Lifecycle state of one normalized Codex command step.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CodexTerminalLifecycle {
    Started,
    Completed,
    Failed,
}

/// Lifecycle state of one normalized Codex search step.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CodexSearchLifecycle {
    Started,
    Completed,
}

/// One file payload inside a completed `fileChange` item.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CodexFileChange {
    pub(crate) path: String,
    pub(crate) kind: CodexFileKind,
    pub(crate) diff: String,
}

/// Provider-disclosed mutation kind of one file payload.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CodexFileKind {
    Add,
    Delete,
    Update,
}

/// One plan step inside a `turn/plan/updated` frame or plan item.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CodexPlanEntry {
    pub(crate) id: String,
    pub(crate) status: CodexPlanStatus,
    pub(crate) text: String,
}

/// Provider-disclosed status of one plan step.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CodexPlanStatus {
    Pending,
    InProgress,
    Completed,
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

/// Extracts the exact claimed `(threadId, turnId)` scope of one activity
/// frame. Both identities must be present and non-empty: activity never
/// invents scope, and scope-free frames stay [`CodexEvent::UnknownMethod`].
fn claimed_scope(params: &Value) -> Option<(String, String)> {
    Some((raw_text(params, "threadId")?, raw_text(params, "turnId")?))
}

/// Extracts one bounded non-empty provider string from a nested item object.
fn nested_text(item: &serde_json::Map<String, Value>, field: &str) -> Option<String> {
    let text = item.get(field)?.as_str()?;
    if text.is_empty() || text.len() > CODEX_MAX_TEXT_FIELD_BYTES {
        return None;
    }
    Some(text.to_owned())
}

#[expect(
    clippy::too_many_lines,
    reason = "single method dispatch table over the app-server envelope vocabulary; extracting arms would add indirection without reuse"
)]
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
        "thread/tokenUsage/updated" => {
            let turn_id = raw_text(params, "turnId").unwrap_or_default();
            if turn_id.is_empty() {
                return CodexEvent::UnknownMethod;
            }
            match parse_thread_token_usage(params) {
                Some(sample) => CodexEvent::TokenUsage { turn_id, sample },
                // An empty measurement stays observable without a report.
                None => CodexEvent::UnknownMethod,
            }
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
        "item/reasoning/summaryTextDelta" => {
            let Some((thread_id, turn_id)) = claimed_scope(params) else {
                return CodexEvent::UnknownMethod;
            };
            let item_id = raw_text(params, "itemId").unwrap_or_default();
            let delta = params
                .get("delta")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let summary_index = params.get("summaryIndex").and_then(Value::as_u64);
            match (item_id.is_empty(), delta.is_empty(), summary_index) {
                (false, false, Some(summary_index)) => CodexEvent::ReasoningSummaryDelta {
                    thread_id,
                    turn_id,
                    item_id,
                    summary_index,
                    delta,
                },
                _ => CodexEvent::UnknownMethod,
            }
        }
        "item/reasoning/summaryPartAdded" => {
            let Some((thread_id, turn_id)) = claimed_scope(params) else {
                return CodexEvent::UnknownMethod;
            };
            let item_id = raw_text(params, "itemId").unwrap_or_default();
            let summary_index = params.get("summaryIndex").and_then(Value::as_u64);
            match (item_id.is_empty(), summary_index) {
                // The section-zero opener precedes the first public text and
                // carries no text of its own.
                (false, Some(0)) => CodexEvent::ActivitySilent,
                (false, Some(summary_index)) => CodexEvent::ReasoningSummaryBoundary {
                    thread_id,
                    turn_id,
                    item_id,
                    summary_index,
                },
                _ => CodexEvent::UnknownMethod,
            }
        }
        // Private reasoning content is never surfaced or durably projected:
        // only the published summary text normalizes onto the activity
        // vocabulary. The frame stays decoded-known so it never disturbs the
        // turn.
        "item/reasoning/textDelta" => CodexEvent::ActivitySilent,
        "turn/plan/updated" => {
            let Some((thread_id, turn_id)) = claimed_scope(params) else {
                return CodexEvent::UnknownMethod;
            };
            let steps = params
                .get("plan")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| {
                            let step = item.get("step")?.as_str()?;
                            if step.is_empty() {
                                return None;
                            }
                            let status = match item.get("status")?.as_str()? {
                                "pending" => CodexPlanStatus::Pending,
                                "inProgress" => CodexPlanStatus::InProgress,
                                "completed" => CodexPlanStatus::Completed,
                                _ => return None,
                            };
                            Some((status, step.to_owned()))
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            if steps.is_empty() {
                return CodexEvent::UnknownMethod;
            }
            let entries = steps
                .into_iter()
                .enumerate()
                .map(|(index, (status, text))| CodexPlanEntry {
                    id: format!("{turn_id}:plan:{index}"),
                    status,
                    text,
                })
                .collect();
            CodexEvent::PlanUpdated {
                thread_id,
                turn_id,
                entries,
            }
        }
        "item/commandExecution/outputDelta" => {
            let Some((thread_id, turn_id)) = claimed_scope(params) else {
                return CodexEvent::UnknownMethod;
            };
            let item_id = raw_text(params, "itemId").unwrap_or_default();
            let delta = params
                .get("delta")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            if item_id.is_empty() || delta.is_empty() {
                return CodexEvent::UnknownMethod;
            }
            CodexEvent::TerminalOutput {
                thread_id,
                turn_id,
                item_id,
                delta,
            }
        }
        "item/mcpToolCall/progress" => {
            let Some((thread_id, turn_id)) = claimed_scope(params) else {
                return CodexEvent::UnknownMethod;
            };
            let item_id = raw_text(params, "itemId").unwrap_or_default();
            let message = params
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            if item_id.is_empty() || message.is_empty() {
                return CodexEvent::UnknownMethod;
            }
            CodexEvent::ToolLifecycle {
                thread_id,
                turn_id,
                item_id,
                tool_name: "mcp".to_owned(),
                action: CodexToolAction::Progress,
                detail: Some(message),
            }
        }
        "item/started" | "item/completed" => decode_item_envelope(method == "item/started", params),
        _ => {
            if CODEX_OPT_OUT_NOTIFICATION_METHODS.contains(&method) {
                return CodexEvent::OptedOut;
            }
            CodexEvent::UnknownMethod
        }
    }
}

/// Decodes one `item/started` / `item/completed` envelope into a typed
/// activity event, mirroring the TypeScript item union.
///
/// Only item types with a valid existing domain observation normalize:
/// reasoning (published summary only), tool (`mcpToolCall` /
/// `dynamicToolCall`), `commandExecution`, `fileChange`, `webSearch`, and
/// plan. Agent-message items stay on the plain delta path, user-message and
/// subagent items stay on the tracker path, and compaction items have no
/// owner channel, so all of those remain [`CodexEvent::UnknownMethod`]
/// without disturbing the turn.
#[expect(
    clippy::too_many_lines,
    reason = "single item-type dispatch table over the Codex item vocabulary; extracting arms would add indirection without reuse"
)]
fn decode_item_envelope(started: bool, params: &Value) -> CodexEvent {
    let Some((thread_id, turn_id)) = claimed_scope(params) else {
        return CodexEvent::UnknownMethod;
    };
    let Some(item) = params.get("item").and_then(Value::as_object) else {
        return CodexEvent::UnknownMethod;
    };
    let item_type = item.get("type").and_then(Value::as_str).unwrap_or("");
    let item_id = nested_text(item, "id").unwrap_or_default();
    match item_type {
        "reasoning" => {
            if item_id.is_empty() {
                return CodexEvent::UnknownMethod;
            }
            if started {
                return CodexEvent::ActivitySilent;
            }
            let Some(summary) = item.get("summary").and_then(Value::as_array) else {
                return CodexEvent::UnknownMethod;
            };
            let text = summary
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join("\n\n");
            CodexEvent::ReasoningSettled {
                thread_id,
                turn_id,
                item_id,
                text: if text.is_empty() { None } else { Some(text) },
            }
        }
        "commandExecution" => {
            if item_id.is_empty() {
                return CodexEvent::UnknownMethod;
            }
            // The provider schema allows exactly `completed | declined |
            // failed | inProgress` here: any other spelling is malformed and
            // must never fall through into a fabricated completion.
            let status = match item.get("status").and_then(Value::as_str) {
                Some("completed" | "inProgress") => "completed",
                Some("failed" | "declined") => "failed",
                _ => return CodexEvent::UnknownMethod,
            };
            let state = if started {
                CodexTerminalLifecycle::Started
            } else if status == "failed" {
                CodexTerminalLifecycle::Failed
            } else {
                CodexTerminalLifecycle::Completed
            };
            CodexEvent::TerminalLifecycle {
                thread_id,
                turn_id,
                item_id,
                command: item
                    .get("command")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                output: item
                    .get("aggregatedOutput")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                exit_code: item
                    .get("exitCode")
                    .and_then(Value::as_i64)
                    .and_then(|code| i32::try_from(code).ok()),
                state,
            }
        }
        "mcpToolCall" | "dynamicToolCall" => {
            if item_id.is_empty() {
                return CodexEvent::UnknownMethod;
            }
            // The provider schema allows exactly `completed | failed |
            // inProgress` here: any other spelling is malformed and must
            // never fall through into a fabricated completion.
            let failed = match item.get("status").and_then(Value::as_str) {
                Some("completed" | "inProgress") => false,
                Some("failed") => true,
                _ => return CodexEvent::UnknownMethod,
            };
            let Some(tool) = nested_text(item, "tool") else {
                return CodexEvent::UnknownMethod;
            };
            let tool_name = if item_type == "mcpToolCall" {
                let Some(server) = nested_text(item, "server") else {
                    return CodexEvent::UnknownMethod;
                };
                format!("{server}/{tool}")
            } else {
                let namespace = item
                    .get("namespace")
                    .and_then(Value::as_str)
                    .unwrap_or("dynamic");
                format!("{namespace}/{tool}")
            };
            let action = if started {
                CodexToolAction::Started
            } else if failed {
                CodexToolAction::Failed
            } else {
                CodexToolAction::Completed
            };
            CodexEvent::ToolLifecycle {
                thread_id,
                turn_id,
                item_id,
                tool_name,
                action,
                detail: None,
            }
        }
        "fileChange" => {
            if item_id.is_empty() {
                return CodexEvent::UnknownMethod;
            }
            let Some(status) = item.get("status").and_then(Value::as_str) else {
                return CodexEvent::UnknownMethod;
            };
            if started || status != "completed" {
                return CodexEvent::UnknownMethod;
            }
            let Some(changes) = item.get("changes").and_then(Value::as_array) else {
                return CodexEvent::UnknownMethod;
            };
            CodexEvent::FileCompleted {
                thread_id,
                turn_id,
                item_id,
                changes: changes
                    .iter()
                    .filter_map(|change| {
                        let object = change.as_object()?;
                        let path = nested_text(object, "path")?;
                        let kind = match object
                            .get("kind")
                            .and_then(Value::as_object)
                            .and_then(|kind| kind.get("type"))
                            .and_then(Value::as_str)?
                        {
                            "add" => CodexFileKind::Add,
                            "delete" => CodexFileKind::Delete,
                            "update" => CodexFileKind::Update,
                            _ => return None,
                        };
                        Some(CodexFileChange {
                            path,
                            kind,
                            diff: object
                                .get("diff")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_owned(),
                        })
                    })
                    .collect(),
            }
        }
        "webSearch" => {
            if item_id.is_empty() {
                return CodexEvent::UnknownMethod;
            }
            let Some(query) = nested_text(item, "query") else {
                return CodexEvent::UnknownMethod;
            };
            CodexEvent::SearchLifecycle {
                thread_id,
                turn_id,
                item_id,
                query,
                state: if started {
                    CodexSearchLifecycle::Started
                } else {
                    CodexSearchLifecycle::Completed
                },
            }
        }
        "plan" => {
            if item_id.is_empty() {
                return CodexEvent::UnknownMethod;
            }
            let Some(text) = nested_text(item, "text") else {
                return CodexEvent::UnknownMethod;
            };
            CodexEvent::PlanUpdated {
                thread_id,
                turn_id,
                entries: vec![CodexPlanEntry {
                    id: item_id,
                    status: if started {
                        CodexPlanStatus::InProgress
                    } else {
                        CodexPlanStatus::Completed
                    },
                    text,
                }],
            }
        }
        _ => CodexEvent::UnknownMethod,
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
    /// The root native thread bound by the pump from `thread/start` (or the
    /// resumed thread). Rich activity frames must claim exactly this thread;
    /// an unbound tracker emits no activity at all.
    native_thread_id: Option<String>,
}

impl CodexPendingTracker {
    /// Creates an empty tracker for one turn.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Binds the root native thread authority for rich activity.
    ///
    /// The pump calls this once with the native thread from `thread/start`
    /// (or the resumed thread) before the `turn/start` wait, so legitimate
    /// interleaved root frames already normalize. Empty identities never
    /// pin: the tracker stays unbound and activity stays dropped. Activity
    /// authority never depends on the optional usage model scope.
    pub(crate) fn bind_native_thread(&mut self, native_thread_id: &str) {
        if native_thread_id.is_empty() {
            return;
        }
        self.native_thread_id = Some(native_thread_id.to_owned());
    }

    /// Returns the bound root native thread, when the pump pinned one.
    pub(crate) fn native_thread_id(&self) -> Option<&str> {
        self.native_thread_id.as_deref()
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

    /// Returns whether a native thread identity belongs to a discovered
    /// child agent. Activity frames claiming a child thread never reach the
    /// root channel: child content travels the transcript projection, never
    /// as root activity.
    pub(crate) fn is_known_child_thread(&self, thread_id: &str) -> bool {
        self.subagents
            .iter()
            .any(|(agent_thread_id, _)| agent_thread_id == thread_id)
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

/// Builds one JSON-RPC notification line (no id) for the stdio transport.
///
/// Mirrors `Notify` in `modules/engines/src/codex/app-server-session.ts`:
/// `{ method, ...(params === undefined ? {} : { params }) }`. The official
/// handshake sends `initialized` with no params after the `initialize`
/// result; a notification never consumes a request id.
pub(crate) fn notification_line(method: &str) -> String {
    serde_json::json!({ "method": method }).to_string()
}

/// Returns whether one inbound line is a JSON-RPC error response.
///
/// Error responses carry `id` plus `error` and no `method` (see
/// `DecodeCodexInboundEnvelope` in `modules/engines/src/codex/protocol.ts`).
/// The streaming pump must fail fast on these instead of ignoring them until
/// the lease expires: a rejected `turn/start` (for example `-32600` for a
/// missing `threadId`) never produces turn events.
pub(crate) fn is_codex_error_response(line: &str) -> bool {
    if line.len() > CODEX_MAX_FRAME_BYTES {
        return false;
    }
    let Ok(value) = serde_json::from_str::<Value>(line) else {
        return false;
    };
    let Some(object) = value.as_object() else {
        return false;
    };
    if object.contains_key("method") {
        return false;
    }
    object.contains_key("id")
        && object
            .get("error")
            .is_some_and(serde_json::Value::is_object)
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

/// Cumulative token sample from one `thread/tokenUsage/updated` frame.
///
/// Mirrors the TypeScript `UsageSchema` shape: `total` carries the running
/// counters, `last.totalTokens` gauges the current window, and
/// `modelContextWindow` carries the provider window.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct CodexTokenUsageSample {
    pub input: Option<u64>,
    pub cached_input: Option<u64>,
    pub output: Option<u64>,
    /// Window gauge from the last request only — never a sum. Every turn
    /// resends the conversation, so `total` keeps adding each resend while
    /// `last` measures what actually occupies the window right now. Absent
    /// stays absent rather than becoming a wrong zero.
    pub context: Option<u64>,
    pub context_window: Option<u64>,
}

/// Extracts the cumulative token sample from `thread/tokenUsage/updated`
/// params.
///
/// Non-`u64` numerics (negatives, fractions) fail closed to absent for that
/// field; absent stays absent rather than becoming zero. Returns `None` when
/// no token field carries a value: an empty measurement is not a report.
pub(crate) fn parse_thread_token_usage(params: &Value) -> Option<CodexTokenUsageSample> {
    let usage = params.get("tokenUsage")?.as_object()?;
    let total = usage.get("total").and_then(Value::as_object);
    let number = |object: Option<&serde_json::Map<String, Value>>, field: &str| {
        object
            .and_then(|object| object.get(field))
            .and_then(Value::as_u64)
    };
    let sample = CodexTokenUsageSample {
        input: number(total, "inputTokens"),
        cached_input: number(total, "cachedInputTokens"),
        output: number(total, "outputTokens"),
        context: usage
            .get("last")
            .and_then(Value::as_object)
            .and_then(|last| last.get("totalTokens"))
            .and_then(Value::as_u64),
        context_window: usage.get("modelContextWindow").and_then(Value::as_u64),
    };
    if sample.input.is_none()
        && sample.cached_input.is_none()
        && sample.output.is_none()
        && sample.context.is_none()
        && sample.context_window.is_none()
    {
        return None;
    }
    Some(sample)
}
