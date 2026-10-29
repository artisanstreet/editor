//! Pure decoding of OpenCode's v2 event stream (`GET /api/event`).
//!
//! The global stream carries every session on the server, so the decoder
//! follows the run's root session plus any child session whose parent it
//! already follows; everything else is ignored. Shapes were captured from
//! OpenCode 2.1.0 (see `transcripts/opencode`).

use crate::{EngineEvent, LiveKind};
use anyhow::Context as _;
use artisan_api::{
    Answer, Approval, ApprovalRequest, Collection, Decision, ErrorKind, FileChange, FileOp, ItemId,
    ItemKind, NoticeKind, Output, Phase, Question, QuestionOption, SearchMode, Status, ToolKind,
    Usage,
};
use serde_json::{Map, Value, json};
use std::collections::{HashMap, HashSet};

#[derive(serde::Deserialize)]
struct NativeField {
    key: String,
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    hidden: bool,
    #[serde(default)]
    options: Vec<NativeOption>,
}
#[derive(serde::Deserialize)]
struct NativeOption {
    value: String,
    label: String,
    #[serde(default)]
    description: Option<String>,
}

/// Tool names OpenCode ships; any other name with an underscore is an MCP
/// tool (`<server>_<tool>`).
const BUILTIN_TOOLS: &[&str] = &[
    "edit",
    "execute",
    "glob",
    "grep",
    "question",
    "read",
    "shell",
    "skill",
    "subagent",
    "webfetch",
    "websearch",
    "write",
];

/// An HTTP request the driver must send to answer OpenCode.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reply {
    pub method: Method,
    pub path: String,
    pub body: Option<Value>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Post,
    Delete,
}

#[derive(Default)]
pub struct Decoder {
    reasoning_summary: crate::reasoning::Summary,
    root: Option<String>,
    /// Followed sessions → the `Agent` item their items belong to.
    sessions: HashMap<String, Option<String>>,
    /// Child sessions no `subagent` call has claimed yet → their
    /// `session.created` data, for the Agent item they get if none does.
    unbound: HashMap<String, Value>,
    /// Tool names by call id: `session.tool.called` doesn't repeat it.
    tool_names: HashMap<String, String>,
    /// By OpenCode call id.
    tools: HashMap<String, Tool>,
    /// Background shells awaiting their completion notification → starting call.
    /// `shell.exited` precedes notification admission and must not release this
    /// guard: the server still needs to wake the assistant with the result.
    shells: HashMap<String, String>,
    /// Texts ended in the current step, per session, for the phase decision.
    step_texts: HashMap<String, Vec<(String, String)>>,
    open_texts: HashSet<String>,
    /// Compactions started per session, which keys the running one.
    compactions: HashMap<String, usize>,
    /// OpenCode inbox id → our user message, for steers.
    steers: HashMap<String, ItemId>,
    admitted: HashSet<String>,
    delivered: HashSet<String>,
    turn_open: bool,
    pending: HashMap<String, Pending>,
    usage: Usage,
    model: Option<String>,
    title: Option<String>,
    /// The error the last step failed with, which the execution failure
    /// that follows repeats.
    step_error: Option<String>,
    /// The last error that ended a turn.
    pub error: Option<String>,
    cancelled: bool,
    goals: HashMap<String, Goal>,
    awaiting_goal: Option<GoalChange>,
}

enum GoalChange {
    Set(Goal),
    Clear,
}

#[derive(Clone, PartialEq, Eq, serde::Deserialize)]
pub(super) struct Goal {
    id: String,
    objective: String,
    status: GoalStatus,
    #[serde(rename = "updatedAt")]
    updated_at: u64,
}

#[derive(Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum GoalStatus {
    Active,
    Paused,
    Complete,
}

impl GoalStatus {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Paused => "paused",
            Self::Complete => "complete",
        }
    }
}

struct Tool {
    session: String,
    status: Status,
    kind: ItemKind,
}

enum Pending {
    Permission {
        session: String,
        request: String,
    },
    Form {
        session: String,
        form: String,
        fields: Vec<FormField>,
    },
}

/// A form field asked as one question.
struct FormField {
    key: String,
    kind: FieldKind,
    /// Option label → the value OpenCode wants back.
    values: HashMap<String, String>,
}

#[derive(Clone, Copy)]
enum FieldKind {
    Text,
    Choice,
    Choices,
    Boolean,
    Number,
}

impl Decoder {
    pub fn new(root: Option<String>) -> Self {
        let mut decoder = Self::default();
        if let Some(root) = root {
            decoder.follow_root(root);
        }
        decoder
    }

    #[cfg(test)]
    pub const fn has_root(&self) -> bool {
        self.root.is_some()
    }

    pub fn follow_root(&mut self, session: String) {
        self.sessions.insert(session.clone(), None);
        self.root = Some(session);
    }

    /// The driver sent a prompt OpenCode admitted as inbox item `message`;
    /// `steer` names our user message when it was a steer.
    pub fn prompt_sent(&mut self, message: &str, steer: Option<ItemId>) -> Vec<EngineEvent> {
        self.admitted.insert(message.to_owned());
        match steer {
            Some(id) if self.delivered.contains(message) => vec![EngineEvent::InputAccepted { id }],
            Some(id) => {
                self.steers.insert(message.to_owned(), id);
                Vec::new()
            }
            None => Vec::new(),
        }
    }

