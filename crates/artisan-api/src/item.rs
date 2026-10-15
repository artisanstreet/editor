use crate::{BlobHash, Collection, ItemId, RunId, Seq, TextContent, ThreadId, Timestamp};
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
        text: TextContent,
        #[serde(default)]
        images: Vec<BlobHash>,
        /// Sent while a run was active.
        #[serde(default)]
        steer: bool,
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
        /// Set while the engine asks the user before running it, and kept
        /// with the decision; the item is `AwaitingUser` until answered.
        approval: Option<Approval>,
    },
    Agent {
        /// The engine's name for it: a type ("general-purpose"), a nickname
        /// or nothing.
        name: Option<String>,
        description: TextContent,
        /// The full task it was given, when the engine reports one apart
        /// from the description (Claude's `prompt`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        prompt: Option<TextContent>,
        /// The engine's own id for the running agent (Claude's `agentId`,
        /// the address its `SendMessage` uses).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent_id: Option<String>,
        /// Tokens in the agent's context after its latest message, and the
        /// model it runs on: what the context meter shows in its view.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        context_used: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        #[serde(default)]
        background: bool,
        result: Option<TextContent>,
        /// Artisan's name for it, unique in its thread ([`AGENT_NAMES`]):
        /// Forge assigns it when the agent is first stored. `None` on agents
        /// stored before; read [`Item::agent_label`].
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
    },
    Question {
        /// The engine waits for the answer.
        blocking: bool,
        questions: Collection<Question>,
        /// One entry per question; `None` means skipped.
        answers: Option<Collection<Option<QuestionAnswer>>>,
        /// Forge-computed literal history. Clients never resolve opaque IDs or
        /// duplicate labels to construct display text.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        answer_text: Option<TextContent>,
    },
    Plan {
        steps: Collection<PlanStep>,
        /// A markdown plan awaiting approval.
        proposal: Option<TextContent>,
        approval: Option<Approval>,
    },
    Image {
        blob: BlobHash,
        caption: Option<TextContent>,
    },
    Compaction {
        tokens_before: Option<u64>,
        tokens_after: Option<u64>,
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
    /// Text between tool calls; folds away once the answer arrives.
    Commentary,
    /// The reply.
    Final,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ToolKind {
    /// A message to another agent or session.
    AgentMessage {
        /// The engine's address for the recipient (Claude's `to`).
        recipient: String,
        /// The thread's `Agent` item the message went to, when the recipient
        /// is one; Forge resolves it from the address.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<ItemId>,
        summary: Option<String>,
        text: TextContent,
    },
    Shell {
        command: TextContent,
        cwd: Option<String>,
        #[serde(default)]
        background: bool,
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
    Other {
        name: String,
        #[serde(default)]
        input: Option<TextContent>,
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

/// Complete tool output. Historical imports can explicitly report bytes already lost.
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
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.text.byte_len() == 0 && self.omitted_bytes == 0
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Approval {
    pub request: ApprovalRequest,
    pub reason: Option<TextContent>,
    /// What the engine offers; the client shows only these.
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

/// Choice identity stays separate from presentation and free-form answers.
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
            None => Ok(()),
            Some(QuestionAnswer::Text { text }) if text.trim().is_empty() => {
                Err("answer text is empty")
            }
            Some(QuestionAnswer::Text { text }) if text.len() > crate::INLINE_TEXT_BYTES => {
                Err("answer text exceeds 16 KiB")
            }
            Some(QuestionAnswer::Text { .. }) => Ok(()),
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
    /// Readable history/next-turn copy; transport answers retain opaque IDs.
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
    /// Stateful plans, agents and questions can be explicitly reused by an adapter.
    /// Completed output and accepted input remain terminal.
    #[must_use]
    pub fn next_status(&self, next: Status) -> Status {
        if matches!(
            self.kind,
            ItemKind::Plan { .. } | ItemKind::Agent { .. } | ItemKind::Question { .. }
        ) {
            next
        } else {
            self.status.advance(next)
        }
    }

    /// The text a reader sees first, for previews and titles.
    #[must_use]
    pub fn preview(&self) -> Option<&str> {
        match &self.kind {
            ItemKind::UserMessage { text, .. } | ItemKind::AssistantMessage { text, .. } => {
                text.inline()
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

#[cfg(test)]
mod question_identity_tests {
    use super::{Question, QuestionAnswer, QuestionOption};
    #[test]
    fn duplicate_labels_keep_distinct_choice_identity_and_text_is_not_a_choice() {
        let question = Question {
            id: "q".into(),
            header: None,
            text: "Pick".into(),
            multi_select: false,
            options: vec![
                QuestionOption {
                    id: "a".into(),
                    label: "Same".into(),
                    description: None,
                },
                QuestionOption {
                    id: "b".into(),
                    label: "Same".into(),
                    description: None,
                },
            ]
            .into(),
        };
        let answer = QuestionAnswer::Choices {
            ids: vec!["b".into()].into(),
        };
        assert!(question.validate_answer(&Some(answer.clone())).is_ok());
        let encoded = serde_json::to_string(&answer).unwrap();
        assert_eq!(
            serde_json::from_str::<QuestionAnswer>(&encoded).unwrap(),
            answer
        );
        assert!(
            question
                .validate_answer(&Some(QuestionAnswer::Choices {
                    ids: vec!["Same".into()].into()
                }))
                .is_err()
        );
        assert!(
            question
                .validate_answer(&Some(QuestionAnswer::Text {
                    text: "Same".into()
                }))
                .is_ok()
        );
        assert!(
            question
                .validate_answer(&Some(QuestionAnswer::Choices {
                    ids: vec!["a".into(), "b".into()].into()
                }))
                .is_err()
        );
        let mut multi = question;
        multi.multi_select = true;
        assert!(
            multi
                .validate_answer(&Some(QuestionAnswer::Choices {
                    ids: vec!["a".into(), "a".into()].into()
                }))
                .is_err()
        );
    }
}
