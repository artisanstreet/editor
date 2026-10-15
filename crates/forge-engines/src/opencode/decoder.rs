//! Pure decoding of OpenCode's v2 event stream (`GET /api/event`).
//!
//! The global stream carries every session on the server, so the decoder
//! follows the run's root session plus any child session whose parent it
//! already follows; everything else is ignored. Shapes were captured from
//! OpenCode 1.18.32 (see `transcripts/opencode`).

use crate::{EngineEvent, LiveKind};
use anyhow::Context as _;
use artisan_api::{
    Answer, Approval, ApprovalRequest, Collection, Decision, ErrorKind, FileChange, FileOp, ItemId,
    ItemKind, Output, Phase, PlanStep, Question, QuestionOption, SearchMode, Status, StepStatus,
    ToolKind, Usage,
};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};

#[derive(serde::Deserialize)]
struct NativeError<'a> {
    name: &'a str,
}

#[derive(serde::Deserialize)]
struct NativeSessionStatus<'a> {
    #[serde(borrow)]
    status: NativeStatus<'a>,
}
#[derive(serde::Deserialize)]
struct NativeStatus<'a> {
    #[serde(rename = "type")]
    kind: &'a str,
}

#[derive(serde::Deserialize)]
struct NativeQuestion {
    question: String,
    #[serde(default)]
    header: Option<String>,
    #[serde(default)]
    multiple: bool,
    options: Vec<NativeQuestionOption>,
}
#[derive(serde::Deserialize)]
struct NativeQuestionOption {
    label: String,
    #[serde(default)]
    description: Option<String>,
}

/// Tool names OpenCode ships; any other name with an underscore is an MCP
/// tool (`<server>_<tool>`).
const BUILTIN_TOOLS: &[&str] = &[
    "apply_patch",
    "bash",
    "codesearch",
    "edit",
    "glob",
    "grep",
    "invalid",
    "list",
    "lsp",
    "ls",
    "multiedit",
    "patch",
    "plan_enter",
    "plan_exit",
    "question",
    "read",
    "skill",
    "task",
    "todoread",
    "todowrite",
    "webfetch",
    "websearch",
    "write",
];

/// An HTTP request the driver must send to answer OpenCode.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Reply {
    pub path: String,
    pub body: Option<Value>,
}

#[derive(Default)]
pub(crate) struct Decoder {
    ownership: crate::retained::Ownership,
    reasoning_summary: crate::reasoning::Summary,
    root: Option<String>,
    /// Followed sessions → the `Agent` item their items belong to.
    sessions: HashMap<String, Option<String>>,
    /// By OpenCode call id.
    tools: HashMap<String, Tool>,
    /// Texts ended in the current step, per session, for the phase decision.
    step_texts: HashMap<String, Vec<(String, String)>>,
    open_texts: HashSet<String>,
    /// OpenCode message id → our user message, for steers.
    steers: HashMap<String, ItemId>,
    admitted: HashSet<String>,
    prompted: HashSet<String>,
    turn_open: bool,
    pending: HashMap<String, Pending>,
    usage: Usage,
    model: Option<String>,
    title: Option<String>,
    /// The last error that ended a turn.
    pub error: Option<String>,
    recoverable_error: Option<String>,
    cancelled: bool,
}

struct Tool {
    session: String,
    status: Status,
    kind: ItemKind,
    /// Set for `task` calls whose child session isn't known yet.
    awaiting_child: bool,
}

#[derive(serde::Serialize)]
enum Pending {
    Permission { session: String, request: String },
    Question { session: String, request: String },
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
    pub fn has_root(&self) -> bool {
        self.root.is_some()
    }

    pub fn follow_root(&mut self, session: String) {
        self.sessions.insert(session.clone(), None);
        self.root = Some(session);
    }

    /// The driver sent a prompt OpenCode admitted as `message`; `steer`
    /// names our user message when it was a steer.
    pub fn prompt_sent(&mut self, message: &str, steer: Option<ItemId>) -> Vec<EngineEvent> {
        self.admitted.insert(message.to_owned());
        match steer {
            Some(id) if self.prompted.contains(message) => vec![EngineEvent::InputAccepted { id }],
            Some(id) => {
                self.steers.insert(message.to_owned(), id);
                Vec::new()
            }
            None => Vec::new(),
        }
    }