    pub fn idle(&self) -> bool {
        !self.turn_open
            && self.awaiting_goal.is_none()
            && (self.error.is_some()
                || self.cancelled
                || !self
                    .root
                    .as_ref()
                    .and_then(|root| self.goals.get(root))
                    .is_some_and(|goal| matches!(goal.status, GoalStatus::Active)))
            && (self.shells.is_empty() || self.error.is_some() || self.cancelled)
            && self
                .admitted
                .iter()
                .all(|message| self.delivered.contains(message))
    }

    pub(super) fn await_goal_change(&mut self, goal: Option<Goal>) {
        self.awaiting_goal = Some(goal.map_or(GoalChange::Clear, GoalChange::Set));
    }

    pub(super) fn goal_changed(&mut self, session: &str, goal: Option<Goal>) -> Vec<EngineEvent> {
        let Some(parent) = self.sessions.get(session).cloned() else {
            return Vec::new();
        };
        let (state, objective) = match goal {
            Some(goal) if !goal.objective.trim().is_empty() => {
                let state = goal.status.as_str();
                let objective = goal.objective.clone();
                self.goals.insert(session.to_owned(), goal);
                (state, objective)
            }
            None => match self.goals.remove(session) {
                Some(goal) => ("cleared", goal.objective),
                None => return Vec::new(),
            },
            Some(_) => return Vec::new(),
        };
        vec![EngineEvent::Item {
            key: parent
                .as_ref()
                .map_or_else(|| "goal".to_owned(), |parent| format!("goal:{parent}")),
            parent,
            status: Status::Done,
            kind: ItemKind::Notice {
                kind: NoticeKind::Goal {
                    state: state.to_owned(),
                },
                text: objective.into(),
            },
        }]
    }

    /// Settles what is still open after the user cancelled.
    pub fn cancel(&mut self) -> Vec<EngineEvent> {
        self.cancelled = true;
        let mut out = Vec::new();
        for key in std::mem::take(&mut self.open_texts) {
            out.push(EngineEvent::Status {
                key,
                status: Status::Cancelled,
            });
        }
        let mut keys: Vec<&String> = self.tools.keys().collect();
        keys.sort();
        for key in keys {
            if !self.tools[key].status.is_terminal() {
                out.push(EngineEvent::Status {
                    key: key.clone(),
                    status: Status::Cancelled,
                });
            }
        }
        out
    }

