use std::collections::HashMap;
use std::time::Duration;

use artisan_domain::{
    ApprovalMode, ApprovalRequest, CodexSelection, FilesystemAccess, NetworkAccess, ObservationId,
    QuestionInput, RootPath,
};
use artisan_native_engine::CODEX_OPT_OUT_NOTIFICATION_METHODS;
use base64::Engine as _;
use serde_json::Value;

use crate::engine_inventory::InventoryReport;
use thiserror::Error;
use tokio::io::{AsyncWrite, AsyncWriteExt};
use tokio::time::Instant;

use super::compaction::{CodexCompactionSignal, CodexCompactionTracker};

/// Maximum accepted JSONL frame bytes (mirrors the TS 8 MiB transport cap).
pub(crate) const CODEX_MAX_FRAME_BYTES: usize = 8 * 1024 * 1024;

/// Maximum UTF-8 bytes retained for one approval/question text field.
const CODEX_MAX_TEXT_FIELD_BYTES: usize = 8 * 1024;

/// Maximum questions retained per `requestUserInput` frame.
const CODEX_MAX_QUESTIONS_PER_FRAME: usize = 32;

/// Maximum answers retained per question response.
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
    #[cfg(test)]
    pub(crate) fn turn_start_params(&self, thread_id: &str, text: &str) -> Value {
        self.turn_start_params_with_images(thread_id, text, &[])
    }

    pub(crate) fn turn_start_params_with_images(
        &self,
        thread_id: &str,
        text: &str,
        images: &[artisan_domain::ImageAttachment],
    ) -> Value {
        let mut params = serde_json::Map::new();
        params.insert("input".to_owned(), prompt_input(text, images));
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
    rpc_id: Value,
    approval_id: String,
    description: String,
    command: Option<String>,
    cwd: Option<String>,
    reason: Option<String>,
    file_change: bool,
}

impl CodexApprovalRequest {
    /// Returns the provider approval identity.
    pub(crate) fn approval_id(&self) -> &str {
        &self.approval_id
    }

    /// Returns the human description.
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

/// Maximum suggested answers retained per question.
const CODEX_MAX_OPTIONS_PER_QUESTION: usize = 16;

/// One typed Codex question from an `item/tool/requestUserInput` request.
///
/// The request asks several questions together and stays open until the
/// user answers them all; the questions join one questionnaire
/// (`group_id`: the request's tool item) and present the same as every
/// other engine's questions.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CodexQuestion {
    /// Unique on the thread: the questionnaire prefixes the provider id.
    question_id: String,
    /// The provider's own id for the question inside its request.
    provider_id: String,
    group_id: String,
    text: String,
    header: Option<String>,
    /// Suggested answers as `(label, description)`, recommended first.
    options: Vec<(String, Option<String>)>,
}

impl CodexQuestion {
    /// Returns the provider question identity.
    #[cfg(test)]
    pub(crate) fn question_id(&self) -> &str {
        &self.question_id
    }

    /// Returns the questionnaire identity this question was asked in.
    pub(crate) fn group_id(&self) -> &str {
        &self.group_id
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
        let options = if self.options.is_empty() {
            None
        } else {
            let mut options = Vec::with_capacity(self.options.len());
            for (label, description) in &self.options {
                options.push(
                    artisan_domain::QuestionOption::new(label.clone(), description.clone())
                        .map_err(|_| CodexTurnError::Configuration)?,
                );
            }
            Some(options)
        };
        Ok(QuestionInput {
            question_id,
            text: self.text.clone(),
            header: self.header.clone(),
            multi_select: false,
            options,
        })
    }
}

/// One typed Codex question request: a blocking `requestUserInput` frame,
/// or the questions of an async `agentMessage`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CodexQuestionRequest {
    /// The JSON-RPC request id the answer reply must echo; [`None`] for an
    /// async request, which nothing waits on: its answers travel back as a
    /// user message steered into the turn.
    rpc_id: Option<Value>,
    /// The questionnaire identity every question of the request shares.
    group_id: String,
    questions: Vec<CodexQuestion>,
}

