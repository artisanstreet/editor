use crate::{BlobHash, Collection, EngineId, ItemId, RunId, Seq, TextContent, ThreadId, Timestamp};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sender {
    pub thread: ThreadId,
    pub engine: EngineId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelSwitch {
    pub from: NamedModel,
    pub to: NamedModel,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NamedModel {
    pub engine: EngineId,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Item {
    pub id: ItemId,
    pub thread_id: ThreadId,
    pub position: Seq,
    pub seq: Seq,
    pub run_id: Option<RunId>,
    pub parent_id: Option<ItemId>,
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
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Done | Self::Failed | Self::Declined | Self::Cancelled
        )
    }

    #[must_use]
    pub const fn advance(self, next: Self) -> Self {
        if self.is_terminal() { self } else { next }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ItemKind {
    UserMessage {
        text: TextContent,
        #[serde(default)]
        images: Vec<BlobHash>,
        #[serde(default)]
        steer: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        from_agent: Option<ItemId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sender: Option<Sender>,
    },
    AssistantMessage {
        text: TextContent,
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
        name: Option<String>,
        description: TextContent,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        prompt: Option<TextContent>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        context_used: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        #[serde(default)]
        background: bool,
        result: Option<TextContent>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        engine: Option<EngineId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thread: Option<ThreadId>,
    },
    Question {
        blocking: bool,
        questions: Collection<Question>,
        answers: Option<Collection<Option<QuestionAnswer>>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        answer_text: Option<TextContent>,
    },
    Plan {
        steps: Collection<PlanStep>,
        proposal: Option<TextContent>,
        approval: Option<Approval>,
    },
    Image {
        blob: BlobHash,
        caption: Option<TextContent>,
    },
    Compaction {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        summary: Option<TextContent>,
        tokens_before: Option<u64>,
        tokens_after: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        switch: Option<ModelSwitch>,
    },
    Notice {
        kind: NoticeKind,
        text: TextContent,
    },
    Error {
        kind: ErrorKind,
        message: TextContent,
        #[serde(default)]
        retryable: bool,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Commentary,
    Final,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ToolKind {
    AgentMessage {
        recipient: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<ItemId>,
        summary: Option<String>,
        text: TextContent,
        #[serde(default, skip_serializing_if = "MessageDirection::is_sent")]
        direction: MessageDirection,
    },
    Agents {
        action: AgentAction,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        addresses: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        targets: Vec<ItemId>,
    },
    Shell {
        command: TextContent,
        cwd: Option<String>,
        #[serde(default)]
        background: bool,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        watch: bool,
    },
    Read {
        path: String,
        range: Option<(u64, u64)>,
    },
    Edit {
        changes: Collection<FileChange>,
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
        input: Option<TextContent>,
    },
    ViewImage {
        path: String,
    },
    Workflow {
        name: Option<String>,
        #[serde(default)]
        input: Option<TextContent>,
    },
    Other {
        name: String,
        #[serde(default)]
        input: Option<TextContent>,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageDirection {
    #[default]
    Sent,
    Received,
    Report,
}

impl MessageDirection {
    #[must_use]
    pub fn is_sent(&self) -> bool {
        *self == Self::Sent
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentAction {
    List,
    Wait,
    Stop,
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
    pub diff: Option<TextContent>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum FileOp {
    Add,
    Modify,
    Delete,
    Move { from: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Output {
    pub text: TextContent,
    #[serde(default)]
    pub omitted_bytes: u64,
}
impl Output {
    #[must_use]
    pub fn from_text(text: &str) -> Self {
        Self {
            text: text.into(),
            omitted_bytes: 0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Approval {
    pub request: ApprovalRequest,
    pub reason: Option<TextContent>,
    pub options: Vec<Decision>,
    pub decision: Option<Decision>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ApprovalRequest {
    Command {
        command: TextContent,
        cwd: Option<String>,
    },
    FileChange,
    Action {
        description: TextContent,
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
    pub text: TextContent,
    #[serde(default)]
    pub multi_select: bool,
    pub options: Collection<QuestionOption>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionOption {
    pub id: String,
    pub label: String,
    pub description: Option<TextContent>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum QuestionAnswer {
    Choices { ids: Collection<String> },
    Text { text: String },
}
impl Question {
    pub fn validate_answer_structure(
        &self,
        answer: &Option<QuestionAnswer>,
    ) -> Result<(), &'static str> {
        match answer {
            Some(QuestionAnswer::Text { text }) if text.trim().is_empty() => {
                Err("answer text is empty")
            }
            Some(QuestionAnswer::Text { text }) if text.len() > crate::INLINE_TEXT_BYTES => {
                Err("answer text exceeds 16 KiB")
            }
            None | Some(QuestionAnswer::Text { .. }) => Ok(()),
            Some(QuestionAnswer::Choices { ids }) => {
                let ids = ids.inline().ok_or("submitted choices must be inline")?;
                if ids.is_empty() {
                    return Err("no choice was selected");
                }
                if !self.multi_select && ids.len() > 1 {
                    return Err("question does not allow multiple choices");
                }
                let mut seen = std::collections::HashSet::new();
                for id in ids {
                    if !seen.insert(id) {
                        return Err("choice was selected twice");
                    }
                }
                Ok(())
            }
        }
    }
    pub fn validate_answer(&self, answer: &Option<QuestionAnswer>) -> Result<(), &'static str> {
        self.validate_answer_structure(answer)?;
        if let Some(QuestionAnswer::Choices { ids }) = answer {
            let choices = self
                .options
                .inline()
                .ok_or("question choices must be loaded before validation")?;
            let known = choices
                .iter()
                .map(|option| option.id.as_str())
                .collect::<std::collections::HashSet<_>>();
            for id in ids.inline().ok_or("submitted choices must be inline")? {
                if !known.contains(id.as_str()) {
                    return Err("unknown question choice");
                }
            }
        }
        Ok(())
    }
    pub fn answer_text(&self, answer: &QuestionAnswer) -> Result<String, &'static str> {
        match answer {
            QuestionAnswer::Text { text } => Ok(text.clone()),
            QuestionAnswer::Choices { ids } => {
                let ids = ids
                    .inline()
                    .ok_or("saved choice identities must be loaded before formatting")?;
                let mut labels = std::collections::HashMap::new();
                let choices = self
                    .options
                    .inline()
                    .ok_or("question choices must be loaded before formatting")?;
                let options = choices
                    .iter()
                    .enumerate()
                    .map(|(index, option)| {
                        *labels.entry(option.label.as_str()).or_insert(0usize) += 1;
                        (option.id.as_str(), (index, option))
                    })
                    .collect::<std::collections::HashMap<_, _>>();
                ids.iter()
                    .map(|id| {
                        let (index, option) =
                            options.get(id.as_str()).ok_or("unknown question choice")?;
                        Ok(if labels[option.label.as_str()] > 1 {
                            format!(
                                "{} (option {} of {})",
                                option.label,
                                index + 1,
                                self.options.len()
                            )
                        } else {
                            option.label.clone()
                        })
                    })
                    .collect::<Result<Vec<_>, &'static str>>()
                    .map(|labels| labels.join(", "))
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanStep {
    pub text: TextContent,
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
    YouShouldKnow { heads_up: bool },
    Unsandboxed { reason: UnsandboxedReason },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnsandboxedReason {
    KernelWithoutLandlock,
    Windows,
    MacOs,
    OtherOs,
    StartedWithFullAccess,
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
    #[must_use]
    pub const fn next_status(&self, next: Status) -> Status {
        if matches!(
            self.kind,
            ItemKind::Plan { .. } | ItemKind::Agent { .. } | ItemKind::Question { .. }
        ) {
            next
        } else {
            self.status.advance(next)
        }
    }

    #[must_use]
    pub fn folds_into_work(&self) -> bool {
        match &self.kind {
            ItemKind::UserMessage {
                steer, from_agent, ..
            } => (*steer && self.run_id.is_some()) || from_agent.is_some(),
            ItemKind::AssistantMessage { phase, .. } => *phase != Phase::Final,
            ItemKind::Tool { .. } | ItemKind::Plan { .. } => self.status != Status::AwaitingUser,
            ItemKind::Notice { kind, .. } => !matches!(
                kind,
                NoticeKind::Goal { .. } | NoticeKind::YouShouldKnow { .. }
            ),
            ItemKind::Agent { .. } => true,
            ItemKind::Question { .. }
            | ItemKind::Compaction { .. }
            | ItemKind::Error { .. }
            | ItemKind::Image { .. } => false,
        }
    }

    #[must_use]
    pub fn preview(&self) -> Option<&str> {
        match &self.kind {
            ItemKind::UserMessage { text, .. } | ItemKind::AssistantMessage { text, .. } => {
                text.inline()
            }
            ItemKind::Tool { .. }
            | ItemKind::Agent { .. }
            | ItemKind::Question { .. }
            | ItemKind::Plan { .. }
            | ItemKind::Image { .. }
            | ItemKind::Compaction { .. }
            | ItemKind::Notice { .. }
            | ItemKind::Error { .. } => None,
        }
    }

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

#[cfg(test)]
#[path = "../tests/item/question_identity.rs"]
mod question_identity_tests;

#[cfg(test)]
#[path = "../tests/item/notice.rs"]
mod notice_tests;