    /// One SSE `data:` payload.
    pub fn feed(&mut self, data: &str) -> anyhow::Result<Vec<EngineEvent>> {
        #[derive(serde::Deserialize)]
        struct Identity<'a> {
            id: &'a str,
        }
        let event: Value = serde_json::from_str(data).context("decoding OpenCode protocol JSON")?;
        anyhow::ensure!(
            event.is_object(),
            "OpenCode protocol frame must be an object"
        );
        anyhow::ensure!(
            event["type"].is_string(),
            "OpenCode event is missing its type"
        );
        let identity = <Identity as serde::Deserialize>::deserialize(&event)
            .context("decoding OpenCode event identity")?;
        anyhow::ensure!(!identity.id.trim().is_empty(), "OpenCode event ID is empty");
        let events = self.feed_raw(&event);
        let events = self.reasoning_summary.normalize(events, false);
        Ok(crate::retained::settle(self, events))
    }

    #[expect(clippy::too_many_lines, reason = "one arm per OpenCode event")]
    fn feed_raw(&mut self, event: &Value) -> Vec<EngineEvent> {
        let mut out = Vec::new();
        let kind = event["type"].as_str().unwrap_or_default();
        let data = &event["data"];
        match kind {
            // Child sessions announce themselves before any of their events.
            "session.created" => {
                self.session_created(data);
                return out;
            }
            // Shells live outside sessions: a background one reports its exit here.
            "shell.exited" => {
                self.shell_exited(data, &mut out);
                return out;
            }
            "form.created" => {
                self.form_created(event, &data["form"], &mut out);
                return out;
            }
            "form.replied" | "form.cancelled" => {
                self.form_settled(kind == "form.replied", data, &mut out);
                return out;
            }
            _ => {}
        }
        let Some(session) = data["sessionID"].as_str() else {
            return out;
        };
        if matches!(kind, "session.step.started" | "rpc.opencode.goal.changed") {
            self.bind_orphan(session, &mut out);
        }
        let Some(parent) = self.sessions.get(session).cloned() else {
            return out;
        };
        let root = self.root.as_deref() == Some(session);
        match kind {
            "rpc.opencode.goal.changed" => {
                if let Ok(goal) = serde_json::from_value::<Option<Goal>>(data["goal"].clone()) {
                    let expected = match (&self.awaiting_goal, &goal) {
                        (Some(GoalChange::Set(expected)), Some(goal)) => expected == goal,
                        (Some(GoalChange::Clear), None) => true,
                        _ => false,
                    };
                    if root && expected {
                        self.awaiting_goal = None;
                    }
                    out.extend(self.goal_changed(session, goal));
                }
            }
            "session.renamed" if root => {
                if let Some(title) = data["title"].as_str()
                    && !title.is_empty()
                    && self.title.as_deref() != Some(title)
                {
                    self.title = Some(title.to_owned());
                    out.push(EngineEvent::Title {
                        title: title.to_owned(),
                    });
                }
            }
            "session.inbox.enqueued" => {
                if data["item"]["type"] == "synthetic" {
                    self.shell_notification(&data["item"]["payload"], &mut out);
                }
                if root
                    && let Some(message) = data["inboxID"].as_str()
                    && matches!(data["item"]["type"].as_str(), Some("user" | "synthetic"))
                {
                    self.admitted.insert(message.to_owned());
                }
            }
            "session.inbox.delivered" if root => {
                if let Some(message) = data["inboxID"].as_str() {
                    self.delivered.insert(message.to_owned());
                    if let Some(id) = self.steers.remove(message) {
                        out.push(EngineEvent::InputAccepted { id });
                    }
                }
            }
            "session.execution.started" if root => {
                self.turn_open = true;
                self.step_error = None;
            }
            "session.execution.succeeded" | "session.execution.interrupted" if root => {
                self.turn_open = false;
            }
            "session.execution.failed" => {
                let message = error_message(&data["error"]);
                if self.step_error.take().as_ref() != Some(&message) {
                    out.push(error_item(event, parent, ErrorKind::Api, &message, false));
                }
                if root {
                    self.turn_open = false;
                    self.error = Some(message);
                }
            }
            "session.step.started" => {
                if root
                    && let Some(model) = model_id(&data["model"])
                    && self.model.as_ref() != Some(&model)
                {
                    self.model = Some(model.clone());
                    out.push(EngineEvent::Model { id: model });
                }
                self.step_texts.remove(session);
            }
            "session.text.started" => {
                let Some(key) = text_key(data) else {
                    return out;
                };
                self.open_texts.insert(key.clone());
                out.push(EngineEvent::Item {
                    key,
                    parent,
                    status: Status::Running,
                    kind: ItemKind::AssistantMessage {
                        text: (String::new()).into(),
                        phase: Phase::Commentary,
                    },
                });
            }
            "session.text.delta" => {
                if let Some(text) = data["delta"].as_str()
                    && let Some(key) = text_key(data)
                {
                    out.push(EngineEvent::Live {
                        target: LiveKind::Text { key },
                        text: text.to_owned(),
                        append: true,
                    });
                }
            }
            "session.text.ended" => {
                let Some(text) = data["text"].as_str() else {
                    let message = "OpenCode completed a text without its required final body";
                    out.push(error_item(event, parent, ErrorKind::Api, message, false));
                    if root {
                        self.error = Some(message.to_owned());
                    }
                    return out;
                };
                let Some(key) = text_key(data) else {
                    return out;
                };
                self.open_texts.remove(&key);
                let text = text.to_owned();
                self.step_texts
                    .entry(session.to_owned())
                    .or_default()
                    .push((key.clone(), text.clone()));
                out.push(EngineEvent::Live {
                    target: LiveKind::Text { key },
                    text,
                    append: false,
                });
            }
            "session.reasoning.started" if root => {
                out.push(EngineEvent::Live {
                    target: LiveKind::Reasoning,
                    text: String::new(),
                    append: false,
                });
            }
            "session.reasoning.delta" if root => {
                if let Some(text) = data["delta"].as_str() {
                    out.push(EngineEvent::Live {
                        target: LiveKind::Reasoning,
                        text: text.to_owned(),
                        append: true,
                    });
                }
            }
            "session.tool.input.started" => {
                if let (Some(call), Some(name)) = (data["id"].as_str(), data["name"].as_str()) {
                    self.tool_names.insert(call.to_owned(), name.to_owned());
                }
            }
            "session.tool.called" => self.tool_called(session, data, &mut out),
            "session.tool.progress" => self.tool_progress(data),
            "session.tool.success" => self.tool_settled(data, true, &mut out),
            "session.tool.failed" => self.tool_settled(data, false, &mut out),
            "session.step.ended" => {
                self.step_ended(session, parent.as_deref(), data, &mut out);
            }
            "session.step.failed" => {
                self.step_texts.remove(session);
                // An interrupt fails the step; the user already knows.
                if data["error"]["type"] != "aborted" {
                    let message = error_message(&data["error"]);
                    out.push(error_item(event, parent, ErrorKind::Api, &message, false));
                    self.step_error = Some(message);
                }
            }
            "session.retry.scheduled" if root => {
                let attempt = data["attempt"].as_u64().unwrap_or_default();
                out.push(EngineEvent::Live {
                    target: LiveKind::Status,
                    text: format!(
                        "Retrying (attempt {attempt}): {}",
                        error_message(&data["error"])
                    ),
                    append: false,
                });
            }
            "session.compaction.started"
            | "session.compaction.ended"
            | "session.compaction.failed" => {
                let count = self.compactions.entry(session.to_owned()).or_default();
                let status = match kind {
                    "session.compaction.started" => {
                        *count += 1;
                        Status::Running
                    }
                    "session.compaction.ended" => Status::Done,
                    _ => Status::Failed,
                };
                out.push(EngineEvent::Item {
                    key: format!("compaction:{session}:{count}"),
                    parent,
                    status,
                    kind: ItemKind::Compaction {
                        summary: None,
                        tokens_before: None,
                        tokens_after: None,
                        switch: None,
                    },
                });
            }
            "permission.asked" => self.permission_asked(session, data, &mut out),
            "permission.replied" => self.permission_replied(data, &mut out),
            _ => tracing::debug!(kind, "ignored unsupported OpenCode event"),
        }
        out
    }

    /// Answers the approval or question on item `key`: the events to emit
    /// now and the request that tells OpenCode.
    pub fn answer(&mut self, key: &str, answer: &Answer) -> Option<(Reply, Vec<EngineEvent>)> {
        let (reply, events) = self.answer_raw(key, answer)?;
        Some((reply, crate::retained::settle(self, events)))
    }

    fn answer_raw(&mut self, key: &str, answer: &Answer) -> Option<(Reply, Vec<EngineEvent>)> {
        let native = if let Answer::Question { answers } = answer {
            let tool = self.tools.get(key)?;
            let ItemKind::Question { questions, .. } = &tool.kind else {
                return None;
            };
            match crate::question::native_collection_answers(questions, answers) {
                Ok(native) => native,
                Err(message) => {
                    tracing::warn!(key, message, "invalid answer; request remains pending");
                    return None;
                }
            }
        } else {
            Vec::new()
        };
        let pending = self.pending.remove(key)?;
        let mut out = Vec::new();
        let reply = match (pending, answer) {
            (Pending::Permission { session, request }, Answer::Approval { decision }) => {
                let reply = match decision {
                    Decision::AllowOnce => "once",
                    Decision::AllowAlways => "always",
                    Decision::DenyOnce | Decision::DenyAlways | Decision::Cancelled => "reject",
                };
                if let Some(tool) = self.tools.get_mut(key) {
                    if let ItemKind::Tool {
                        approval: Some(approval),
                        ..
                    } = &mut tool.kind
                    {
                        approval.decision = Some(*decision);
                    }
                    tool.status = if decision.allows() {
                        Status::Running
                    } else {
                        Status::Declined
                    };
                    out.push(self.tool_event(key));
                }
                Reply {
                    method: Method::Post,
                    path: format!("/api/session/{session}/permission/{request}/reply"),
                    body: Some(json!({ "decision": reply })),
                }
            }
            (
                Pending::Form {
                    session,
                    form,
                    fields,
                },
                Answer::Question { answers },
            ) => {
                let skipped = answers.iter().all(Option::is_none);
                if let Some(tool) = self.tools.get_mut(key) {
                    if let ItemKind::Question {
                        answers: stored, ..
                    } = &mut tool.kind
                    {
                        *stored = Some(answers.clone().into());
                    }
                    tool.status = if skipped {
                        Status::Declined
                    } else {
                        Status::Done
                    };
                    out.push(self.tool_event(key));
                }
                if skipped {
                    Reply {
                        method: Method::Delete,
                        path: format!("/api/session/{session}/form/{form}"),
                        body: None,
                    }
                } else {
                    let answer: Map<String, Value> = fields
                        .iter()
                        .zip(&native)
                        .filter_map(|(field, answer)| {
                            Some((field.key.clone(), field.value(answer.as_ref()?)))
                        })
                        .collect();
                    Reply {
                        method: Method::Post,
                        path: format!("/api/session/{session}/form/{form}/reply"),
                        body: Some(json!({ "answer": answer })),
                    }
                }
            }
            (pending, _) => {
                tracing::warn!(key, "answer does not match what OpenCode asked");
                self.pending.insert(key.to_owned(), pending);
                return None;
            }
        };
        Some((reply, out))
    }

    /// A child session of a followed one: followed from now on, under the
    /// `subagent` call that claims it with its progress.
    fn session_created(&mut self, data: &Value) {
        let (Some(session), Some(parent)) = (data["sessionID"].as_str(), data["parentID"].as_str())
        else {
            return;
        };
        if self.sessions.contains_key(session) || !self.sessions.contains_key(parent) {
            return;
        }
        self.sessions.insert(session.to_owned(), None);
        self.unbound.insert(session.to_owned(), data.clone());
    }

    /// A child no call claimed before its first step still gets an Agent
    /// item. Arrival order never ties a child to a call.
    fn bind_orphan(&mut self, session: &str, out: &mut Vec<EngineEvent>) {
        let Some(info) = self.unbound.remove(session) else {
            return;
        };
        let parent = info["parentID"]
            .as_str()
            .and_then(|parent| self.sessions.get(parent).cloned().flatten());
        let key = format!("agent:{session}");
        out.push(EngineEvent::Item {
            key: key.clone(),
            parent,
            status: Status::Running,
            kind: ItemKind::Agent {
                engine: None,
                thread: None,
                name: str_at(&info, "agent"),
                description: (str_at(&info, "title").unwrap_or_default()).into(),
                prompt: None,
                agent_id: None,
                context_used: None,
                model: None,
                background: false,
                result: None,
                label: None,
            },
        });
        self.sessions.insert(session.to_owned(), Some(key));
    }

    fn tool_called(&mut self, session: &str, data: &Value, out: &mut Vec<EngineEvent>) {
        let Some(call) = data["id"].as_str() else {
            return;
        };
        let Some(name) = self.tool_names.get(call).cloned() else {
            tracing::warn!(call, "OpenCode called a tool it never named");
            return;
        };
        let input = &data["input"];
        // The question arrives as `form.created`.
        if name == "question" {
            return;
        }
        let kind = if name == "subagent" {
            ItemKind::Agent {
                engine: None,
                thread: None,
                name: str_at(input, "agent"),
                description: (str_at(input, "description").unwrap_or_default()).into(),
                prompt: (str_at(input, "prompt")).map(Into::into),
                agent_id: None,
                context_used: None,
                model: None,
                background: input["background"].as_bool() == Some(true),
                result: None,
                label: None,
            }
        } else {
            let approval = match self.tools.get(call).map(|tool| &tool.kind) {
                Some(ItemKind::Tool { approval, .. }) => approval.clone(),
                _ => None,
            };
            ItemKind::Tool {
                tool: match tool_kind(&name, input) {
                    Ok(tool) => tool,
                    Err(message) => {
                        out.push(EngineEvent::Item {
                            key: format!("invalid-tool:{call}"),
                            parent: self.sessions.get(session).cloned().flatten(),
                            status: Status::Done,
                            kind: ItemKind::Error {
                                kind: ErrorKind::Execution,
                                message: message.into(),
                                retryable: false,
                            },
                        });
                        return;
                    }
                },
                title: str_at(input, "description"),
                output: None,
                exit_code: None,
                approval,
            }
        };
        let status = self
            .tools
            .get(call)
            .map_or(Status::Running, |tool| tool.status);
        self.tools.insert(
            call.to_owned(),
            Tool {
                session: session.to_owned(),
                status,
                kind,
            },
        );
        out.push(self.tool_event(call));
    }

    /// A `subagent` call names the child session it runs in its progress.
    fn tool_progress(&mut self, data: &Value) {
        let (Some(call), Some(child)) =
            (data["id"].as_str(), data["metadata"]["sessionID"].as_str())
        else {
            return;
        };
        let Some(tool) = self.tools.get(call) else {
            return;
        };
        if !matches!(tool.kind, ItemKind::Agent { .. }) {
            return;
        }
        self.unbound.remove(child);
        self.sessions
            .insert(child.to_owned(), Some(call.to_owned()));
    }

    fn tool_settled(&mut self, data: &Value, success: bool, out: &mut Vec<EngineEvent>) {
        let Some(call) = data["id"].as_str() else {
            return;
        };
        let cancelled = self.cancelled || data["error"]["type"] == "aborted";
        let Some(tool) = self.tools.get_mut(call) else {
            return;
        };
        let metadata = &data["metadata"];
        let text = if success {
            crate::native::content_text(&data["content"])
        } else {
            Some(error_message(&data["error"]))
        };
        match &mut tool.kind {
            ItemKind::Agent { result, .. } => {
                *result = text.map(Into::into);
            }
            ItemKind::Tool {
                tool: kind,
                output,
                exit_code,
                approval,
                ..
            } => {
                *output = text
                    .filter(|text| !text.is_empty())
                    .map(|text| Output::from_text(&text));
                *exit_code = metadata["exit"]
                    .as_i64()
                    .and_then(|code| i32::try_from(code).ok());
                if let ToolKind::Edit { changes } = kind
                    && let Err(message) = settle_changes(changes, metadata)
                {
                    tool.status = Status::Failed;
                    out.push(EngineEvent::Item {
                        key: format!("edit-error:{call}"),
                        parent: None,
                        status: Status::Done,
                        kind: ItemKind::Error {
                            kind: ErrorKind::Execution,
                            message: message.into(),
                            retryable: false,
                        },
                    });
                    out.push(self.tool_event(call));
                    return;
                }
                let denied = approval
                    .as_ref()
                    .and_then(|approval| approval.decision)
                    .is_some_and(|d| !d.allows());
                tool.status = match (success, denied, cancelled) {
                    (true, ..) => Status::Done,
                    (false, true, _) => Status::Declined,
                    (false, false, true) => Status::Cancelled,
                    (false, false, false) => Status::Failed,
                };
                // A background shell's call returns while it runs on; it
                // ends with its `shell.exited`.
                if success
                    && str_at(metadata, "status").as_deref() == Some("running")
                    && let ToolKind::Shell { background, .. } = kind
                    && let Some(shell) = str_at(metadata, "shellID")
                {
                    *background = true;
                    tool.status = Status::Running;
                    self.shells.insert(shell, call.to_owned());
                }
                out.push(self.tool_event(call));
                return;
            }
            // Settled when answered.
            ItemKind::Question { .. } => return,
            ItemKind::UserMessage { .. }
            | ItemKind::AssistantMessage { .. }
            | ItemKind::Plan { .. }
            | ItemKind::Image { .. }
            | ItemKind::Compaction { .. }
            | ItemKind::Notice { .. }
            | ItemKind::Error { .. } => {}
        }
        tool.status = match (success, cancelled) {
            (true, _) => Status::Done,
            (false, true) => Status::Cancelled,
            (false, false) => Status::Failed,
        };
        out.push(self.tool_event(call));
    }

    fn shell_exited(&mut self, data: &Value, out: &mut Vec<EngineEvent>) {
        let Some(call) = data["id"]
            .as_str()
            .and_then(|shell| self.shells.get(shell).cloned())
        else {
            return;
        };
        let Some(tool) = self.tools.get_mut(&call) else {
            return;
        };
        let exit = data["exit"]
            .as_i64()
            .and_then(|code| i32::try_from(code).ok());
        tool.status = match (data["status"].as_str(), exit) {
            (Some("exited"), Some(0) | None) => Status::Done,
            (Some("killed"), _) => Status::Cancelled,
            _ => Status::Failed,
        };
        if let ItemKind::Tool { exit_code, .. } = &mut tool.kind
            && exit.is_some()
        {
            *exit_code = exit;
        }
        out.push(self.tool_event(&call));
    }

    fn shell_notification(&mut self, payload: &Value, out: &mut Vec<EngineEvent>) {
        let metadata = &payload["metadata"];
        if metadata["source"] != "shell" {
            return;
        }
        let Some(call) = metadata["shellID"]
            .as_str()
            .and_then(|shell| self.shells.remove(shell))
        else {
            return;
        };
        let Some(tool) = self.tools.get_mut(&call) else {
            return;
        };
        let exit = metadata["exit"]
            .as_i64()
            .and_then(|code| i32::try_from(code).ok());
        tool.status = match metadata["state"].as_str() {
            Some("cancelled") => Status::Cancelled,
            Some("error") => Status::Failed,
            Some("completed")
                if metadata["timeout"] == true || exit.is_some_and(|code| code != 0) =>
            {
                Status::Failed
            }
            Some("completed") => Status::Done,
            _ => tool.status,
        };
        if let ItemKind::Tool {
            output, exit_code, ..
        } = &mut tool.kind
        {
            if let Some(text) = payload["text"].as_str() {
                *output = Some(Output::from_text(text));
            }
            if exit.is_some() {
                *exit_code = exit;
            }
        }
        out.push(self.tool_event(&call));
    }

    fn step_ended(
        &mut self,
        session: &str,
        parent: Option<&str>,
        data: &Value,
        out: &mut Vec<EngineEvent>,
    ) {
        let tokens = &data["tokens"];
        let count = |value: &Value| value.as_u64().unwrap_or_default();
        self.usage.input += count(&tokens["input"]);
        self.usage.output += count(&tokens["output"]);
        self.usage.reasoning += count(&tokens["reasoning"]);
        self.usage.cache_read += count(&tokens["cache"]["read"]);
        self.usage.cache_write =
            Some(self.usage.cache_write.unwrap_or_default() + count(&tokens["cache"]["write"]));
        if let Some(cost) = data["cost"].as_f64() {
            self.usage.cost_usd = Some(self.usage.cost_usd.unwrap_or_default() + cost);
        }
        if self.root.as_deref() == Some(session) {
            self.usage.context_used = Some(
                count(&tokens["input"])
                    + count(&tokens["cache"]["read"])
                    + count(&tokens["cache"]["write"]),
            );
        }
        out.push(EngineEvent::Usage(self.usage.clone()));
        let texts = self.step_texts.remove(session).unwrap_or_default();
        let final_step = data["finish"].as_str() != Some("tool-calls");
        let last = texts.len().saturating_sub(1);
        for (index, (key, text)) in texts.into_iter().enumerate() {
            out.push(EngineEvent::Item {
                key,
                parent: parent.map(str::to_owned),
                status: Status::Done,
                kind: ItemKind::AssistantMessage {
                    text: text.into(),
                    phase: if final_step && index == last {
                        Phase::Final
                    } else {
                        Phase::Commentary
                    },
                },
            });
        }
    }

    fn permission_asked(&mut self, session: &str, data: &Value, out: &mut Vec<EngineEvent>) {
        let Some(request) = data["id"].as_str() else {
            return;
        };
        let action = data["action"].as_str().unwrap_or_default();
        let resources: Vec<&str> = data["resources"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        let approval_request = match action {
            "shell" => ApprovalRequest::Command {
                command: (resources.join("\n")).into(),
                cwd: None,
            },
            "edit" | "write" => ApprovalRequest::FileChange,
            _ => ApprovalRequest::Action {
                description: (format!("{action} {}", resources.join(", "))
                    .trim()
                    .to_owned())
                .into(),
            },
        };
        let approval = Approval {
            request: approval_request,
            reason: None,
            options: vec![
                Decision::AllowOnce,
                Decision::AllowAlways,
                Decision::DenyOnce,
            ],
            decision: None,
        };
        // Approvals live on the tool they gate; one asked before its call
        // is announced creates the item from what the permission says.
        let key = data["source"]["id"].as_str().unwrap_or(request).to_owned();
        let tool = self.tools.entry(key.clone()).or_insert_with(|| Tool {
            session: session.to_owned(),
            status: Status::Running,
            kind: ItemKind::Tool {
                tool: tool_kind(
                    action,
                    &json!({ "command": resources.join("\n"), "path": resources.first() }),
                )
                .unwrap_or_else(|_| ToolKind::Other {
                    name: action.to_owned(),
                    input: Some(json!({"resources":resources}).to_string().into()),
                }),
                title: None,
                output: None,
                exit_code: None,
                approval: None,
            },
        });
        if let ItemKind::Tool { approval: slot, .. } = &mut tool.kind {
            *slot = Some(approval);
        }
        tool.status = Status::AwaitingUser;
        self.pending.insert(
            key.clone(),
            Pending::Permission {
                session: session.to_owned(),
                request: request.to_owned(),
            },
        );
        out.push(self.tool_event(&key));
    }

    /// A reply we didn't send (another client answered).
    fn permission_replied(&mut self, data: &Value, out: &mut Vec<EngineEvent>) {
        let Some(request) = data["requestID"].as_str() else {
            return;
        };
        let Some(key) = self
            .pending
            .iter()
            .find_map(|(key, pending)| match pending {
                Pending::Permission { request: asked, .. } if asked == request => Some(key.clone()),
                Pending::Permission { .. } | Pending::Form { .. } => None,
            })
        else {
            return;
        };
        let decision = match data["reply"].as_str() {
            Some("once") => Decision::AllowOnce,
            Some("always") => Decision::AllowAlways,
            _ => Decision::DenyOnce,
        };
        if let Some((_, events)) = self.answer(&key, &Answer::Approval { decision }) {
            out.extend(events);
        }
    }

    /// A form (the question tool asks through one) becomes a question per
    /// field.
    fn form_created(&mut self, event: &Value, form: &Value, out: &mut Vec<EngineEvent>) {
        let (Some(session), Some(id)) = (form["sessionID"].as_str(), form["id"].as_str()) else {
            return;
        };
        let Some(parent) = self.sessions.get(session).cloned() else {
            return;
        };
        let key = form["metadata"]["tool"]["id"]
            .as_str()
            .unwrap_or(id)
            .to_owned();
        let native = <Vec<NativeField> as serde::Deserialize>::deserialize(&form["fields"]);
        let asked = native.ok().and_then(|fields| {
            let asked: Option<Vec<_>> = fields
                .into_iter()
                .filter(|field| !field.hidden)
                .map(form_question)
                .collect();
            asked.filter(|asked| !asked.is_empty())
        });
        let Some(asked) = asked else {
            let message = "OpenCode asked a form Artisan can't show";
            tracing::warn!(session, form = id, message);
            out.push(error_item(
                event,
                parent,
                ErrorKind::Execution,
                message,
                false,
            ));
            if self.root.as_deref() == Some(session) {
                self.error = Some(message.to_owned());
                self.turn_open = false;
            }
            return;
        };
        let (questions, fields): (Vec<Question>, Vec<FormField>) = asked.into_iter().unzip();
        self.tools.insert(
            key.clone(),
            Tool {
                session: session.to_owned(),
                status: Status::AwaitingUser,
                kind: ItemKind::Question {
                    blocking: true,
                    questions: questions.into(),
                    answers: None,
                    answer_text: None,
                },
            },
        );
        self.pending.insert(
            key.clone(),
            Pending::Form {
                session: session.to_owned(),
                form: id.to_owned(),
                fields,
            },
        );
        out.push(self.tool_event(&key));
    }

    /// Settles a form another client answered or dismissed.
    fn form_settled(&mut self, replied: bool, data: &Value, out: &mut Vec<EngineEvent>) {
        let Some(form) = data["id"].as_str() else {
            return;
        };
        let Some((key, keys)) = self
            .pending
            .iter()
            .find_map(|(key, pending)| match pending {
                Pending::Form {
                    form: asked,
                    fields,
                    ..
                } if asked == form => Some((
                    key.clone(),
                    fields
                        .iter()
                        .map(|field| field.key.clone())
                        .collect::<Vec<_>>(),
                )),
                Pending::Permission { .. } | Pending::Form { .. } => None,
            })
        else {
            return;
        };
        let answers = keys
            .iter()
            .map(|field| {
                let value = replied
                    .then(|| &data["answer"][field])
                    .filter(|value| !value.is_null())?;
                let text = match value {
                    Value::String(text) => text.clone(),
                    Value::Array(items) => items
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(", "),
                    other
                    @ (Value::Null | Value::Bool(_) | Value::Number(_) | Value::Object(_)) => {
                        other.to_string()
                    }
                };
                Some(artisan_api::QuestionAnswer::Text { text })
            })
            .collect();
        if let Some((_, events)) = self.answer(&key, &Answer::Question { answers }) {
            out.extend(events);
        }
    }

    fn tool_event(&self, key: &str) -> EngineEvent {
        let tool = &self.tools[key];
        EngineEvent::Item {
            key: key.to_owned(),
            parent: self.sessions.get(&tool.session).cloned().flatten(),
            status: tool.status,
            kind: tool.kind.clone(),
        }
    }
}

