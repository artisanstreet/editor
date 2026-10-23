//! The `codex app-server` frames Artisan reads, as typed structs: each line
//! is parsed once into these and decoded from them.
//!
//! Shapes follow Codex rust-v0.162.1's generated v2 schema
//! (`codex-rs/app-server-protocol/schema/typescript/v2`) and the recordings
//! in `transcripts/codex`. Unknown fields are ignored and unknown methods,
//! item types and statuses parse as `Unknown`, so a newer Codex adding
//! something never fails a run. Fields the decoder can do without, or
//! checks itself so it can say what was missing, are `Option`.

use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;

/// A collection Codex may send as `null` when empty.
fn nullable<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

/// One line, before its params are read.
#[derive(Deserialize)]
pub(super) struct Frame {
    pub method: Option<String>,
    pub id: Option<Value>,
    #[serde(default)]
    pub params: Value,
    pub result: Option<Value>,
    pub error: Option<Value>,
}

/// A notification Codex sends.
pub(super) enum Notification {
    ThreadStarted(ThreadStarted),
    ThreadNameUpdated(ThreadNameUpdated),
    TurnStarted(TurnStarted),
    TurnCompleted(TurnCompleted),
    ItemStarted(ItemNotice),
    ItemCompleted(ItemNotice),
    /// `item/agentMessage/delta` and `item/plan/delta`.
    TextDelta(Delta),
    ReasoningDelta(Delta),
    ReasoningPart(ReasoningPart),
    OutputDelta(Delta),
    McpProgress(McpProgress),
    PlanUpdated(PlanUpdated),
    TokenUsageUpdated(TokenUsageUpdated),
    Rerouted(Rerouted),
    SafetyBuffering(SafetyBuffering),
    GoalUpdated(GoalUpdated),
    GoalCleared(Scoped),
    Error(ErrorNotice),
    Warning(Warning),
    RequestResolved(RequestResolved),
    /// A method Artisan doesn't read.
    Unknown,
}

impl Notification {
    pub(super) fn parse(method: &str, params: &Value) -> Result<Self, serde_json::Error> {
        fn read<'de, T: Deserialize<'de>>(params: &'de Value) -> Result<T, serde_json::Error> {
            T::deserialize(params)
        }
        Ok(match method {
            "thread/started" => Self::ThreadStarted(read(params)?),
            "thread/name/updated" => Self::ThreadNameUpdated(read(params)?),
            "turn/started" => Self::TurnStarted(read(params)?),
            "turn/completed" => Self::TurnCompleted(read(params)?),
            "item/started" => Self::ItemStarted(read(params)?),
            "item/completed" => Self::ItemCompleted(read(params)?),
            "item/agentMessage/delta" | "item/plan/delta" => Self::TextDelta(read(params)?),
            "item/reasoning/summaryTextDelta" => Self::ReasoningDelta(read(params)?),
            "item/reasoning/summaryPartAdded" => Self::ReasoningPart(read(params)?),
            "item/commandExecution/outputDelta" => Self::OutputDelta(read(params)?),
            "item/mcpToolCall/progress" => Self::McpProgress(read(params)?),
            "turn/plan/updated" => Self::PlanUpdated(read(params)?),
            "thread/tokenUsage/updated" => Self::TokenUsageUpdated(read(params)?),
            "model/rerouted" => Self::Rerouted(read(params)?),
            "model/safetyBuffering/updated" => Self::SafetyBuffering(read(params)?),
            "thread/goal/updated" => Self::GoalUpdated(read(params)?),
            "thread/goal/cleared" => Self::GoalCleared(read(params)?),
            "error" => Self::Error(read(params)?),
            "warning" => Self::Warning(read(params)?),
            "serverRequest/resolved" => Self::RequestResolved(read(params)?),
            _ => Self::Unknown,
        })
    }

