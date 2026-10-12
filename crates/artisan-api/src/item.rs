use crate::{BlobHash, ItemId, RunId, Seq, ThreadId, Timestamp};
use serde::{Deserialize, Serialize};

/// Anything in a thread. Adapters map each engine's wiring onto these kinds.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Item {
    pub id: ItemId,
    pub thread_id: ThreadId,
    /// Seq at creation. Stable; orders the thread and pages the window.
    pub position: Seq,
    /// Seq of the last change. Bumps on every update.
    pub seq: Seq,
    pub run_id: Option<RunId>,
    /// Set when the item belongs to a subagent: the subagent's `Agent` item.
    pub parent_id: Option<ItemId>,
    /// The engine's own id (tool_use_id, Codex item id, ACP toolCallId…),
    /// used only to update the item in place.
    pub native_id: Option<String>,
    pub status: Status,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub kind: ItemKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Running,
    AwaitingUser,
    Done,
    Failed,
    Declined,
    Cancelled,
}

impl Status {
    /// A terminal status never reopens.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Done | Self::Failed | Self::Declined | Self::Cancelled
        )
    }

    /// Applies an update, keeping a terminal status terminal.
    #[must_use]
    pub const fn advance(self, next: Self) -> Self {
        if self.is_terminal() { self } else { next }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ItemKind {
    UserMessage {
        text: String,
        #[serde(default)]
        images: Vec<BlobHash>,
        /// Sent while a run was active.
        #[serde(default)]
        steer: bool,
    },
    AssistantMessage {
        text: String,
        phase: Phase,
    },
    Tool {
        tool: ToolKind,
        title: Option<String>,
        output: Option<Output>,
        exit_code: Option<i32>,
        approval: Option<Approval>,
    },
    Agent {
        /// The engine's name for it: a type ("general-purpose"), a nickname
        /// or nothing.
        name: Option<String>,
        description: String,
        /// The full task it was given, when the engine reports one apart
        /// from the description (Claude's `prompt`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        prompt: Option<String>,
        /// The engine's own id for the running agent (Claude's `agentId`,
        /// the address its `SendMessage` uses).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent_id: Option<String>,
        #[serde(default)]
        background: bool,
        result: Option<String>,
        /// Artisan's name for it, unique in its thread ([`AGENT_NAMES`]):
        /// Forge assigns it when the agent is first stored. `None` on agents
        /// stored before; read [`Item::agent_label`].
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
    },
    Question {
        /// The engine waits for the answer.
        blocking: bool,
        questions: Vec<Question>,
        /// One entry per question; `None` means skipped.
        answers: Option<Vec<Option<Vec<String>>>>,
    },
    Plan {
        steps: Vec<PlanStep>,
        /// A markdown plan awaiting approval.
        proposal: Option<String>,
        approval: Option<Approval>,
    },
    Image {
        blob: BlobHash,
        caption: Option<String>,
    },
    Compaction {
        tokens_before: Option<u64>,
        tokens_after: Option<u64>,
    },
    Notice {
        kind: NoticeKind,
        text: String,
    },
    Error {
        kind: ErrorKind,
        message: String,
        #[serde(default)]
        retryable: bool,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// Text between tool calls; folds away once the answer arrives.
    Commentary,
    /// The reply.
    Final,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ToolKind {
    Shell {
        command: String,
        cwd: Option<String>,
        #[serde(default)]
        background: bool,
    },
    Read {
        path: String,
        range: Option<(u64, u64)>,
    },
    Edit {
        changes: Vec<FileChange>,
    },
    Search {
        pattern: String,
        path: Option<String>,
        mode: SearchMode,
    },
    WebFetch {
        url: String,
    },
    WebSearch {
        query: String,
    },
    Mcp {
        server: String,
        tool: String,
        #[serde(default)]
        input: Option<String>,
    },
    ViewImage {
        path: String,
    },
    Other {
        name: String,
        #[serde(default)]
        input: Option<String>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchMode {
    Grep,
    Glob,
    List,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileChange {
    pub path: String,
    pub op: FileOp,
    /// Unified diff, when the engine provides one.
    pub diff: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum FileOp {
    Add,
    Modify,
    Delete,
    Move { from: String },
}

/// Tool output kept for display: the head and tail of what the tool printed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Output {
    pub head: String,
    pub tail: String,
    /// Bytes dropped between head and tail.
    pub truncated_bytes: u64,
}

impl Output {
    /// Bytes kept from each end.
    pub const KEEP: usize = 8 * 1024;

    /// Keeps the first and last [`Self::KEEP`] bytes on char boundaries.
    #[must_use]
    pub fn from_text(text: &str) -> Self {
        if text.len() <= Self::KEEP * 2 {
            return Self {
                head: text.to_owned(),
                tail: String::new(),
                truncated_bytes: 0,
            };
        }
        let mut head_end = Self::KEEP;
        while !text.is_char_boundary(head_end) {
            head_end -= 1;
        }
        let mut tail_start = text.len() - Self::KEEP;
        while !text.is_char_boundary(tail_start) {
            tail_start += 1;
        }
        Self {
            head: text[..head_end].to_owned(),
            tail: text[tail_start..].to_owned(),
            truncated_bytes: (tail_start - head_end) as u64,
        }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.head.is_empty() && self.tail.is_empty()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Approval {
    pub request: ApprovalRequest,
    pub reason: Option<String>,
    /// What the engine offers; the client shows only these.
    pub options: Vec<Decision>,
    pub decision: Option<Decision>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ApprovalRequest {
    Command {
        command: String,
        cwd: Option<String>,
    },
    FileChange,
    Action {
        description: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    AllowOnce,
    AllowAlways,
    DenyOnce,
    DenyAlways,
    Cancelled,
}

impl Decision {
    #[must_use]
    pub const fn allows(self) -> bool {
        matches!(self, Self::AllowOnce | Self::AllowAlways)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Question {
    pub id: String,
    pub header: Option<String>,
    pub text: String,
    #[serde(default)]
    pub multi_select: bool,
    pub options: Vec<QuestionOption>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionOption {
    pub label: String,
    pub description: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanStep {
    pub text: String,
    pub status: StepStatus,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    Pending,
    InProgress,
    Done,
    Cancelled,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NoticeKind {
    Goal { state: String },
    Safety,
    Reroute { model: String },
    Info,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    Api,
    Refusal,
    MaxTurns,
    Execution,
}

impl Item {
    /// The text a reader sees first, for previews and titles.
    #[must_use]
    pub fn preview(&self) -> Option<&str> {
        match &self.kind {
            ItemKind::UserMessage { text, .. } | ItemKind::AssistantMessage { text, .. } => {
                Some(text)
            }
            _ => None,
        }
    }

    /// What a subagent is called on screen: Artisan's name for it, else a
    /// stable one from its id (agents stored before Forge named them).
    #[must_use]
    pub fn agent_label(&self) -> Option<String> {
        let ItemKind::Agent { label, .. } = &self.kind else {
            return None;
        };
        Some(
            label
                .clone()
                .unwrap_or_else(|| crate::fallback_agent_name(self.id.as_str()).to_owned()),
        )
    }
}