impl crate::retained::Retains for Decoder {
    fn retained(&mut self, key: &str) -> Option<&mut ItemKind> {
        self.tools.get_mut(key).map(|tool| &mut tool.kind)
    }
}

/// One form field as a question; `None` for a field Artisan can't ask.
fn form_question(field: NativeField) -> Option<(Question, FormField)> {
    let text = field
        .description
        .clone()
        .or_else(|| field.title.clone())
        .unwrap_or_else(|| field.key.clone());
    let header = field.title.filter(|title| *title != text);
    let (kind, options) = match field.kind.as_str() {
        "string" if field.options.is_empty() => (FieldKind::Text, Vec::new()),
        "string" => (FieldKind::Choice, field.options),
        "multiselect" => (FieldKind::Choices, field.options),
        "boolean" => (
            FieldKind::Boolean,
            ["Yes", "No"]
                .map(|label| NativeOption {
                    value: label.to_owned(),
                    label: label.to_owned(),
                    description: None,
                })
                .into(),
        ),
        "number" | "integer" => (FieldKind::Number, Vec::new()),
        _ => return None,
    };
    if text.trim().is_empty() || options.iter().any(|option| option.label.trim().is_empty()) {
        return None;
    }
    let question = Question {
        id: field.key.clone(),
        header,
        text: text.into(),
        multi_select: field.kind == "multiselect",
        options: options
            .iter()
            .enumerate()
            .map(|(index, option)| QuestionOption {
                id: format!("option-{index}"),
                label: option.label.clone(),
                description: option.description.clone().map(Into::into),
            })
            .collect::<Vec<_>>()
            .into(),
    };
    let values = options
        .into_iter()
        .map(|option| (option.label, option.value))
        .collect();
    Some((
        question,
        FormField {
            key: field.key,
            kind,
            values,
        },
    ))
}

