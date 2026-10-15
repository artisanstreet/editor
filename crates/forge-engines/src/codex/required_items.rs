//! Required consumed fields from Codex rust-v0.162.1 generated v2 schema.
//! https://github.com/openai/codex/tree/rust-v0.162.1/codex-rs/app-server-protocol/schema/typescript/v2
//! Optional metadata and the optional description extension remain outside
//! these projections. Validation runs before scope or deferred state changes.
use anyhow::Context as _;
use serde_json::Value;

// Borrowed projections of consumed fields from the generated v2 schema.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct CompletedTurn<'a> {
    pub thread_id: &'a str,
    #[serde(borrow)]
    pub turn: TerminalTurn<'a>,
}
#[derive(serde::Deserialize)]
pub(super) struct TerminalTurn<'a> {
    id: &'a str,
    pub status: TerminalStatus,
}
#[derive(Clone, Copy, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum TerminalStatus {
    Completed,
    Interrupted,
    Failed,
}
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct CollabCall<'a> {
    id: &'a str,
    #[serde(borrow)]
    pub receiver_thread_ids: Vec<&'a str>,
    pub tool: CollabTool,
    pub status: CollabStatus,
    pub prompt: Option<&'a str>,
    pub description: Option<&'a str>,
    #[serde(borrow)]
    pub agents_states: std::collections::HashMap<&'a str, AgentReply<'a>>,
}
#[derive(serde::Deserialize)]
struct UserMessage<'a> {
    id: &'a str,
    #[serde(borrow)]
    content: Vec<UserInput<'a>>,
}
#[derive(serde::Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum UserInput<'a> {
    Text { text: &'a str },
    Image,
    LocalImage,
    Audio,
    LocalAudio,
    Skill,
    Mention,
}
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct StartedTurn<'a> {
    thread_id: &'a str,
    #[serde(borrow)]
    pub turn: TurnIdentity<'a>,
}
#[derive(serde::Deserialize)]
pub(super) struct TurnIdentity<'a> {
    pub id: &'a str,
}
#[derive(serde::Deserialize)]
pub(super) struct StartedThread<'a> {
    #[serde(borrow)]
    pub thread: ThreadIdentity<'a>,
}
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ThreadIdentity<'a> {
    pub id: &'a str,
    pub parent_thread_id: Option<&'a str>,
    pub agent_nickname: Option<&'a str>,
}
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct UsageUpdated<'a> {
    thread_id: &'a str,
    #[serde(rename = "tokenUsage")]
    _token_usage: TokenUsage,
}
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TokenUsage {
    pub last: TokenCounts,
    pub model_context_window: Option<u64>,
}
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TokenCounts {
    pub total_tokens: u64,
    pub input_tokens: u64,
    pub cached_input_tokens: u64,
    pub cache_write_input_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_output_tokens: u64,
}
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ErrorNotification<'a> {
    thread_id: &'a str,
    #[serde(borrow)]
    pub error: ErrorText<'a>,
    pub will_retry: bool,
}
#[derive(serde::Deserialize)]
pub(super) struct ErrorText<'a> {
    pub message: &'a str,
}
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Elicitation<'a> {
    thread_id: &'a str,
    pub server_name: &'a str,
    pub message: &'a str,
}
pub(super) fn validate_elicitation(params: &Value) -> anyhow::Result<()> {
    let value = <Elicitation as serde::Deserialize>::deserialize(params)
        .context("Codex elicitation requires threadId, serverName and message")?;
    native_identity(value.thread_id, "threadId")
}