impl CodexQuestionRequest {
    /// Returns the questions in this request group.
    #[cfg(test)]
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
    ThreadName {
        thread_id: String,
        title: artisan_domain::ThreadTitle,
    },
    AgentMessageSnapshot {
        thread_id: String,
        item_id: String,
        text: String,
        phase: artisan_domain::AssistantMessagePhase,
    },
    /// One streamed fragment of an agent message. `thread_id` is the thread
    /// it claims, absent when the frame names none.
    AgentMessageDelta {
        thread_id: Option<String>,
        item_id: String,
        turn_id: String,
        delta: String,
    },
    /// A turn starting or settling on `thread_id`, absent when the frame
    /// names none. A child agent's turn is never the root turn.
    TurnState {
        thread_id: Option<String>,
        turn_id: String,
        state: CodexTurnState,
        /// Why a failed turn failed, when a policy screen refused it.
        refusal: Option<CodexSafety>,
    },
    /// A safety screen holding the response, releasing it, rerouting the
    /// request, or refusing it (`model/safetyBuffering/updated`,
    /// `model/rerouted`, or a policy `error`).
    Safety {
        thread_id: String,
        turn_id: String,
        safety: CodexSafety,
    },
    /// Cumulative token usage from one `thread/tokenUsage/updated` frame.
    ///
    /// Projected best-effort onto the shared usage vocabulary; an empty
    /// measurement never becomes a report (see [`parse_thread_token_usage`]).
    TokenUsage {
        thread_id: Option<String>,
        turn_id: String,
        sample: CodexTokenUsageSample,
    },
    ApprovalRequested(CodexApprovalRequest),
    QuestionRequested(CodexQuestionRequest),
    /// The questions of an async `agentMessage` (`request_user_input_async`):
    /// the tool already returned, so the model keeps working while they
    /// wait. They replace the item's prose, which only restates them.
    AsyncQuestionsRequested {
        thread_id: String,
        request: CodexQuestionRequest,
    },
    /// A subagent's thread started (`thread/started` naming a parent).
    SubagentDiscovered {
        agent_thread_id: String,
        parent_thread_id: String,
        /// The name Codex gave the agent, or its role when it has none.
        name: Option<String>,
    },
    /// One step of a `collabAgentToolCall` item: the agent on `thread_id`
    /// spawning, messaging, waiting on, or closing subagents.
    AgentCall {
        thread_id: String,
        turn_id: String,
        /// Whether the call spawned the receiving agents.
        spawn: bool,
        /// The agents the call addressed.
        receivers: Vec<String>,
        /// Where each addressed agent stands, when Codex reported it.
        states: Vec<(String, CodexToolAction)>,
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
    /// One image the engine generated: a completed, successful
    /// `imageGeneration` item. Images it only looked at (`imageView`) are
    /// steps of its work and show nothing; an agent shows an image with an
    /// `artisan-image` block.
    ImageShown {
        thread_id: String,
        turn_id: String,
        item_id: String,
        image: CodexImage,
    },
    /// One provider-neutral plan update (`turn/plan/updated` or an
    /// `item/started` / `item/completed` plan item).
    PlanUpdated {
        thread_id: String,
        turn_id: String,
        entries: Vec<CodexPlanEntry>,
    },
    /// One context-compaction signal: a `contextCompaction` item lifecycle
    /// step or the deprecated `thread/compacted` notification. Never a turn
    /// settlement.
    Compaction {
        thread_id: String,
        turn_id: String,
        signal: CodexCompactionSignal,
    },
    /// Where the thread's goal stands (`thread/goal/updated`), or that it was
    /// cleared (`thread/goal/cleared`). A goal belongs to the thread; it
    /// can change between turns as well as during one.
    Goal {
        thread_id: String,
        state: artisan_domain::GoalState,
        objective: Option<String>,
    },
    /// A decoded-known frame that intentionally emits no observation: started
    /// reasoning items, the section-zero `summaryPartAdded` opener, and
    /// private `item/reasoning/textDelta` content (never surfaced).
    ActivitySilent,
    ThreadClosed,
    OptedOut,
    UnknownMethod,
    /// One MCP server's startup state (`mcpServer/startupStatus/updated`).
    /// It belongs to the engine, not the turn: it updates the Forge's
    /// record of the engine's connectors.
    McpServerStatus {
        name: String,
        status: artisan_domain::ConnectorStatus,
    },
}

/// One image Codex generated, saved by the engine (`savedPath`) or inline
/// as base64 (`result`), with the prompt the model actually used.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CodexImage {
    pub(crate) saved_path: Option<String>,
    pub(crate) base64: Option<String>,
    pub(crate) revised_prompt: Option<String>,
}

/// One step of Codex's safety screening, in the shared vocabulary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CodexSafety {
    pub(crate) action: artisan_domain::SafetyAction,
    pub(crate) category: artisan_domain::SafetyCategory,
    /// Codex's own code: the `codexErrorInfo`, reroute reason, or first
    /// buffering reason.
    pub(crate) provider_code: Option<String>,
    pub(crate) message: Option<String>,
    /// The model a reroute moved the request to.
    pub(crate) model: Option<String>,
}

/// Reads the policy area out of a Codex signal such as
/// `highRiskCyberActivity` or a buffering use case.
fn safety_category_of<'a>(
    signals: impl IntoIterator<Item = &'a str>,
) -> artisan_domain::SafetyCategory {
    use artisan_domain::SafetyCategory;
    for signal in signals {
        let signal = signal.to_ascii_lowercase();
        if signal.contains("cyber") {
            return SafetyCategory::Cyber;
        }
        if signal.contains("bio") {
            return SafetyCategory::Bio;
        }
        if signal.contains("chem") {
            return SafetyCategory::Chemical;
        }
    }
    SafetyCategory::Unspecified
}