impl FormField {
    /// The value OpenCode wants for the chosen labels or typed text.
    fn value(&self, answer: &[String]) -> Value {
        let value = |label: &String| self.values.get(label).unwrap_or(label).clone();
        match self.kind {
            FieldKind::Choices => Value::from(answer.iter().map(value).collect::<Vec<_>>()),
            FieldKind::Boolean => Value::from(answer.first().is_some_and(|label| label == "Yes")),
            FieldKind::Number => {
                let text = answer.first().map(String::as_str).unwrap_or_default();
                text.trim()
                    .parse::<f64>()
                    .map_or_else(|_| Value::from(text), Value::from)
            }
            FieldKind::Text | FieldKind::Choice => {
                Value::from(answer.first().map(value).unwrap_or_default())
            }
        }
    }
}

fn text_key(data: &Value) -> Option<String> {
    let message = data["assistantMessageID"]
        .as_str()
        .filter(|message| !message.is_empty());
    if let (Some(message), Some(ordinal)) = (message, data["ordinal"].as_u64()) {
        Some(format!("{message}/{ordinal}"))
    } else {
        tracing::warn!("OpenCode text event omitted its message/ordinal identity");
        None
    }
}

fn model_id(model: &Value) -> Option<String> {
    Some(format!(
        "{}/{}",
        model["providerID"].as_str()?,
        model["id"].as_str()?
    ))
}