fn native_identity(value: &str, field: &str) -> anyhow::Result<()> {
    anyhow::ensure!(!value.trim().is_empty(), "Codex omitted required {field}");
    Ok(())
}
pub(super) fn validate_consumed_items(method: &str, params: &Value) -> anyhow::Result<()> {
    match method {
        "turn/started" => {
            let value = <StartedTurn as serde::Deserialize>::deserialize(params)
                .context("Codex turn/started requires threadId and turn.id")?;
            native_identity(value.thread_id, "threadId")?;
            native_identity(value.turn.id, "turn.id")?;
        }
        "thread/started" => {
            let value = <StartedThread as serde::Deserialize>::deserialize(params)
                .context("Codex thread/started requires thread.id")?;
            native_identity(value.thread.id, "thread.id")?;
            if let Some(parent) = value.thread.parent_thread_id {
                native_identity(parent, "parentThreadId")?;
            }
        }
        "thread/tokenUsage/updated" => {
            let value = <UsageUpdated as serde::Deserialize>::deserialize(params)
                .context("Codex token usage requires explicit last counts")?;
            native_identity(value.thread_id, "threadId")?;
        }
        "error" => {
            let value = <ErrorNotification as serde::Deserialize>::deserialize(params)
                .context("Codex error requires threadId, message and willRetry")?;
            native_identity(value.thread_id, "threadId")?;
        }
        "turn/completed" => {
            // inProgress is a schema TurnStatus, but is invalid for a terminal
            // notification. Reject before clearing the active turn or reply.
            let value = <CompletedTurn as serde::Deserialize>::deserialize(params)
                .context("Codex turn/completed requires threadId, turn.id and terminal status")?;
            native_identity(value.thread_id, "threadId")?;
            native_identity(value.turn.id, "turn.id")?;
        }
        "item/started" | "item/completed" => {
            let item = &params["item"];
            match item["type"].as_str() {
                Some("agentMessage") => {
                    let value: AssistantText = serde::Deserialize::deserialize(item)
                        .context("Codex agentMessage requires text and a valid nullable phase")?;
                    native_identity(value.id, "item.id")?;
                }
                Some("plan") => {
                    let value: PlanText = serde::Deserialize::deserialize(item)
                        .context("Codex plan requires text")?;
                    native_identity(value.id, "item.id")?;
                }
                Some("commandExecution" | "fileChange" | "mcpToolCall" | "dynamicToolCall") => {
                    let value = <NativeToolStatus as serde::Deserialize>::deserialize(item)
                        .context("Codex tool Item requires a valid native status")?;
                    native_identity(value.id(), "item.id")?;
                    validate_tool_fields(item)?;
                }
                Some("collabAgentToolCall") => {
                    let value = <CollabCall as serde::Deserialize>::deserialize(item)
                        .context("Codex collabAgentToolCall requires receiverThreadIds strings")?;
                    native_identity(value.id, "item.id")?;
                    for receiver in value.receiver_thread_ids {
                        native_identity(receiver, "receiverThreadIds entry")?;
                    }
                }
                Some("userMessage") => {
                    let value = <UserMessage as serde::Deserialize>::deserialize(item)
                        .context("Codex userMessage requires valid content")?;
                    native_identity(value.id, "item.id")?;
                }
                _ => return Ok(()),
            }
            native_identity(
                params["threadId"]
                    .as_str()
                    .context("Codex item requires threadId")?,
                "threadId",
            )?;
        }
        _ => {}
    }
    Ok(())
}

pub(super) fn user_text(item: &Value) -> Option<String> {
    let message = <UserMessage as serde::Deserialize>::deserialize(item).ok()?;
    let text: Vec<_> = message
        .content
        .into_iter()
        .filter_map(|part| match part {
            UserInput::Text { text } => Some(text),
            _ => None, // Images, audio, skills and mentions carry no authored text.
        })
        .collect();
    (!text.is_empty()).then(|| text.join("\n"))
}