/// Decodes a policy refusal from a `TurnError` (`turn.error` on a failed
/// turn, or an `error` notification's `error`). Other failures, such as
/// limits or transport errors, are not refusals and decode to [`None`].
fn decode_policy_refusal(error: &Value) -> Option<CodexSafety> {
    use artisan_domain::SafetyCategory;
    let code = error.get("codexErrorInfo")?.as_str()?;
    let category = match code {
        "cyberPolicy" => SafetyCategory::Cyber,
        "bioPolicy" => SafetyCategory::Bio,
        _ if code.ends_with("Policy") || code.ends_with("PolicyViolation") => {
            SafetyCategory::Policy
        }
        _ => return None,
    };
    let message = bounded_text(error, "message").or_else(|| {
        error
            .get("misalignment")
            .and_then(|details| bounded_text(details, "detailedExplanation"))
    });
    Some(CodexSafety {
        action: artisan_domain::SafetyAction::Refused,
        category,
        provider_code: Some(code.to_owned()),
        message,
        model: None,
    })
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

/// Decodes `thread/goal/updated`: `{threadId, turnId?, goal: {objective,
/// status, ...}}`. Codex's statuses fold onto the shared ones: a pause, a
/// block, and an exhausted usage or token budget all wait on the reader;
/// `complete` is met.
fn decode_goal_updated(params: &Value) -> Option<CodexEvent> {
    use artisan_domain::GoalState;
    let thread_id = raw_text(params, "threadId")?;
    let goal = params.get("goal")?;
    let state = match goal.get("status")?.as_str()? {
        "active" => GoalState::Active,
        "paused" | "blocked" | "usageLimited" | "budgetLimited" => GoalState::Paused,
        "complete" => GoalState::Met,
        _ => return None,
    };
    let objective = goal
        .get("objective")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty() && text.len() <= artisan_domain::OBSERVATION_TEXT_MAX_BYTES)
        .map(str::to_owned);
    Some(CodexEvent::Goal {
        thread_id,
        state,
        objective,
    })
}

/// Extracts the exact claimed `(threadId, turnId)` scope of one activity
/// frame. Both identities must be present and non-empty: activity never
/// invents scope, and scope-free frames stay [`CodexEvent::UnknownMethod`].
fn claimed_scope(params: &Value) -> Option<(String, String)> {
    Some((raw_text(params, "threadId")?, raw_text(params, "turnId")?))
}