    /// The thread it concerns; `None` for one about no thread in particular.
    pub(super) fn thread(&self) -> Option<&str> {
        Some(match self {
            Self::ThreadStarted(_) | Self::Unknown => return None,
            Self::ThreadNameUpdated(notice) => &notice.thread_id,
            Self::TurnStarted(notice) => &notice.thread_id,
            Self::TurnCompleted(notice) => &notice.thread_id,
            Self::ItemStarted(notice) | Self::ItemCompleted(notice) => &notice.thread_id,
            Self::TextDelta(delta) | Self::ReasoningDelta(delta) | Self::OutputDelta(delta) => {
                &delta.thread_id
            }
            Self::ReasoningPart(notice) => &notice.thread_id,
            Self::McpProgress(notice) => &notice.thread_id,
            Self::PlanUpdated(notice) => &notice.thread_id,
            Self::TokenUsageUpdated(notice) => &notice.thread_id,
            Self::Rerouted(notice) => &notice.thread_id,
            Self::SafetyBuffering(notice) => &notice.thread_id,
            Self::GoalUpdated(notice) => &notice.thread_id,
            Self::GoalCleared(notice) => &notice.thread_id,
            Self::Error(notice) => &notice.thread_id,
            Self::Warning(notice) => notice.thread_id.as_deref()?,
            Self::RequestResolved(notice) => notice.thread_id.as_deref()?,
        })
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Scoped {
    pub thread_id: String,
}

#[derive(Deserialize)]
pub(super) struct ThreadStarted {
    pub thread: ThreadInfo,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ThreadInfo {
    pub id: String,
    pub parent_thread_id: Option<String>,
    pub agent_nickname: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ThreadNameUpdated {
    pub thread_id: String,
    pub thread_name: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TurnStarted {
    pub thread_id: String,
    pub turn: TurnId,
}

#[derive(Deserialize)]
pub(super) struct TurnId {
    pub id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TurnCompleted {
    pub thread_id: String,
    pub turn: CompletedTurn,
}

#[derive(Deserialize)]
pub(super) struct CompletedTurn {
    /// `inProgress` is a schema status, but never a terminal one.
    pub status: TurnStatus,
    pub error: Option<CodexError>,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum TurnStatus {
    Completed,
    Interrupted,
    Failed,
    /// `inProgress`, or one Artisan doesn't know: `turn/completed` says the
    /// turn ended, and that is how it reads.
    #[serde(other)]
    Ended,
}

/// Codex's error shape, on failed turns and `error` notifications.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct CodexError {
    pub message: Option<String>,
    /// A code such as `cyberPolicy`; objects for other kinds.
    pub codex_error_info: Option<Value>,
}

impl CodexError {
    /// Policy blocks (`cyberPolicy`, `misalignmentPolicyViolation`, …).
    pub(super) fn is_refusal(&self) -> bool {
        self.codex_error_info
            .as_ref()
            .and_then(Value::as_str)
            .is_some_and(|code| code.ends_with("Policy") || code.ends_with("PolicyViolation"))
    }
    /// Whether the failure passes by itself. Codex's own retries already
    /// ran; what is left is capacity or connection trouble, or the plan's
    /// usage running out (whose reset Codex doesn't say here).
    pub(super) fn retry(&self) -> Option<crate::Retry> {
        let transient = |status: &Value| {
            status["httpStatusCode"]
                .as_u64()
                .is_none_or(|code| code == 429 || code >= 500)
        };
        match self.codex_error_info.as_ref()? {
            Value::String(code) => match code.as_str() {
                "usageLimitExceeded" => Some(crate::Retry::UsageLimit { resets_at: None }),
                "serverOverloaded" | "internalServerError" | "rateLimitExceeded" => {
                    Some(crate::Retry::Overloaded)
                }
                _ => None,
            },
            Value::Object(info) => {
                let (kind, status) = info.iter().next()?;
                matches!(
                    kind.as_str(),
                    "httpConnectionFailed"
                        | "responseStreamConnectionFailed"
                        | "responseStreamDisconnected"
                        | "responseTooManyFailedAttempts"
                )
                .then_some(crate::Retry::Overloaded)
                .filter(|_| transient(status))
            }
            Value::Null | Value::Bool(_) | Value::Number(_) | Value::Array(_) => None,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ItemNotice {
    pub thread_id: String,
    pub item: Item,
}

/// An item of a thread, by its `type`.
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub(super) enum Item {
    AgentMessage {
        id: String,
        text: Option<String>,
        phase: Option<MessagePhase>,
        /// Questions the message asks without waiting; `null` when none.
        questions: Option<Value>,
    },
    Plan {
        id: String,
        text: Option<String>,
    },
    UserMessage {
        #[serde(default, deserialize_with = "nullable")]
        content: Vec<UserInput>,
    },
    #[serde(rename_all = "camelCase")]
    CommandExecution {
        id: String,
        status: Option<ExecutionStatus>,
        command: Option<String>,
        cwd: Option<String>,
        aggregated_output: Option<String>,
        exit_code: Option<i64>,
    },
    FileChange {
        id: String,
        status: Option<ExecutionStatus>,
        changes: Option<Vec<FileChange>>,
    },
    McpToolCall {
        id: String,
        status: Option<CallStatus>,
        server: Option<String>,
        tool: Option<String>,
        #[serde(default)]
        arguments: Value,
        result: Option<McpResult>,
        error: Option<CodexError>,
    },
    #[serde(rename_all = "camelCase")]
    DynamicToolCall {
        id: String,
        status: Option<CallStatus>,
        success: Option<bool>,
        tool: Option<String>,
        #[serde(default)]
        arguments: Value,
        #[serde(default)]
        content_items: Value,
    },
    WebSearch {
        id: String,
        query: Option<String>,
        action: Option<WebAction>,
    },
    ImageView {
        id: String,
        path: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    ImageGeneration {
        id: String,
        saved_path: Option<String>,
        revised_prompt: Option<String>,
    },
    ContextCompaction {
        id: String,
    },
    CollabAgentToolCall(CollabCall),
    #[serde(rename_all = "camelCase")]
    Sleep {
        id: String,
        duration_ms: Option<u64>,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum MessagePhase {
    Commentary,
    PartialAnswer,
    FinalAnswer,
    #[serde(other)]
    Unknown,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub(super) enum UserInput {
    Text {
        text: String,
    },
    /// Images, audio, skills and mentions carry no authored text.
    #[serde(other)]
    Other,
}

/// `CommandExecutionStatus` and `PatchApplyStatus`.
#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum ExecutionStatus {
    InProgress,
    Completed,
    Failed,
    Declined,
    #[serde(other)]
    Unknown,
}

/// `McpToolCallStatus` and `DynamicToolCallStatus`: no `declined`.
#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum CallStatus {
    InProgress,
    Completed,
    Failed,
    #[serde(other)]
    Unknown,
}

#[derive(Deserialize)]
pub(super) struct FileChange {
    pub path: String,
    pub kind: PatchKind,
    #[serde(default)]
    pub diff: String,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub(super) enum PatchKind {
    Add,
    Delete,
    Update {
        /// Snake case on the wire, unlike the rest of the schema.
        move_path: Option<String>,
    },
}

#[derive(Deserialize)]
pub(super) struct McpResult {
    /// MCP content blocks.
    #[serde(default)]
    pub content: Value,
}

#[derive(Deserialize)]
pub(super) struct WebAction {
    pub query: Option<String>,
    pub url: Option<String>,
    #[serde(default, deserialize_with = "nullable")]
    pub queries: Vec<Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct CollabCall {
    pub id: String,
    pub tool: CollabTool,
    pub status: CollabStatus,
    #[serde(default, deserialize_with = "nullable")]
    pub receiver_thread_ids: Vec<String>,
    pub prompt: Option<String>,
    pub description: Option<String>,
    #[serde(default, deserialize_with = "nullable")]
    pub agents_states: HashMap<String, AgentState>,
}

#[derive(Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum CollabTool {
    SpawnAgent,
    SendInput,
    Wait,
    #[serde(other)]
    Other,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum CollabStatus {
    InProgress,
    Completed,
    Failed,
    Interrupted,
    #[serde(other)]
    Unknown,
}

#[derive(Deserialize)]
pub(super) struct AgentState {
    pub message: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Delta {
    pub thread_id: String,
    pub item_id: Option<String>,
    #[serde(rename = "delta")]
    pub text: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ReasoningPart {
    pub thread_id: String,
    pub summary_index: Option<u64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct McpProgress {
    pub thread_id: String,
    pub item_id: Option<String>,
    pub message: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct PlanUpdated {
    pub thread_id: String,
    /// Read entry by entry, so a malformed plan is reported, not skipped.
    #[serde(default)]
    pub plan: Value,
}

#[derive(Deserialize)]
pub(super) struct PlanEntry {
    pub step: String,
    pub status: PlanStatus,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum PlanStatus {
    Pending,
    InProgress,
    Completed,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TokenUsageUpdated {
    pub thread_id: String,
    pub token_usage: TokenUsage,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TokenUsage {
    /// One model call; `total` is the whole thread, history included.
    pub last: TokenCounts,
    pub model_context_window: Option<u64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
#[expect(clippy::struct_field_names, reason = "the schema's names")]
pub(super) struct TokenCounts {
    pub total_tokens: u64,
    pub input_tokens: u64,
    pub cached_input_tokens: u64,
    /// Left out by Codex versions that don't count cache writes.
    #[serde(default)]
    pub cache_write_input_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_output_tokens: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Rerouted {
    pub thread_id: String,
    pub turn_id: String,
    pub from_model: String,
    pub to_model: String,
    /// `highRiskCyberActivity` on 0.162.1.
    pub reason: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SafetyBuffering {
    pub thread_id: String,
    pub turn_id: String,
    pub show_buffering_ui: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct GoalUpdated {
    pub thread_id: String,
    pub goal: Goal,
}

#[derive(Deserialize)]
pub(super) struct Goal {
    pub objective: String,
    /// `active`, `paused`, `blocked`, `usageLimited`, `budgetLimited`,
    /// `complete`.
    pub status: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ErrorNotice {
    pub thread_id: String,
    pub error: CodexError,
    #[serde(default)]
    pub will_retry: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Warning {
    pub thread_id: Option<String>,
    pub message: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RequestResolved {
    pub thread_id: Option<String>,
    pub request_id: Value,
}

/// A request Codex makes of the client.
pub(super) enum Request {
    CommandApproval(CommandApproval),
    FileChangeApproval(Approval),
    PermissionsApproval(PermissionsApproval),
    UserInput(UserInputRequest),
    Elicitation(Elicitation),
    /// A method Artisan doesn't serve.
    Unknown,
}

impl Request {
    /// The thread it concerns; MCP servers may ask outside any thread.
    pub(super) fn thread(&self) -> Option<&str> {
        match self {
            Self::CommandApproval(request) => Some(&request.thread_id),
            Self::FileChangeApproval(request) => Some(&request.thread_id),
            Self::PermissionsApproval(request) => Some(&request.thread_id),
            Self::UserInput(request) => Some(&request.thread_id),
            Self::Elicitation(request) => request.thread_id.as_deref(),
            Self::Unknown => None,
        }
    }

    pub(super) fn parse(method: &str, params: &Value) -> Result<Self, serde_json::Error> {
        Ok(match method {
            "item/commandExecution/requestApproval" => {
                Self::CommandApproval(Deserialize::deserialize(params)?)
            }
            "item/fileChange/requestApproval" => {
                Self::FileChangeApproval(Deserialize::deserialize(params)?)
            }
            "item/permissions/requestApproval" => {
                Self::PermissionsApproval(Deserialize::deserialize(params)?)
            }
            "item/tool/requestUserInput" => Self::UserInput(Deserialize::deserialize(params)?),
            "mcpServer/elicitation/request" => Self::Elicitation(Deserialize::deserialize(params)?),
            _ => Self::Unknown,
        })
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct CommandApproval {
    pub thread_id: String,
    pub item_id: Option<String>,
    pub command: Option<String>,
    pub cwd: Option<String>,
    pub reason: Option<String>,
    /// `command`, or `writeStdin` for input to a running command.
    pub kind: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Approval {
    pub thread_id: String,
    pub item_id: Option<String>,
    pub reason: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct PermissionsApproval {
    pub thread_id: String,
    pub item_id: Option<String>,
    pub reason: Option<String>,
    /// Granted back as asked, minus nulls.
    #[serde(default)]
    pub permissions: Value,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct UserInputRequest {
    pub thread_id: String,
    pub item_id: Option<String>,
    /// Codex 0.162.1 always sends it; a request is answered either way.
    #[serde(default = "blocking")]
    pub is_blocking: bool,
    #[serde(default, deserialize_with = "nullable")]
    pub questions: Vec<UserQuestion>,
}

const fn blocking() -> bool {
    true
}

#[derive(Deserialize)]
pub(super) struct UserQuestion {
    pub id: Option<String>,
    pub header: Option<String>,
    pub question: Option<String>,
    pub options: Option<Vec<UserOption>>,
}

#[derive(Deserialize)]
pub(super) struct UserOption {
    pub label: Option<String>,
    pub description: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Elicitation {
    pub thread_id: Option<String>,
    pub server_name: String,
    pub message: String,
}

/// Questions inside an agent message.
#[derive(Deserialize)]
pub(super) struct AsyncQuestion {
    pub title: String,
    /// `null` for a question answered in the user's own words.
    #[serde(default, deserialize_with = "nullable")]
    pub options: Vec<String>,
}