    /// The engine has nothing left to do for this run: the last turn ended
    /// and every prompt we sent has started.
    pub fn idle(&self) -> bool {
        !self.turn_open
            && self
                .admitted
                .iter()
                .all(|message| self.prompted.contains(message))
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
        let event: Value = serde_json::from_str(data).context("decoding OpenCode protocol JSON")?;
        anyhow::ensure!(
            event.is_object(),
            "OpenCode protocol frame must be an object"
        );
        anyhow::ensure!(
            event["type"].is_string(),
            "native protocol frame is missing its required envelope identity"
        );
        #[derive(serde::Deserialize)]
        struct Identity<'a> {
            id: &'a str,
        }
        let identity = <Identity as serde::Deserialize>::deserialize(&event)
            .context("decoding OpenCode event identity")?;
        anyhow::ensure!(!identity.id.trim().is_empty(), "OpenCode event ID is empty");
        let pending_frame = matches!(
            event["type"].as_str(),
            Some("permission.v2.asked" | "question.v2.asked")
        );
        if pending_frame {
            crate::retained::admit_pending(self.pending.len(), self.ownership.bytes(), &event)?;
        }
        let pending_before = self.pending.len();
        let events = self.feed_raw(&event);
        let events = self.reasoning_summary.normalize(events, false);
        for key in crate::retained::changed_keys(&events) {
            if let Some(tool) = self.tools.get_mut(key) {
                if tool.status.is_terminal() {
                    crate::retained::compact_terminal(&mut tool.kind)?;
                }
                self.ownership
                    .replace(key, crate::retained::owned_charge(&tool.kind)?);
            } else {
                self.ownership.remove(key);
            }
        }
        if pending_frame || self.pending.len() != pending_before {
            self.ownership
                .set_pending(crate::retained::owned_charge(&self.pending)?);
        }
        self.ownership.check()?;
        Ok(events)
    }

    fn feed_raw(&mut self, event: &Value) -> Vec<EngineEvent> {
        let mut out = Vec::new();
        let kind = event["type"].as_str().unwrap_or_default();
        let data = &event["data"];
        // Child sessions announce themselves before any of their events.
        if matches!(kind, "session.created" | "session.updated") {
            self.session_info(data, &mut out);
            return out;
        }
        let Some(session) = data["sessionID"].as_str() else {
            return out;
        };
        let Some(parent) = self.sessions.get(session).cloned() else {
            return out;
        };
        let root = self.root.as_deref() == Some(session);
        match kind {
            "session.next.prompt.admitted" if root => {
                if let Some(message) = data["messageID"].as_str() {
                    self.admitted.insert(message.to_owned());
                }
            }
            "session.next.prompted" if root => {
                if let Some(message) = data["messageID"].as_str() {
                    self.prompted.insert(message.to_owned());
                    if let Some(id) = self.steers.remove(message) {
                        out.push(EngineEvent::InputAccepted { id });
                    }
                }
                self.turn_open = true;
            }
            "session.next.step.started" => {
                if root {
                    self.turn_open = true;
                    self.recoverable_error = None;
                    if let Some(model) = model_id(&data["model"])
                        && self.model.as_ref() != Some(&model)
                    {
                        self.model = Some(model.clone());
                        out.push(EngineEvent::Model { id: model });
                    }
                }
                self.step_texts.remove(session);
                self.ownership.remove_buffer(session);
            }
            "session.next.text.started" => {
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
            "session.next.text.delta" => {
                if let Some(text) = data["delta"].as_str() {
                    out.push(EngineEvent::Live {
                        target: LiveKind::Text {
                            key: {
                                let Some(key) = text_key(data) else {
                                    return out;
                                };
                                key
                            },
                        },
                        text: text.to_owned(),
                        append: true,
                    });
                }
            }
            "session.next.text.ended" => {
                let Some(text) = data["text"].as_str() else {
                    let message = "OpenCode completed a text without its required final body";
                    out.push(error_item(&event, parent, ErrorKind::Api, message, false));
                    if root {
                        self.error = Some(message.to_owned());
                        self.turn_open = false;
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
                self.ownership.append_buffer(
                    session,
                    key.len()
                        .saturating_add(text.len())
                        .saturating_add(128)
                        .saturating_mul(4),
                );
                out.push(EngineEvent::Live {
                    target: LiveKind::Text { key },
                    text,
                    append: false,
                });
            }
            "session.next.reasoning.started" if root => {
                out.push(EngineEvent::Live {
                    target: LiveKind::Reasoning,
                    text: String::new(),
                    append: false,
                });
            }
            "session.next.reasoning.delta" if root => {
                if let Some(text) = data["delta"].as_str() {
                    out.push(EngineEvent::Live {
                        target: LiveKind::Reasoning,
                        text: text.to_owned(),
                        append: true,
                    });
                }
            }
            "session.next.tool.called" => self.tool_called(session, data, &mut out),
            "session.next.tool.progress" => self.tool_progress(data, &mut out),
            "session.next.tool.success" => self.tool_settled(data, true, &mut out),
            "session.next.tool.failed" => self.tool_settled(data, false, &mut out),
            "session.next.step.ended" => {
                self.step_ended(session, parent, data, &mut out);
            }
            "session.next.step.failed" => {
                let message = error_message(&data["error"]);
                self.step_texts.remove(session);
                self.ownership.remove_buffer(session);
                out.push(error_item(&event, parent, ErrorKind::Api, &message, false));
                if root {
                    self.turn_open = false;
                    self.error = Some(message);
                }
            }
            "session.error" => {
                let message = error_message(&data["error"]);
                let recovering = matches!(
                    <NativeError as serde::Deserialize>::deserialize(&data["error"]),
                    Ok(NativeError {
                        name: "ContextOverflowError"
                    })
                );
                out.push(error_item(
                    &event,
                    parent,
                    ErrorKind::Api,
                    &message,
                    recovering,
                ));
                // v1.18.32 processor.ts halt publishes other session errors
                // before setting idle; overflow instead requests compaction.
                if root {
                    if recovering {
                        self.recoverable_error = Some(message);
                    } else {
                        self.error = Some(message);
                        self.turn_open = false;
                    }
                }
            }
            "session.status" | "session.idle" if root => {
                let idle = kind == "session.idle"
                    || matches!(
                        <NativeSessionStatus as serde::Deserialize>::deserialize(data),
                        Ok(NativeSessionStatus {
                            status: NativeStatus { kind: "idle" }
                        })
                    );
                if idle
                    && self.turn_open
                    && let Some(message) = self.recoverable_error.take()
                {
                    self.error = Some(message);
                    self.turn_open = false;
                }
            }
            "session.next.retried" if root => {
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
            "session.next.compaction.started" | "session.next.compaction.ended" => {
                let Some(message) = data["messageID"]
                    .as_str()
                    .filter(|id| !id.trim().is_empty())
                else {
                    out.push(error_item(
                        event,
                        parent,
                        ErrorKind::Execution,
                        "OpenCode compaction omitted required messageID",
                        false,
                    ));
                    return out;
                };
                let key = format!("compaction:{message}");
                let status = if kind.ends_with("started") {
                    Status::Running
                } else {
                    Status::Done
                };
                out.push(EngineEvent::Item {
                    key,
                    parent,
                    status,
                    kind: ItemKind::Compaction {
                        tokens_before: None,
                        tokens_after: None,
                    },
                });
            }
            "todo.updated" if root => {
                let steps = match todo_steps(&data["todos"]) {
                    Ok(steps) => steps,
                    Err(message) => {
                        out.push(error_item(
                            event,
                            parent,
                            ErrorKind::Execution,
                            &message,
                            false,
                        ));
                        return out;
                    }
                };
                let status = if steps
                    .iter()
                    .all(|step| matches!(step.status, StepStatus::Done | StepStatus::Cancelled))
                {
                    Status::Done
                } else {
                    Status::Running
                };
                out.push(EngineEvent::Item {
                    key: "plan".into(),
                    parent: None,
                    status,
                    kind: ItemKind::Plan {
                        steps: steps.into(),
                        proposal: None,
                        approval: None,
                    },
                });
            }
            "permission.v2.asked" => self.permission_asked(session, data, &mut out),
            "permission.v2.replied" => self.permission_replied(data, &mut out),
            "question.v2.asked" => self.question_asked(session, data, &mut out),
            "question.v2.replied" | "question.v2.rejected" => {
                self.question_settled(kind == "question.v2.replied", data, &mut out);
            }
            _ => tracing::debug!(kind, "ignored unsupported OpenCode event"),
        }
        out
    }

    /// Answers the approval or question on item `key`: the events to emit
    /// now and the request that tells OpenCode.
    pub fn answer(&mut self, key: &str, answer: &Answer) -> Option<(Reply, Vec<EngineEvent>)> {
        let mut result = self.answer_raw(key, answer);
        if let Some(item) = self.tools.get_mut(key)
            && item.status.is_terminal()
            && let Err(error) = crate::retained::compact_terminal(&mut item.kind)
            && let Some((_, events)) = &mut result
        {
            events.push(crate::retained::error(key, error));
        }
        let refreshed = (|| -> anyhow::Result<()> {
            if let Some(item) = self.tools.get(key) {
                self.ownership
                    .replace(key, crate::retained::owned_charge(&item.kind)?);
            }
            self.ownership
                .set_pending(crate::retained::owned_charge(&self.pending)?);
            self.ownership.check()
        })();
        if let Err(error) = refreshed
            && let Some((_, events)) = &mut result
        {
            events.push(crate::retained::ownership_error(key, error));
        }

        result
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
                    path: format!("/api/session/{session}/permission/{request}/reply"),
                    body: Some(json!({ "reply": reply })),
                }
            }
            (Pending::Question { session, request }, Answer::Question { answers }) => {
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
                        path: format!("/api/session/{session}/question/{request}/reject"),
                        body: None,
                    }
                } else {
                    let answers: Vec<Vec<String>> = native
                        .iter()
                        .map(|answer| answer.clone().unwrap_or_default())
                        .collect();
                    Reply {
                        path: format!("/api/session/{session}/question/{request}/reply"),
                        body: Some(json!({ "answers": answers })),
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

    fn session_info(&mut self, data: &Value, out: &mut Vec<EngineEvent>) {
        let info = &data["info"];
        let Some(session) = data["sessionID"].as_str().or_else(|| info["id"].as_str()) else {
            return;
        };
        if self.root.as_deref() == Some(session) {
            if let Some(title) = info["title"].as_str()
                && !title.is_empty()
                && !title.starts_with("New session - ")
                && self.title.as_deref() != Some(title)
            {
                self.title = Some(title.to_owned());
                out.push(EngineEvent::Title {
                    title: title.to_owned(),
                });
            }
            return;
        }
        if self.sessions.contains_key(session) {
            return;
        }
        let Some(parent_session) = info["parentID"].as_str() else {
            return;
        };
        if !self.sessions.contains_key(parent_session) {
            return;
        }
        // Bind only an explicit call relation. Arrival order and lexical call
        // IDs do not identify the process that created a child session.
        let call = info["parentCallID"]
            .as_str()
            .or_else(|| data["callID"].as_str());
        let agent = if let Some((key, tool)) =
            call.and_then(|call| self.tools.get_mut(call).map(|tool| (call, tool)))
        {
            tool.awaiting_child = false;
            key.to_owned()
        } else {
            // A child we can't tie to a call still gets an Agent item.
            let key = format!("agent:{session}");
            out.push(EngineEvent::Item {
                key: key.clone(),
                parent: self.sessions[parent_session].clone(),
                status: Status::Running,
                kind: ItemKind::Agent {
                    name: info["agent"].as_str().map(str::to_owned),
                    description: (info["title"].as_str().unwrap_or_default().to_owned()).into(),
                    prompt: None,
                    agent_id: None,
                    context_used: None,
                    model: None,
                    background: false,
                    result: None,
                    label: None,
                },
            });
            key
        };
        self.sessions.insert(session.to_owned(), Some(agent));
    }

    fn tool_called(&mut self, session: &str, data: &Value, out: &mut Vec<EngineEvent>) {
        let (Some(call), Some(name)) = (data["callID"].as_str(), data["tool"].as_str()) else {
            return;
        };
        let input = &data["input"];
        match name {
            // The todo list arrives as `todo.updated`; the question as
            // `question.v2.asked`.
            "todowrite" | "todoread" | "question" => return,
            _ => {}
        }
        let kind = if name == "task" {
            ItemKind::Agent {
                name: str_at(input, "subagent_type"),
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
                tool: match tool_kind(name, input) {
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
                awaiting_child: name == "task",
            },
        );
        out.push(self.tool_event(call));
    }

    fn tool_progress(&mut self, data: &Value, out: &mut Vec<EngineEvent>) {
        let Some(call) = data["callID"].as_str() else {
            return;
        };
        let Some(tool) = self.tools.get(call) else {
            return;
        };
        let text = data["delta"]
            .as_str()
            .or_else(|| data["output"].as_str())
            .or_else(|| data["text"].as_str())
            .map(str::to_owned)
            .or_else(|| content_text(&data["content"]));
        if let Some(text) = text {
            let key = call.to_owned();
            let target = match &tool.kind {
                ItemKind::Tool {
                    tool: ToolKind::Shell { .. },
                    ..
                } => LiveKind::Shell { key },
                _ => LiveKind::Progress { key },
            };
            out.push(EngineEvent::Live {
                target,
                text,
                append: true,
            });
        }
    }

    fn tool_settled(&mut self, data: &Value, success: bool, out: &mut Vec<EngineEvent>) {
        let Some(call) = data["callID"].as_str() else {
            return;
        };
        let cancelled = self.cancelled;
        let Some(tool) = self.tools.get_mut(call) else {
            return;
        };
        let structured = &data["structured"];
        let text = if success {
            content_text(&data["content"]).or_else(|| str_at(structured, "content"))
        } else {
            Some(error_message(&data["error"]))
        };
        match &mut tool.kind {
            ItemKind::Agent { result, .. } => {
                *result = text.map(Into::into);
                tool.awaiting_child = false;
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
                *exit_code = structured["exit"]
                    .as_i64()
                    .and_then(|code| i32::try_from(code).ok());
                if let ToolKind::Edit { changes } = kind {
                    if let Err(message) = settle_changes(changes, structured) {
                        tool.status = Status::Failed;
                        out.push(EngineEvent::Item {
                            key: format!("edit-error:{call}"),
                            parent: None,
                            status: Status::Failed,
                            kind: ItemKind::Error {
                                kind: ErrorKind::Execution,
                                message: message.into(),
                                retryable: false,
                            },
                        });
                        out.push(self.tool_event(call));
                        return;
                    }
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
                out.push(self.tool_event(call));
                return;
            }
            // Settled when answered.
            ItemKind::Question { .. } => return,
            _ => {}
        }
        tool.status = match (success, cancelled) {
            (true, _) => Status::Done,
            (false, true) => Status::Cancelled,
            (false, false) => Status::Failed,
        };
        out.push(self.tool_event(call));
    }

    fn step_ended(
        &mut self,
        session: &str,
        parent: Option<String>,
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
        let root = self.root.as_deref() == Some(session);
        if root {
            self.usage.context_used = Some(
                count(&tokens["input"])
                    + count(&tokens["cache"]["read"])
                    + count(&tokens["cache"]["write"]),
            );
        }
        out.push(EngineEvent::Usage(self.usage.clone()));
        let texts = self.step_texts.remove(session).unwrap_or_default();
        self.ownership.remove_buffer(session);
        let final_step = data["finish"].as_str() != Some("tool-calls");
        let last = texts.len().saturating_sub(1);
        for (index, (key, text)) in texts.into_iter().enumerate() {
            out.push(EngineEvent::Item {
                key,
                parent: parent.clone(),
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
        if !final_step {
            return;
        }
        if root {
            self.turn_open = false;
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
            "bash" => ApprovalRequest::Command {
                command: (resources.join("\n")).into(),
                cwd: None,
            },
            "edit" => ApprovalRequest::FileChange,
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
        let key = data["source"]["callID"]
            .as_str()
            .unwrap_or(request)
            .to_owned();
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
                    input: (Some(json!({"resources":resources}).to_string())).map(Into::into),
                }),
                title: None,
                output: None,
                exit_code: None,
                approval: None,
            },
            awaiting_child: false,
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
                _ => None,
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

    fn question_asked(&mut self, session: &str, data: &Value, out: &mut Vec<EngineEvent>) {
        let Some(request) = data["id"].as_str() else {
            return;
        };
        let key = data["tool"]["callID"]
            .as_str()
            .unwrap_or(request)
            .to_owned();
        let parsed = <Vec<NativeQuestion> as serde::Deserialize>::deserialize(&data["questions"]);
        let native = match parsed {
            Ok(questions)
                if !questions.is_empty()
                    && questions.iter().all(|question| {
                        !question.question.trim().is_empty()
                            && question
                                .options
                                .iter()
                                .all(|option| !option.label.trim().is_empty())
                    }) =>
            {
                questions
            }
            _ => {
                let message = "OpenCode supplied malformed question prompts or options";
                tracing::warn!(request, message);
                out.push(EngineEvent::Item {
                    key: format!("invalid-question:{request}"),
                    parent: self.sessions.get(session).cloned().flatten(),
                    status: Status::Done,
                    kind: ItemKind::Error {
                        kind: ErrorKind::Execution,
                        message: (message.to_owned()).into(),
                        retryable: false,
                    },
                });
                self.error = Some(message.to_owned());
                self.turn_open = false;
                return;
            }
        };
        let questions = native
            .into_iter()
            .enumerate()
            .map(|(index, question)| Question {
                id: index.to_string(),
                header: question.header,
                text: (question.question).into(),
                multi_select: question.multiple,
                options: question
                    .options
                    .into_iter()
                    .enumerate()
                    .map(|(index, option)| QuestionOption {
                        id: format!("option-{index}"),
                        label: option.label,
                        description: (option.description).map(Into::into),
                    })
                    .collect::<Vec<_>>()
                    .into(),
            })
            .collect::<Vec<_>>();
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
                awaiting_child: false,
            },
        );
        self.pending.insert(
            key.clone(),
            Pending::Question {
                session: session.to_owned(),
                request: request.to_owned(),
            },
        );
        out.push(self.tool_event(&key));
    }

    /// Settles a question another client answered.
    fn question_settled(&mut self, replied: bool, data: &Value, out: &mut Vec<EngineEvent>) {
        let Some(request) = data["requestID"].as_str() else {
            return;
        };
        let Some(key) = self
            .pending
            .iter()
            .find_map(|(key, pending)| match pending {
                Pending::Question { request: asked, .. } if asked == request => Some(key.clone()),
                _ => None,
            })
        else {
            return;
        };
        let answers = if replied {
            data["answers"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|answer| {
                    Some(artisan_api::QuestionAnswer::Text {
                        text: answer
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(Value::as_str)
                            .collect::<Vec<_>>()
                            .join(", "),
                    })
                })
                .collect()
        } else {
            Vec::new()
        };
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

fn text_key(data: &Value) -> Option<String> {
    use serde::Deserialize;
    #[derive(Deserialize)]
    struct TextIdentity<'a> {
        #[serde(rename = "assistantMessageID")]
        message: &'a str,
        #[serde(rename = "textID")]
        text: &'a str,
    }
    match TextIdentity::deserialize(data) {
        Ok(identity) if !identity.message.is_empty() && !identity.text.is_empty() => {
            Some(format!("{}/{}", identity.message, identity.text))
        }
        _ => {
            tracing::warn!("OpenCode text event omitted its message/text identity");
            None
        }
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

fn content_text(content: &Value) -> Option<String> {
    let parts: Vec<&str> = content
        .as_array()?
        .iter()
        .filter(|part| part["type"].as_str() == Some("text"))
        .filter_map(|part| part["text"].as_str())
        .collect();
    (!parts.is_empty()).then(|| parts.join("\n"))
}

fn error_message(error: &Value) -> String {
    error["message"]
        .as_str()
        .or_else(|| error["data"]["message"].as_str())
        .or_else(|| error.as_str())
        .or_else(|| error["name"].as_str())
        .unwrap_or("OpenCode reported an error")
        .to_owned()
}

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
        status: Status::Failed,
        kind: ItemKind::Error {
            kind,
            message: (message.to_owned()).into(),
            retryable,
        },
    }
}

fn tool_kind(name: &str, input: &Value) -> Result<ToolKind, String> {
    let required = |field: &str| {
        input[field]
            .as_str()
            .is_some_and(|value| !value.trim().is_empty())
    };
    let valid = match name {
        "bash" => required("command"),
        "read" | "write" | "edit" | "multiedit" => required("path") || required("filePath"),
        "grep" | "glob" => required("pattern"),
        "webfetch" => required("url"),
        "websearch" | "codesearch" => required("query"),
        "apply_patch" | "patch" => !patch_changes(input).is_empty(),
        _ => !name.trim().is_empty(),
    };
    if !valid {
        return Err(format!(
            "OpenCode {name} omitted required native tool fields"
        ));
    }
    let text = |key: &str| str_at(input, key).filter(|value| !value.trim().is_empty());
    let path = || {
        text("path")
            .or_else(|| text("filePath"))
            .expect("validated path")
    };
    Ok(match name {
        "bash" => ToolKind::Shell {
            command: (text("command").expect("validated command")).into(),
            cwd: text("workdir"),
            background: false,
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
        "edit" | "multiedit" => ToolKind::Edit {
            changes: (vec![FileChange {
                path: path(),
                op: FileOp::Modify,
                diff: None,
            }])
            .into(),
        },
        "apply_patch" | "patch" => ToolKind::Edit {
            changes: (patch_changes(input)).into(),
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
        "list" | "ls" => ToolKind::Search {
            pattern: text("path").unwrap_or_else(|| ".".to_owned()),
            path: text("path"),
            mode: SearchMode::List,
        },
        "webfetch" => ToolKind::WebFetch {
            url: text("url").expect("validated URL"),
        },
        "websearch" | "codesearch" => ToolKind::WebSearch {
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

/// Files named in an `apply_patch` envelope (`*** Add File: path`, …).
fn patch_changes(input: &Value) -> Vec<FileChange> {
    let patch = input["patchText"]
        .as_str()
        .or_else(|| input["patch"].as_str())
        .unwrap_or_default();
    patch
        .lines()
        .filter_map(|line| {
            let line = line.strip_prefix("*** ")?;
            let (op, path) = if let Some(path) = line.strip_prefix("Add File: ") {
                (FileOp::Add, path)
            } else if let Some(path) = line.strip_prefix("Update File: ") {
                (FileOp::Modify, path)
            } else {
                (FileOp::Delete, line.strip_prefix("Delete File: ")?)
            };
            Some(FileChange {
                path: path.trim().to_owned(),
                op,
                diff: None,
            })
        })
        .collect()
}

/// Edit results list the files with their diffs; `write` says whether the
/// file existed.
fn settle_changes(changes: &mut Collection<FileChange>, structured: &Value) -> Result<(), String> {
    if let Some(files) = structured["files"].as_array() {
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
    } else if structured["existed"].as_bool() == Some(true) {
        let records = changes
            .inline_mut()
            .ok_or("late native edit correction requires explicit file records")?;
        for change in records {
            change.op = FileOp::Modify;
        }
    }
    Ok(())
}

#[derive(serde::Deserialize)]
struct NativeTodo {
    content: String,
    status: NativeTodoStatus,
}
#[derive(serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum NativeTodoStatus {
    Pending,
    InProgress,
    Completed,
    Cancelled,
}
fn todo_steps(todos: &Value) -> Result<Vec<PlanStep>, String> {
    let entries = <Vec<NativeTodo> as serde::Deserialize>::deserialize(todos)
        .map_err(|error| format!("malformed OpenCode todos: {error}"))?;
    if entries.iter().any(|entry| entry.content.trim().is_empty()) {
        return Err("OpenCode todo content is empty".into());
    }
    Ok(entries
        .into_iter()
        .map(|todo| PlanStep {
            text: (todo.content).into(),
            status: match todo.status {
                NativeTodoStatus::Pending => StepStatus::Pending,
                NativeTodoStatus::InProgress => StepStatus::InProgress,
                NativeTodoStatus::Completed => StepStatus::Done,
                NativeTodoStatus::Cancelled => StepStatus::Cancelled,
            },
        })
        .collect())
}

#[cfg(test)]
mod boundary_tests {
    use super::*;
    #[test]
    fn malformed_text_completion_does_not_publish_an_empty_replacement() {
        let mut decoder = Decoder::new(Some("root".into()));
        decoder.turn_open = true;
        let events = decoder.feed(&json!({"id":"bad-text","type":"session.next.text.ended","data":{"sessionID":"root","messageID":"m1","partID":"part1"}}).to_string()).unwrap();
        assert!(
            decoder
                .error
                .as_deref()
                .is_some_and(|message| message.contains("required final body"))
        );
        assert!(decoder.idle());
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, EngineEvent::Live { text, .. } if text.is_empty()))
        );
    }

    #[test]
    fn overflow_is_terminal_on_explicit_idle_but_clears_when_recovery_starts() {
        let error = json!({ "id": "overflow", "type": "session.error", "data": { "sessionID": "root", "error": { "name": "ContextOverflowError", "data": { "message": "too much context" } } } }).to_string();
        let idle = json!({ "id": "idle", "type": "session.status", "data": { "sessionID": "root", "status": { "type": "idle" } } }).to_string();
        let mut decoder = Decoder::new(Some("root".into()));
        decoder.feed(&idle).unwrap(); // Initial idle before prompt acceptance is harmless.
        assert!(decoder.error.is_none());
        decoder.turn_open = true;
        decoder.feed(&error).unwrap();
        decoder.feed(&idle).unwrap();
        assert!(decoder.idle());
        assert_eq!(decoder.error.as_deref(), Some("too much context"));
        let mut recovering = Decoder::new(Some("root".into()));
        recovering.turn_open = true;
        recovering.feed(&error).unwrap();
        recovering.feed(&json!({ "id": "step", "type": "session.next.step.started", "data": { "sessionID": "root" } }).to_string()).unwrap();
        recovering.feed(&idle).unwrap();
        assert!(recovering.error.is_none());
        assert!(!recovering.idle());
    }

    #[test]
    fn root_session_error_ends_without_waiting_for_a_step_but_overflow_can_compact() {
        let mut decoder = Decoder::new(Some("root".into()));
        decoder.turn_open = true;
        let recovering = decoder.feed(&json!({"id":"overflow", "type":"session.error", "data":{"sessionID":"root", "error":{"name":"ContextOverflowError", "data":{"message":"context full"}}}}).to_string()).unwrap();
        assert!(!decoder.idle() && decoder.error.is_none());
        assert!(matches!(
            &recovering[0],
            EngineEvent::Item {
                kind: ItemKind::Error {
                    retryable: true,
                    ..
                },
                ..
            }
        ));
        decoder.feed(&json!({"id":"failure", "type":"session.error", "data":{"sessionID":"root", "error":{"name":"APIError", "data":{"message":"denied"}}}}).to_string()).unwrap();
        assert!(decoder.idle());
        assert_eq!(decoder.error.as_deref(), Some("denied"));
    }

    #[test]
    fn malformed_questions_fail_explicitly_instead_of_creating_empty_items() {
        let mut decoder = Decoder::new(Some("root".into()));
        decoder.turn_open = true;
        let mut events = Vec::new();
        decoder.question_asked(
            "root",
            &json!({ "id": "ask", "questions": [{ "options": [] }] }),
            &mut events,
        );
        assert!(decoder.idle());
        assert!(decoder.error.is_some());
        assert!(matches!(
            &events[0],
            EngineEvent::Item {
                kind: ItemKind::Error { .. },
                ..
            }
        ));
        assert!(decoder.pending.is_empty());
    }
}

#[cfg(test)]
mod required_tool_fields_tests {
    use super::*;
    #[test]
    fn known_native_tool_inputs_cannot_form_empty_canonical_cards() {
        for name in [
            "bash",
            "read",
            "write",
            "edit",
            "grep",
            "glob",
            "webfetch",
            "websearch",
        ] {
            assert!(tool_kind(name, &json!({})).is_err(), "{name}");
        }
        assert!(tool_kind("write", &json!({"filePath":"file","content":""})).is_ok());
    }
}

#[cfg(test)]
mod malformed_frame_tests {
    use super::*;
    #[test]
    fn malformed_json_and_nonobject_frames_are_protocol_errors() {
        let mut decoder = Decoder::new(Some("root".into()));
        assert!(decoder.feed("{broken-json").is_err());
        assert!(decoder.feed("null").is_err());
        assert!(decoder.feed("[]").is_err());
        assert!(decoder.feed("{}").is_err());
    }
}

#[cfg(test)]
mod native_plan_identity_tests {
    use super::*;
    #[test]
    fn unfinished_native_todos_are_running_and_malformed_batches_never_become_partial_plans() {
        let mut decoder = Decoder::new(Some("root".into()));
        let events=decoder.feed(&json!({"id":"event-1","type":"todo.updated","data":{"sessionID":"root","todos":[{"content":"Task","status":"pending"}]}}).to_string()).unwrap();
        assert!(matches!(
            &events[0],
            EngineEvent::Item {
                status: Status::Running,
                kind: ItemKind::Plan { .. },
                ..
            }
        ));
        let events=decoder.feed(&json!({"id":"event-2","type":"todo.updated","data":{"sessionID":"root","todos":[{"content":"Valid","status":"completed"},{"status":"pending"}]}}).to_string()).unwrap();
        assert!(events.iter().all(|event| !matches!(
            event,
            EngineEvent::Item {
                kind: ItemKind::Plan { .. },
                ..
            }
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            EngineEvent::Item {
                kind: ItemKind::Error { .. },
                ..
            }
        )));
        assert!(
            decoder
                .feed(&json!({"type":"server.connected","data":{}}).to_string())
                .is_err()
        );
        let events=decoder.feed(&json!({"id":"event-3","type":"session.next.compaction.started","data":{"sessionID":"root"}}).to_string()).unwrap();
        assert!(events.iter().any(|event|matches!(event,EngineEvent::Item {kind:ItemKind::Error {message,..},..} if message.inline().expect("adapter error is inline").contains("messageID"))));
        assert!(events.iter().all(|event| !matches!(
            event,
            EngineEvent::Item {
                kind: ItemKind::Compaction { .. },
                ..
            }
        )));
    }
}

#[cfg(test)]
mod collection_partial_update_tests {
    use super::*;
    use artisan_api::CollectionRef;
    #[test]
    fn late_partial_edit_does_not_panic_or_drop_referenced_changes() {
        let records: Vec<FileChange> = (0..5000)
            .map(|n| FileChange {
                path: format!("file-{n}"),
                op: FileOp::Modify,
                diff: None,
            })
            .collect();
        let reference = CollectionRef::from_records(&records).unwrap();
        let mut changes = Collection::Stored(reference.clone());
        settle_changes(&mut changes, &json!({})).unwrap();
        assert_eq!(changes.stored(), Some(&reference));
        assert!(settle_changes(&mut changes, &json!({"existed":true})).is_err());
        assert_eq!(changes.stored(), Some(&reference));
        settle_changes(
            &mut changes,
            &json!({"files":[{"file":"changed","status":"added"}]}),
        )
        .unwrap();
        assert_eq!(changes.inline().unwrap()[0].path, "changed");
        assert_eq!(changes.inline().unwrap()[0].op, FileOp::Add);
    }
}

#[cfg(test)]
mod step_owner_tests {
    use super::*;
    #[test]
    fn completed_text_is_owned_until_its_native_step_ends() {
        let mut decoder = Decoder::new(Some("root".into()));
        let text = "step 界🙂 ".repeat(4096);
        decoder.feed(&json!({"id":"text", "type":"session.next.text.ended", "data":{"sessionID":"root", "assistantMessageID":"message", "textID":"part", "text":text}}).to_string()).unwrap();
        assert!(decoder.ownership.bytes() >= text.len() * 4);
        let before = decoder.ownership.bytes();
        decoder
            .feed(
                &json!({"id":"extension", "type":"extension", "data":{"sessionID":"root"}})
                    .to_string(),
            )
            .unwrap();
        assert_eq!(decoder.ownership.bytes(), before);
        decoder.feed(&json!({"id":"step", "type":"session.next.step.ended", "data":{"sessionID":"root", "finish":"stop"}}).to_string()).unwrap();
        assert!(decoder.step_texts.get("root").is_none());
        assert_eq!(decoder.ownership.bytes(), 0);
    }
}