/// Extracts one bounded non-empty provider string from a nested item object.
/// Reads `requestUserInput` options: `[{label, description}]`, bounded.
fn labelled_options(options: Option<&Value>) -> Vec<(String, Option<String>)> {
    options
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .take(CODEX_MAX_OPTIONS_PER_QUESTION)
                .filter_map(|option| {
                    let label = option.get("label")?.as_str()?;
                    if label.is_empty() || label.len() > CODEX_MAX_TEXT_FIELD_BYTES {
                        return None;
                    }
                    let description = option
                        .get("description")
                        .and_then(Value::as_str)
                        .filter(|description| {
                            !description.is_empty()
                                && description.len() <= CODEX_MAX_TEXT_FIELD_BYTES
                        })
                        .map(str::to_owned);
                    Some((label.to_owned(), description))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Reads the questions of an async `agentMessage`: `[{title, options?}]`,
/// options as bare labels, bounded. The item is the questionnaire, and
/// each question is keyed by its position, the only identity it has.
fn async_question_request(
    item_id: &str,
    questions: Option<&Value>,
) -> Option<CodexQuestionRequest> {
    let questions: Vec<CodexQuestion> = questions?
        .as_array()?
        .iter()
        .take(CODEX_MAX_QUESTIONS_PER_FRAME)
        .enumerate()
        .filter_map(|(index, question)| {
            let title = question.get("title")?.as_str()?;
            if title.is_empty() || title.len() > CODEX_MAX_TEXT_FIELD_BYTES {
                return None;
            }
            let options = question
                .get("options")
                .and_then(Value::as_array)
                .map(|options| {
                    options
                        .iter()
                        .take(CODEX_MAX_OPTIONS_PER_QUESTION)
                        .filter_map(Value::as_str)
                        .filter(|label| {
                            !label.is_empty() && label.len() <= CODEX_MAX_TEXT_FIELD_BYTES
                        })
                        .map(|label| (label.to_owned(), None))
                        .collect()
                })
                .unwrap_or_default();
            Some(CodexQuestion {
                question_id: format!("{item_id}:{index}"),
                provider_id: index.to_string(),
                group_id: item_id.to_owned(),
                text: title.to_owned(),
                header: None,
                options,
            })
        })
        .collect();
    (!questions.is_empty()).then(|| CodexQuestionRequest {
        rpc_id: None,
        group_id: item_id.to_owned(),
        questions,
    })
}

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
        "thread/name/updated" => {
            match (
                params.get("threadId").and_then(Value::as_str),
                params.get("threadName").and_then(Value::as_str),
            ) {
                (Some(thread_id), Some(title)) if !thread_id.is_empty() => {
                    artisan_domain::ThreadTitle::parse(title.to_owned()).map_or(
                        CodexEvent::UnknownMethod,
                        |title| CodexEvent::ThreadName {
                            thread_id: thread_id.to_owned(),
                            title,
                        },
                    )
                }
                _ => CodexEvent::UnknownMethod,
            }
        }

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
                thread_id: raw_text(params, "threadId"),
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
            let refusal = match state {
                CodexTurnState::Failed => params
                    .get("turn")
                    .and_then(|turn| turn.get("error"))
                    .and_then(decode_policy_refusal),
                _ => None,
            };
            CodexEvent::TurnState {
                thread_id: raw_text(params, "threadId"),
                turn_id,
                state,
                refusal,
            }
        }
        "model/safetyBuffering/updated" => {
            let Some((thread_id, turn_id)) = claimed_scope(params) else {
                return CodexEvent::UnknownMethod;
            };
            let Some(holding) = params.get("showBufferingUi").and_then(Value::as_bool) else {
                return CodexEvent::UnknownMethod;
            };
            let signals: Vec<&str> = ["useCases", "reasons"]
                .iter()
                .filter_map(|field| params.get(*field).and_then(Value::as_array))
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            let reason = params
                .get("reasons")
                .and_then(Value::as_array)
                .and_then(|reasons| reasons.first())
                .and_then(Value::as_str)
                .filter(|reason| {
                    !reason.is_empty()
                        && reason.len() <= artisan_domain::OBSERVATION_PROVIDER_CODE_MAX_BYTES
                })
                .map(str::to_owned);
            CodexEvent::Safety {
                thread_id,
                turn_id,
                safety: CodexSafety {
                    action: if holding {
                        artisan_domain::SafetyAction::Holding
                    } else {
                        artisan_domain::SafetyAction::Released
                    },
                    category: safety_category_of(signals),
                    provider_code: reason,
                    message: None,
                    model: None,
                },
            }
        }
        "model/rerouted" => {
            let Some((thread_id, turn_id)) = claimed_scope(params) else {
                return CodexEvent::UnknownMethod;
            };
            let reason = raw_text(params, "reason");
            CodexEvent::Safety {
                thread_id,
                turn_id,
                safety: CodexSafety {
                    action: artisan_domain::SafetyAction::Rerouted,
                    category: safety_category_of(reason.as_deref()),
                    provider_code: reason,
                    message: None,
                    model: bounded_text(params, "toModel"),
                },
            }
        }
        "error" => {
            let Some((thread_id, turn_id)) = claimed_scope(params) else {
                return CodexEvent::UnknownMethod;
            };
            match params.get("error").and_then(decode_policy_refusal) {
                Some(safety) => CodexEvent::Safety {
                    thread_id,
                    turn_id,
                    safety,
                },
                None => CodexEvent::UnknownMethod,
            }
        }
        "thread/tokenUsage/updated" => {
            let turn_id = raw_text(params, "turnId").unwrap_or_default();
            if turn_id.is_empty() {
                return CodexEvent::UnknownMethod;
            }
            match parse_thread_token_usage(params) {
                Some(sample) => CodexEvent::TokenUsage {
                    thread_id: raw_text(params, "threadId"),
                    turn_id,
                    sample,
                },
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
                rpc_id: envelope.get("id").cloned().unwrap_or(Value::Null),
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
                rpc_id: envelope.get("id").cloned().unwrap_or(Value::Null),
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
            // The request blocks until its reply carries every answer, so
            // it must have an id to answer under.
            let Some(rpc_id) = envelope.get("id").cloned() else {
                return CodexEvent::UnknownMethod;
            };
            // The questionnaire is the request's tool item; the JSON-RPC id
            // stands in when an older server omits it.
            let group_id = raw_text(params, "itemId")
                .filter(|item| !item.is_empty())
                .or_else(|| envelope.get("id").map(|id| format!("request-{id}")))
                .unwrap_or_default();
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
                            if id.is_empty() || text.is_empty() || group_id.is_empty() {
                                return None;
                            }
                            // Provider ids are short per-request keys; the
                            // questionnaire prefix keeps them unique on the
                            // thread, where questions outlive their request.
                            Some(CodexQuestion {
                                question_id: format!("{group_id}:{id}"),
                                provider_id: id.to_owned(),
                                group_id: group_id.clone(),
                                text: text.to_owned(),
                                header: item
                                    .get("header")
                                    .and_then(Value::as_str)
                                    .filter(|header| !header.is_empty())
                                    .map(str::to_owned),
                                options: labelled_options(item.get("options")),
                            })
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            if questions.is_empty() {
                return CodexEvent::UnknownMethod;
            }
            CodexEvent::QuestionRequested(CodexQuestionRequest {
                rpc_id: Some(rpc_id),
                group_id,
                questions,
            })
        }
        "thread/started" => {
            // Only a subagent's thread names a parent.
            let Some(thread) = params.get("thread").and_then(Value::as_object) else {
                return CodexEvent::UnknownMethod;
            };
            let (Some(agent_thread_id), Some(parent_thread_id)) = (
                nested_text(thread, "id"),
                nested_text(thread, "parentThreadId"),
            ) else {
                return CodexEvent::UnknownMethod;
            };
            CodexEvent::SubagentDiscovered {
                agent_thread_id,
                parent_thread_id,
                name: nested_text(thread, "agentNickname")
                    .or_else(|| nested_text(thread, "agentRole")),
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
        "thread/goal/updated" => decode_goal_updated(params).unwrap_or(CodexEvent::UnknownMethod),
        "mcpServer/startupStatus/updated" => {
            decode_mcp_server_status(params).unwrap_or(CodexEvent::UnknownMethod)
        }
        "thread/goal/cleared" => match raw_text(params, "threadId") {
            Some(thread_id) => CodexEvent::Goal {
                thread_id,
                state: artisan_domain::GoalState::Cleared,
                objective: None,
            },
            None => CodexEvent::UnknownMethod,
        },
        "thread/compacted" => match claimed_scope(params) {
            Some((thread_id, turn_id)) => CodexEvent::Compaction {
                thread_id,
                turn_id,
                signal: CodexCompactionSignal::ThreadCompacted,
            },
            None => CodexEvent::UnknownMethod,
        },
        _ => {
            if CODEX_OPT_OUT_NOTIFICATION_METHODS.contains(&method) {
                return CodexEvent::OptedOut;
            }
            CodexEvent::UnknownMethod
        }
    }
}

/// The skills a `skills/list` reply reports for the run's project; skills
/// turned off in Codex are left out.
pub(crate) fn skills_list_inventory(line: &str) -> Option<InventoryReport> {
    use artisan_domain::{InventorySkill, InventorySkillSource};

    let value: Value = serde_json::from_str(line).ok()?;
    let entries = value.get("result")?.get("data")?.as_array()?;
    let mut skills = Vec::new();
    for skill in entries
        .iter()
        .filter_map(|entry| entry.get("skills").and_then(Value::as_array))
        .flatten()
    {
        if skill.get("enabled").and_then(Value::as_bool) == Some(false) {
            continue;
        }
        let Some(name) = skill
            .get("name")
            .and_then(Value::as_str)
            .filter(|name| artisan_domain::valid_inventory_name(name))
        else {
            continue;
        };
        let description = trimmed_text(
            skill
                .get("interface")
                .and_then(|face| face.get("shortDescription")),
        )
        .or_else(|| trimmed_text(skill.get("shortDescription")))
        .or_else(|| trimmed_text(skill.get("description")))
        .unwrap_or_default();
        let plugin = trimmed_text(skill.get("pluginId"))
            .filter(|plugin| artisan_domain::valid_inventory_name(plugin))
            .map(str::to_owned);
        let source = if plugin.is_some() {
            InventorySkillSource::Plugin
        } else {
            match trimmed_text(skill.get("scope")) {
                Some("repo") => InventorySkillSource::Project,
                Some("user") => InventorySkillSource::User,
                _ => InventorySkillSource::System,
            }
        };
        skills.push(InventorySkill {
            name: name.to_owned(),
            description: bounded_description(description),
            source,
            plugin,
        });
    }
    Some(InventoryReport {
        skills: Some(skills),
        connectors: crate::engine_inventory::ConnectorReport::Unchanged,
    })
}

fn trimmed_text(value: Option<&Value>) -> Option<&str> {
    value.and_then(Value::as_str).map(str::trim)
}

fn bounded_description(description: &str) -> String {
    let single_line = description.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut end = single_line
        .len()
        .min(artisan_domain::EXTENSION_DESCRIPTION_MAX_BYTES);
    while !single_line.is_char_boundary(end) {
        end -= 1;
    }
    single_line[..end].to_owned()
}

/// Decodes one MCP server's startup state. A server that needs its user to
/// sign in again reports `failed` with `reauthenticationRequired`.
fn decode_mcp_server_status(params: &Value) -> Option<CodexEvent> {
    let name = params.get("name")?.as_str()?;
    if !artisan_domain::valid_inventory_name(name) {
        return None;
    }
    let reauthenticate =
        params.get("failureReason").and_then(Value::as_str) == Some("reauthenticationRequired");
    let status = match params.get("status")?.as_str()? {
        "ready" => artisan_domain::ConnectorStatus::Connected,
        "starting" => artisan_domain::ConnectorStatus::Pending,
        "failed" if reauthenticate => artisan_domain::ConnectorStatus::NeedsAuth,
        "failed" => artisan_domain::ConnectorStatus::Failed,
        "cancelled" => artisan_domain::ConnectorStatus::Disabled,
        _ => return None,
    };
    Some(CodexEvent::McpServerStatus {
        name: name.to_owned(),
        status,
    })
}

/// Maps a `CollabAgentStatus` to where the subagent stands. An agent that
/// was shut down had finished; one interrupted or lost did not.
fn collab_agent_action(status: &str) -> Option<CodexToolAction> {
    match status {
        "pendingInit" | "running" => Some(CodexToolAction::Progress),
        "completed" | "shutdown" => Some(CodexToolAction::Completed),
        "errored" | "interrupted" | "notFound" => Some(CodexToolAction::Failed),
        _ => None,
    }
}

/// Decodes one `item/started` / `item/completed` envelope into a typed
/// activity event, mirroring the TypeScript item union.
///
/// Only item types with a valid existing domain observation normalize:
/// reasoning (published summary only), tool (`mcpToolCall` /
/// `dynamicToolCall`), `commandExecution`, `fileChange`, `webSearch`, and
/// plan, `contextCompaction` lifecycle steps, and completed
/// `imageGeneration` items (`imageView` is acknowledged silently). Agent messages preserve
/// authoritative text and phase alongside the delta path, and
/// `collabAgentToolCall` items report the subagents a call addressed. User
/// items remain [`CodexEvent::UnknownMethod`] without disturbing the turn.
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
        "agentMessage" if !item_id.is_empty() => {
            if item.get("delivery").and_then(Value::as_str) == Some("async")
                && let Some(request) = async_question_request(&item_id, item.get("questions"))
            {
                return CodexEvent::AsyncQuestionsRequested { thread_id, request };
            }
            let phase = match item.get("phase").and_then(Value::as_str) {
                Some("commentary") => artisan_domain::AssistantMessagePhase::Commentary,
                Some("final_answer") => artisan_domain::AssistantMessagePhase::Final,
                _ => artisan_domain::AssistantMessagePhase::Unspecified,
            };
            CodexEvent::AgentMessageSnapshot {
                thread_id,
                item_id,
                text: item
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned(),
                phase,
            }
        }
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
        "collabAgentToolCall" => {
            let Some(tool) = item.get("tool").and_then(Value::as_str) else {
                return CodexEvent::UnknownMethod;
            };
            let receivers = item
                .get("receiverThreadIds")
                .and_then(Value::as_array)
                .map(|ids| {
                    ids.iter()
                        .filter_map(Value::as_str)
                        .filter(|id| !id.is_empty() && id.len() <= CODEX_MAX_TEXT_FIELD_BYTES)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default();
            let states = item
                .get("agentsStates")
                .and_then(Value::as_object)
                .map(|states| {
                    states
                        .iter()
                        .filter(|(id, _)| !id.is_empty() && id.len() <= CODEX_MAX_TEXT_FIELD_BYTES)
                        .filter_map(|(id, state)| {
                            let status = state.get("status").and_then(Value::as_str)?;
                            Some((id.clone(), collab_agent_action(status)?))
                        })
                        .collect()
                })
                .unwrap_or_default();
            CodexEvent::AgentCall {
                thread_id,
                turn_id,
                spawn: tool == "spawnAgent",
                receivers,
                states,
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
        // Looking at an image is a step of the work, never a preview.
        "imageView" => {
            if item_id.is_empty() {
                return CodexEvent::UnknownMethod;
            }
            CodexEvent::ActivitySilent
        }
        "imageGeneration" => {
            if item_id.is_empty() {
                return CodexEvent::UnknownMethod;
            }
            if started {
                return CodexEvent::ActivitySilent;
            }
            // A failed generation (a usage limit, a refusal) shows nothing;
            // the turn's own error path reports why.
            let failed = item
                .get("failure")
                .is_some_and(|failure| !failure.is_null())
                || matches!(
                    item.get("status").and_then(Value::as_str),
                    Some("failed" | "error" | "cancelled")
                );
            let saved_path = nested_text(item, "savedPath");
            let base64 = item
                .get("result")
                .and_then(Value::as_str)
                .filter(|result| !result.is_empty())
                .map(str::to_owned);
            if failed || (saved_path.is_none() && base64.is_none()) {
                return CodexEvent::ActivitySilent;
            }
            CodexEvent::ImageShown {
                thread_id,
                turn_id,
                item_id,
                image: CodexImage {
                    saved_path,
                    base64,
                    revised_prompt: nested_text(item, "revisedPrompt"),
                },
            }
        }
        "contextCompaction" => {
            if item_id.is_empty() {
                return CodexEvent::UnknownMethod;
            }
            let signal = if started {
                CodexCompactionSignal::ItemStarted {
                    item_id,
                    started_at_ms: params.get("startedAtMs").and_then(Value::as_u64),
                }
            } else {
                CodexCompactionSignal::ItemCompleted {
                    item_id,
                    completed_at_ms: params.get("completedAtMs").and_then(Value::as_u64),
                }
            };
            CodexEvent::Compaction {
                thread_id,
                turn_id,
                signal,
            }
        }
        _ => CodexEvent::UnknownMethod,
    }
}

/// One subagent thread seen during the turn.
#[derive(Debug)]
struct CodexSubagent {
    thread_id: String,
    parent_thread_id: String,
    name: Option<String>,
    /// Where its row last said it stood; [`None`] until it is shown.
    shown: Option<CodexToolAction>,
}

/// One subagent row to show: the root agent's subagent, by thread.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CodexSubagentRow {
    pub(crate) thread_id: String,
    pub(crate) name: Option<String>,
    pub(crate) action: CodexToolAction,
}

/// In-memory pending interaction tracker for one live Codex turn.
///
/// Permission requests land as pending approvals and `requestUserInput`
/// frames land as pending questions. Resolutions apply through the durable
/// resolve path; a deny records the decision with no turn side effect while
/// the run continues. Subagents are tracked so their frames never reach the
/// root channel and the root agent's own subagents show as agent rows.
#[derive(Debug, Default)]
pub(crate) struct CodexPendingTracker {
    approvals: HashMap<String, CodexApprovalRequest>,
    /// Open question requests by questionnaire, each with the answers
    /// recorded so far by provider question id.
    question_requests: HashMap<String, PendingQuestionRequest>,
    subagents: Vec<CodexSubagent>,
    /// The root native thread bound by the pump from `thread/start` (or the
    /// resumed thread). Rich activity frames must claim exactly this thread;
    /// an unbound tracker emits no activity at all.
    native_thread_id: Option<String>,
    /// Pairs `contextCompaction` items with the deprecated
    /// `thread/compacted` echo so one compaction settles once.
    compactions: CodexCompactionTracker,
    /// The `skills/list` request whose reply is still due.
    skills_request: Option<u64>,
    /// Whether Codex holds the response back for a safety check: the turn
    /// is alive though silent, so it must not settle as a stall.
    safety_hold: bool,
    /// The turn whose policy refusal was already reported, so the `error`
    /// notification and the failed turn report it once.
    refusal_reported: Option<String>,
}

impl CodexPendingTracker {
    /// Creates an empty tracker for one turn.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Expects the reply to the `skills/list` request `request_id`.
    pub(crate) fn expect_skills_reply(&mut self, request_id: u64) {
        self.skills_request = Some(request_id);
    }

    /// Takes `line` when it is the expected `skills/list` reply, success or
    /// error alike, so the probe can never fail the turn. The caller reads
    /// the reply with [`skills_list_inventory`].
    pub(crate) fn take_skills_reply(&mut self, line: &str) -> bool {
        let taken = self
            .skills_request
            .is_some_and(|request_id| super::wire::codex_response_id_matches(line, request_id));
        if taken {
            self.skills_request = None;
        }
        taken
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

    /// Returns the turn's compaction pairing state.
    pub(crate) fn compactions_mut(&mut self) -> &mut CodexCompactionTracker {
        &mut self.compactions
    }

    ///
    /// The decision uses the app-server v2 vocabulary of the
    /// `item/commandExecution/requestApproval` and
    /// `item/fileChange/requestApproval` responses (`accept`, `decline`).
    /// Codex rejects any other spelling and aborts the turn.
    pub(crate) fn approval_reply(&self, id: &str, approved: bool) -> Option<Value> {
        let request = self.approvals.get(id)?;
        Some(serde_json::json!({
            "id": request.rpc_id,
            "result": {"decision": if approved { "accept" } else { "decline" }},
        }))
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

    /// Notes one question request; re-noting the same questionnaire is a
    /// no-op. Returns the questions newly asked.
    ///
    /// Each question is validated through the domain constructor first, so
    /// an out-of-bound provider frame never reaches the durable rows; a
    /// request whose questions all fail validation is not tracked, since
    /// nothing could ever answer it.
    pub(crate) fn note_question_request(
        &mut self,
        request: &CodexQuestionRequest,
    ) -> Vec<CodexQuestion> {
        if self.question_requests.contains_key(&request.group_id) {
            return Vec::new();
        }
        let asked: Vec<CodexQuestion> = request
            .questions
            .iter()
            .filter(|question| question.to_domain_input().is_ok())
            .cloned()
            .collect();
        if asked.is_empty() {
            return Vec::new();
        }
        self.question_requests.insert(
            request.group_id.clone(),
            PendingQuestionRequest {
                rpc_id: request.rpc_id.clone(),
                questions: asked.clone(),
                answers: HashMap::new(),
            },
        );
        asked
    }

    /// Records the durable answer to one question of an open request.
    ///
    /// Returns the handoff once every question of the request has an
    /// answer, and [`None`] while others are still open; the request then
    /// leaves the tracker. Unknown or already answered questions are
    /// refused.
    ///
    /// # Errors
    ///
    /// Returns [`CodexTurnError::Configuration`] when no open request holds
    /// the question.
    pub(crate) fn record_question_answer(
        &mut self,
        question_id: &str,
        answers: &[String],
    ) -> Result<Option<CodexQuestionHandoff>, CodexTurnError> {
        let Some((group_id, request)) = self
            .question_requests
            .iter_mut()
            .find(|(_, request)| request.is_open(question_id))
        else {
            return Err(CodexTurnError::Configuration);
        };
        let Some(provider_id) = request.provider_id(question_id) else {
            return Err(CodexTurnError::Configuration);
        };
        request.answers.insert(provider_id, answers.to_vec());
        if request.answers.len() < request.questions.len() {
            return Ok(None);
        }
        let group_id = group_id.clone();
        let request = self
            .question_requests
            .remove(&group_id)
            .ok_or(CodexTurnError::Configuration)?;
        Ok(Some(request.handoff()))
    }

    /// Returns whether a frame claiming `thread_id` speaks for the root
    /// thread. A frame that names no thread does; so does every frame while
    /// no root is bound.
    pub(crate) fn claims_root(&self, thread_id: Option<&str>) -> bool {
        if let (Some(claimed), Some(root)) = (thread_id, self.native_thread_id.as_deref()) {
            return claimed == root;
        }
        true
    }

    /// Notes one subagent thread, never adopting the root turn.
    ///
    /// Returns the row to show when the root agent's own subagent is new or
    /// was renamed; a subagent of a subagent is tracked but never shown.
    pub(crate) fn note_subagent(
        &mut self,
        agent_thread_id: &str,
        parent_thread_id: &str,
        name: Option<&str>,
    ) -> Option<CodexSubagentRow> {
        let known = self
            .subagents
            .iter()
            .position(|agent| agent.thread_id == agent_thread_id);
        let index = if let Some(index) = known {
            let agent = &mut self.subagents[index];
            if name.is_none() || agent.name.as_deref() == name {
                return None;
            }
            agent.name = name.map(str::to_owned);
            index
        } else {
            self.subagents.push(CodexSubagent {
                thread_id: agent_thread_id.to_owned(),
                parent_thread_id: parent_thread_id.to_owned(),
                name: name.map(str::to_owned),
                shown: None,
            });
            self.subagents.len() - 1
        };
        let action = self.subagents[index]
            .shown
            .unwrap_or(CodexToolAction::Started);
        self.show_subagent(index, action)
    }

    /// Records where a subagent stands. Returns its row when the root
    /// agent's own subagent moved; unknown threads and unchanged states show
    /// nothing.
    pub(crate) fn advance_subagent(
        &mut self,
        agent_thread_id: &str,
        action: CodexToolAction,
    ) -> Option<CodexSubagentRow> {
        let index = self
            .subagents
            .iter()
            .position(|agent| agent.thread_id == agent_thread_id)?;
        if self.subagents[index].shown == Some(action) {
            return None;
        }
        self.show_subagent(index, action)
    }

    fn show_subagent(&mut self, index: usize, action: CodexToolAction) -> Option<CodexSubagentRow> {
        let root = self.native_thread_id.as_deref()?;
        let agent = &mut self.subagents[index];
        if agent.parent_thread_id != root {
            return None;
        }
        agent.shown = Some(action);
        Some(CodexSubagentRow {
            thread_id: agent.thread_id.clone(),
            name: agent.name.clone(),
            action,
        })
    }

    /// Returns whether a native thread identity belongs to a discovered
    /// child agent. Activity frames claiming a child thread never reach the
    /// root channel: child content travels the transcript projection, never
    /// as root activity.
    pub(crate) fn is_known_child_thread(&self, thread_id: &str) -> bool {
        self.subagents
            .iter()
            .any(|agent| agent.thread_id == thread_id)
    }

    /// Resolves one approval; returns whether it was pending.
    ///
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
            .map(|request| request.questions.len() - request.answers.len())
            .sum()
    }

    /// Returns whether the provider is waiting on the user: an open
    /// approval or blocking question keeps the turn alive however long the
    /// user takes. Async questions block nothing, so they never count.
    pub(crate) fn waiting_on_user(&self) -> bool {
        !self.approvals.is_empty()
            || self
                .question_requests
                .values()
                .any(|request| request.rpc_id.is_some())
    }

    /// Whether the turn may stay silent without settling as a stall: the
    /// user owes an answer, or Codex holds the response for a safety check.
    pub(crate) fn stall_exempt(&self) -> bool {
        self.waiting_on_user() || self.safety_hold
    }

    /// Follows a safety step; returns whether it should be reported, which
    /// a refusal already reported for its turn is not.
    pub(crate) fn observe_safety(&mut self, turn_id: &str, safety: &CodexSafety) -> bool {
        use artisan_domain::SafetyAction;
        self.safety_hold = safety.action == SafetyAction::Holding;
        if safety.action != SafetyAction::Refused {
            return true;
        }
        if self.refusal_reported.as_deref() == Some(turn_id) {
            return false;
        }
        self.refusal_reported = Some(turn_id.to_owned());
        true
    }

    /// Returns the number of discovered subagents.
    #[cfg(test)]
    pub(crate) fn subagent_count(&self) -> usize {
        self.subagents.len()
    }
}

/// How a fully answered question request goes back to Codex.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum CodexQuestionHandoff {
    /// The JSON-RPC reply line answering a blocking `requestUserInput`.
    Reply(String),
    /// The user message answering async questions, steered into the turn;
    /// [`None`] when every question was skipped, which needs no message.
    Message(Option<String>),
}

/// One open question request with the answers recorded so far.
#[derive(Debug)]
struct PendingQuestionRequest {
    rpc_id: Option<Value>,
    questions: Vec<CodexQuestion>,
    /// Answers by provider question id; a skipped question records an
    /// empty list.
    answers: HashMap<String, Vec<String>>,
}

impl PendingQuestionRequest {
    fn provider_id(&self, question_id: &str) -> Option<String> {
        self.questions
            .iter()
            .find(|question| question.question_id == question_id)
            .map(|question| question.provider_id.clone())
    }

    /// Whether the request holds the question and it has no answer yet.
    fn is_open(&self, question_id: &str) -> bool {
        self.provider_id(question_id)
            .is_some_and(|provider_id| !self.answers.contains_key(&provider_id))
    }

    /// The handoff answering the request: the JSON-RPC reply for a
    /// blocking request, the user message for an async one.
    fn handoff(&self) -> CodexQuestionHandoff {
        match &self.rpc_id {
            Some(rpc_id) => CodexQuestionHandoff::Reply(self.reply_line(rpc_id)),
            None => CodexQuestionHandoff::Message(self.answer_message()),
        }
    }

    /// The JSON-RPC reply that answers the request: every answered
    /// question by its provider id, skipped questions left out (Codex treats
    /// a missing answer as "continue with best judgment").
    fn reply_line(&self, rpc_id: &Value) -> String {
        let mut map = serde_json::Map::new();
        for (provider_id, answers) in self
            .answers
            .iter()
            .filter(|(_, answers)| !answers.is_empty())
            .take(CODEX_MAX_ANSWERS)
        {
            map.insert(
                provider_id.clone(),
                serde_json::json!({ "answers": answers }),
            );
        }
        serde_json::json!({ "id": rpc_id, "result": { "answers": Value::Object(map) } }).to_string()
    }

    /// The user message answering async questions: each answered question
    /// quoted above its answer, in the order asked. Skipped questions are
    /// left out; the model already went on with its own default for them.
    fn answer_message(&self) -> Option<String> {
        let answered: Vec<String> = self
            .questions
            .iter()
            .filter_map(|question| {
                let answers = self.answers.get(&question.provider_id)?;
                if answers.is_empty() {
                    return None;
                }
                let quoted = question
                    .text
                    .lines()
                    .map(|line| format!("> {line}"))
                    .collect::<Vec<_>>()
                    .join("\n");
                Some(format!("{quoted}\n\n{}", answers.join(", ")))
            })
            .collect();
        (!answered.is_empty()).then(|| answered.join("\n\n"))
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

/// Encode each attachment as native image input, never as a filename in text.
///
/// Shared by `turn/start` and `turn/steer`: both take the same ordered
/// `UserInput` array, and an image-only message carries no empty text item.
pub(crate) fn prompt_input(text: &str, images: &[artisan_domain::ImageAttachment]) -> Value {
    let mut content = Vec::new();
    if !text.is_empty() || images.is_empty() {
        content.push(serde_json::json!({"type":"text", "text":text, "text_elements":[]}));
    }
    for image in images {
        content.push(serde_json::json!({"type":"image", "url":format!(
            "data:{};base64,{}", image.mime_type_str(),
            base64::engine::general_purpose::STANDARD.encode(image.bytes()))}));
    }
    Value::Array(content)
}