fn str_at(value: &Value, key: &str) -> Option<String> {
    value[key]
        .as_str()
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

fn error_message(error: &Value) -> String {
    error["message"]
        .as_str()
        .or_else(|| error.as_str())
        .or_else(|| error["type"].as_str())
        .unwrap_or("OpenCode reported an error")
        .to_owned()
}

#[expect(
    clippy::expect_used,
    reason = "callers pass events whose identity was validated on arrival"
)]
fn error_item(
    event: &Value,
    parent: Option<String>,
    kind: ErrorKind,
    message: &str,
    retryable: bool,
) -> EngineEvent {
    EngineEvent::Item {
        key: format!(
            "error:{}",
            event["id"]
                .as_str()
                .expect("validated native event identity")
        ),
        parent,
        status: Status::Done,
        kind: ItemKind::Error {
            kind,
            message: (message.to_owned()).into(),
            retryable,
        },
    }
}

#[expect(
    clippy::expect_used,
    reason = "the required fields are checked at the top before any is read"
)]
fn tool_kind(name: &str, input: &Value) -> Result<ToolKind, String> {
    let required = |field: &str| {
        input[field]
            .as_str()
            .is_some_and(|value| !value.trim().is_empty())
    };
    let valid = match name {
        "shell" => required("command"),
        "read" | "write" | "edit" => required("path"),
        "grep" | "glob" => required("pattern"),
        "webfetch" => required("url"),
        "websearch" => required("query"),
        _ => !name.trim().is_empty(),
    };
    if !valid {
        return Err(format!(
            "OpenCode {name} omitted required native tool fields"
        ));
    }
    let text = |key: &str| str_at(input, key).filter(|value| !value.trim().is_empty());
    let path = || text("path").expect("validated path");
    Ok(match name {
        "shell" => ToolKind::Shell {
            command: (text("command").expect("validated command")).into(),
            cwd: text("workdir"),
            background: input["background"].as_bool() == Some(true),
            watch: false,
        },
        "read" => ToolKind::Read {
            path: path(),
            range: match (input["offset"].as_u64(), input["limit"].as_u64()) {
                (None, None) => None,
                (offset, limit) => {
                    let start = offset.unwrap_or(1);
                    Some((
                        start,
                        limit.map_or(u64::MAX, |limit| {
                            start.saturating_add(limit.saturating_sub(1))
                        }),
                    ))
                }
            },
        },
        "write" => ToolKind::Edit {
            changes: (vec![FileChange {
                path: path(),
                op: FileOp::Add,
                diff: None,
            }])
            .into(),
        },
        "edit" => ToolKind::Edit {
            changes: (vec![FileChange {
                path: path(),
                op: FileOp::Modify,
                diff: None,
            }])
            .into(),
        },
        "grep" => ToolKind::Search {
            pattern: text("pattern").expect("validated pattern"),
            path: text("path"),
            mode: SearchMode::Grep,
        },
        "glob" => ToolKind::Search {
            pattern: text("pattern").expect("validated pattern"),
            path: text("path"),
            mode: SearchMode::Glob,
        },
        "webfetch" => ToolKind::WebFetch {
            url: text("url").expect("validated URL"),
        },
        "websearch" => ToolKind::WebSearch {
            query: text("query").expect("validated query"),
        },
        _ => {
            let input = (!input.is_null()).then(|| input.to_string());
            match name.split_once('_') {
                Some((server, tool)) if !BUILTIN_TOOLS.contains(&name) => ToolKind::Mcp {
                    server: server.to_owned(),
                    tool: tool.to_owned(),
                    input: input.map(Into::into),
                },
                _ => ToolKind::Other {
                    name: name.to_owned(),
                    input: input.map(Into::into),
                },
            }
        }
    })
}

/// Edit results list the files with their diffs; `write` says whether the
/// file existed.
fn settle_changes(changes: &mut Collection<FileChange>, metadata: &Value) -> Result<(), String> {
    if let Some(files) = metadata["files"].as_array() {
        *changes = files
            .iter()
            .map(|file| FileChange {
                path: str_at(file, "file")
                    .or_else(|| str_at(file, "path"))
                    .unwrap_or_default(),
                op: match file["status"].as_str() {
                    Some("added") => FileOp::Add,
                    Some("deleted") => FileOp::Delete,
                    _ => FileOp::Modify,
                },
                diff: (str_at(file, "patch")).map(Into::into),
            })
            .collect::<Vec<_>>()
            .into();
    } else if metadata["existed"].as_bool() == Some(true) {
        let records = changes
            .inline_mut()
            .ok_or("late native edit correction requires explicit file records")?;
        for change in records {
            change.op = FileOp::Modify;
        }
    }
    Ok(())
}