// CommandExecutionStatus and PatchApplyStatus share their four wire variants;
// McpToolCallStatus and DynamicToolCallStatus share three. Keep that distinction
// so a native declined MCP/dynamic status is not silently accepted.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
enum ExecutionStatus {
    InProgress,
    Completed,
    Failed,
    Declined,
}
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
enum CallStatus {
    InProgress,
    Completed,
    Failed,
}
#[derive(serde::Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum NativeToolStatus<'a> {
    CommandExecution {
        id: &'a str,
        status: ExecutionStatus,
    },
    FileChange {
        id: &'a str,
        status: ExecutionStatus,
    },
    McpToolCall {
        id: &'a str,
        status: CallStatus,
    },
    DynamicToolCall {
        id: &'a str,
        status: CallStatus,
        success: Option<bool>,
    },
}
impl NativeToolStatus<'_> {
    fn id(&self) -> &str {
        match self {
            Self::CommandExecution { id, .. }
            | Self::FileChange { id, .. }
            | Self::McpToolCall { id, .. }
            | Self::DynamicToolCall { id, .. } => id,
        }
    }
    fn canonical(&self, completed: bool) -> artisan_api::Status {
        use artisan_api::Status;
        match self {
            Self::CommandExecution { status, .. } | Self::FileChange { status, .. } => match status
            {
                ExecutionStatus::InProgress => Status::Running,
                ExecutionStatus::Completed => Status::Done,
                ExecutionStatus::Failed => Status::Failed,
                ExecutionStatus::Declined => Status::Declined,
            },
            Self::DynamicToolCall {
                success: Some(false),
                ..
            } if completed => Status::Failed,
            Self::McpToolCall { status, .. } | Self::DynamicToolCall { status, .. } => match status
            {
                CallStatus::InProgress => Status::Running,
                CallStatus::Completed => Status::Done,
                CallStatus::Failed => Status::Failed,
            },
        }
    }
}
pub(super) fn native_tool_status(item: &Value, completed: bool) -> Option<artisan_api::Status> {
    <NativeToolStatus as serde::Deserialize>::deserialize(item)
        .ok()
        .map(|item| item.canonical(completed))
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum CollabTool {
    SpawnAgent,
    SendInput,
    ResumeAgent,
    Wait,
    CloseAgent,
    SendMessage,
    FollowupTask,
    InterruptAgent,
    ListAgents,
}
#[derive(Clone, Copy, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum CollabStatus {
    InProgress,
    Completed,
    Failed,
    Interrupted,
}
#[derive(serde::Deserialize)]
pub(super) struct AgentReply<'a> {
    pub message: Option<&'a str>,
}
#[derive(serde::Deserialize)]
struct ToolName<'a> {
    tool: &'a str,
}
#[derive(serde::Deserialize)]
struct McpName<'a> {
    server: &'a str,
    tool: &'a str,
}
#[derive(serde::Deserialize)]
struct CommandName<'a> {
    command: &'a str,
}
#[derive(serde::Deserialize)]
struct FileChanges<'a> {
    #[serde(borrow)]
    changes: Vec<NativeFileChange<'a>>,
}
#[derive(serde::Deserialize)]
pub(super) struct NativeFileChange<'a> {
    path: &'a str,
    #[serde(borrow)]
    kind: PatchKind<'a>,
    diff: &'a str,
}
#[derive(Clone, Copy, serde::Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum PatchKind<'a> {
    Add,
    Delete,
    Update { move_path: Option<&'a str> },
}
impl NativeFileChange<'_> {
    pub fn canonical(&self) -> artisan_api::FileChange {
        use artisan_api::FileOp;
        let (path, op) = match self.kind {
            PatchKind::Add => (self.path.to_owned(), FileOp::Add),
            PatchKind::Delete => (self.path.to_owned(), FileOp::Delete),
            PatchKind::Update {
                move_path: Some(to),
            } => (
                to.to_owned(),
                FileOp::Move {
                    from: self.path.to_owned(),
                },
            ),
            PatchKind::Update { move_path: None } => (self.path.to_owned(), FileOp::Modify),
        };
        artisan_api::FileChange {
            path,
            op,
            diff: (!self.diff.is_empty()).then(|| self.diff.into()),
        }
    }
}
#[derive(serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum MessagePhase {
    Commentary,
    PartialAnswer,
    FinalAnswer,
}
#[derive(serde::Deserialize)]
pub(super) struct AssistantText<'a> {
    id: &'a str,
    pub text: &'a str,
    pub phase: Option<MessagePhase>,
}
#[derive(serde::Deserialize)]
struct PlanText<'a> {
    id: &'a str,
    #[serde(rename = "text")]
    _text: &'a str,
}
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct UserInputRequest<'a> {
    thread_id: &'a str,
    item_id: &'a str,
    pub is_blocking: bool,
    #[serde(borrow)]
    #[serde(rename = "questions")]
    _questions: Vec<PromptFields<'a>>,
}
#[derive(serde::Deserialize)]
struct PromptFields<'a> {
    #[serde(rename = "id")]
    _id: &'a str,
    #[serde(rename = "header")]
    _header: &'a str,
    #[serde(rename = "question")]
    _question: &'a str,
    #[serde(borrow)]
    #[serde(rename = "options")]
    _options: Option<Vec<PromptOption<'a>>>,
}
#[derive(serde::Deserialize)]
struct PromptOption<'a> {
    #[serde(rename = "label")]
    _label: &'a str,
    #[serde(rename = "description")]
    _description: &'a str,
}
pub(super) fn validate_user_input_request(params: &Value) -> anyhow::Result<()> {
    let request: UserInputRequest = serde::Deserialize::deserialize(params)
        .context("Codex user input requires isBlocking and typed prompts/options")?;
    native_identity(request.thread_id, "threadId")?;
    native_identity(request.item_id, "itemId")?;
    Ok(())
}
fn validate_tool_fields(item: &Value) -> anyhow::Result<()> {
    match item["type"].as_str() {
        Some("commandExecution") => {
            let value: CommandName = serde::Deserialize::deserialize(item)?;
            native_identity(value.command, "command")?;
        }
        Some("mcpToolCall") => {
            let value: McpName = serde::Deserialize::deserialize(item)?;
            native_identity(value.server, "server")?;
            native_identity(value.tool, "tool")?;
            item.get("arguments")
                .context("Codex MCP call requires arguments (null is valid)")?;
            let _: McpOutput = serde::Deserialize::deserialize(item)
                .context("Codex MCP result/error has invalid consumed output fields")?;
        }
        Some("dynamicToolCall") => {
            let value: ToolName = serde::Deserialize::deserialize(item)?;
            native_identity(value.tool, "tool")?;
            item.get("arguments")
                .context("Codex dynamic call requires arguments (null is valid)")?;
            let _: DynamicOutput = serde::Deserialize::deserialize(item)
                .context("Codex dynamic output has invalid consumed text content")?;
        }
        Some("fileChange") => {
            let value: FileChanges = serde::Deserialize::deserialize(item)?;
            for change in value.changes {
                native_identity(change.path, "change.path")?;
            }
        }
        _ => {}
    }
    Ok(())
}

#[derive(serde::Deserialize)]
struct McpOutput<'a> {
    #[serde(borrow, rename = "error")]
    _error: Option<ErrorText<'a>>,
    #[serde(rename = "result")]
    _result: Option<McpContent>,
}
#[derive(serde::Deserialize)]
struct McpContent {
    // MCP content is Array<JsonValue>; validate the consumed container without
    // inventing a content schema or copying unconsumed arbitrary JSON.
    #[serde(rename = "content")]
    _content: Vec<serde::de::IgnoredAny>,
}
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct DynamicOutput<'a> {
    #[serde(borrow, rename = "contentItems")]
    _content_items: Option<Vec<DynamicPart<'a>>>,
}
#[derive(serde::Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum DynamicPart<'a> {
    InputText {
        #[serde(rename = "text")]
        _text: &'a str,
    },
    InputImage,
    InputAudio,
}
