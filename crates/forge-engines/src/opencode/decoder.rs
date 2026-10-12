//! Pure decoding of OpenCode's v2 event stream (`GET /api/event`).
//!
//! The global stream carries every session on the server, so the decoder
//! follows the run's root session plus any child session whose parent it
//! already follows; everything else is ignored. Shapes were captured from
//! OpenCode 1.18.32 (see `transcripts/opencode`).

use crate::{EngineEvent, LiveKind};
use artisan_api::{
    Answer, Approval, ApprovalRequest, Decision, ErrorKind, FileChange, FileOp, ItemId, ItemKind,
    Output, Phase, PlanStep, Question, QuestionOption, SearchMode, Status, StepStatus, ToolKind,
    Usage,
};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};

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
    cancelled: bool,
}

struct Tool {
    session: String,
    status: Status,
    kind: ItemKind,
    /// Set for `task` calls whose child session isn't known yet.
    awaiting_child: bool,
}

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
            Some(id) if self.prompted.contains(message) => vec![EngineEvent::SteerAccepted { id }],
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
    pub fn feed(&mut self, data: &str) -> Vec<EngineEvent> {
        let Ok(event) = serde_json::from_str::<Value>(data) else {
            tracing::warn!("OpenCode sent an event that is not JSON");
            return Vec::new();
        };
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
                        out.push(EngineEvent::SteerAccepted { id });
                    }
                }
                self.turn_open = true;
            }
            "session.next.step.started" => {
                if root {
                    self.turn_open = true;
                    if let Some(model) = model_id(&data["model"])
                        && self.model.as_ref() != Some(&model)
                    {
                        self.model = Some(model.clone());
                        out.push(EngineEvent::Model { id: model });
                    }
                }
                self.step_texts.remove(session);
            }
            "session.next.text.started" => {
                let key = text_key(data);
                self.open_texts.insert(key.clone());
                out.push(EngineEvent::Item {
                    key,
                    parent,
                    status: Status::Running,
                    kind: ItemKind::AssistantMessage {
                        text: String::new(),
                        phase: Phase::Commentary,
                    },
                });
            }
            "session.next.text.delta" => {
                if let Some(text) = data["delta"].as_str() {
                    out.push(EngineEvent::Live {
                        target: LiveKind::Text {
                            key: text_key(data),
                        },
                        text: text.to_owned(),
                        append: true,
                    });
                }
            }
            "session.next.text.ended" => {
                let key = text_key(data);
                self.open_texts.remove(&key);
                let text = data["text"].as_str().unwrap_or_default().to_owned();
                self.step_texts
                    .entry(session.to_owned())
                    .or_default()
                    .push((key.clone(), text.clone()));
                out.push(EngineEvent::Item {
                    key,
                    parent,
                    status: Status::Done,
                    kind: ItemKind::AssistantMessage {
                        text,
                        phase: Phase::Commentary,
                    },
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
                out.push(error_item(&event, parent, ErrorKind::Api, &message));
                if root {
                    self.turn_open = false;
                    self.error = Some(message);
                }
            }
            "session.error" => {
                let message = error_message(&data["error"]);
                out.push(error_item(&event, parent, ErrorKind::Api, &message));
                if root {
                    self.error = Some(message);
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
                let key = format!(
                    "compaction:{}",
                    data["messageID"].as_str().unwrap_or_default()
                );
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
                let steps = todo_steps(&data["todos"]);
                out.push(EngineEvent::Item {
                    key: "plan".to_owned(),
                    parent: None,
                    status: Status::Done,
                    kind: ItemKind::Plan {
                        steps,
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
            _ => {}
        }
        out
    }

    /// Answers the approval or question on item `key`: the events to emit
    /// now and the request that tells OpenCode.
    pub fn answer(&mut self, key: &str, answer: &Answer) -> Option<(Reply, Vec<EngineEvent>)> {
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
                        *stored = Some(answers.clone());
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
                    let answers: Vec<Vec<String>> = answers
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
        // The task call that spawned it: the oldest one in the parent
        // session still waiting for its child.
        let mut calls: Vec<(&String, &mut Tool)> = self
            .tools
            .iter_mut()
            .filter(|(_, tool)| tool.session == parent_session && tool.awaiting_child)
            .collect();
        calls.sort_by(|a, b| a.0.cmp(b.0));
        let agent = if let Some((key, tool)) = calls.into_iter().next() {
            tool.awaiting_child = false;
            key.clone()
        } else {
            // A child we can't tie to a call still gets an Agent item.
            let key = format!("agent:{session}");
            out.push(EngineEvent::Item {
                key: key.clone(),
                parent: self.sessions[parent_session].clone(),
                status: Status::Running,
                kind: ItemKind::Agent {
                    name: info["agent"].as_str().map(str::to_owned),
                    description: info["title"].as_str().unwrap_or_default().to_owned(),
                    prompt: None,
                    agent_id: None,
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
                description: str_at(input, "prompt")
                    .or_else(|| str_at(input, "description"))
                    .unwrap_or_default(),
                prompt: None,
                agent_id: None,
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
                tool: tool_kind(name, input),
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
                *result = text;
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
                    settle_changes(changes, structured);
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
        if data["finish"].as_str() == Some("tool-calls") {
            return;
        }
        // The step that ends the turn holds the reply.
        if let Some((key, text)) = texts.into_iter().last() {
            out.push(EngineEvent::Item {
                key,
                parent,
                status: Status::Done,
                kind: ItemKind::AssistantMessage {
                    text,
                    phase: Phase::Final,
                },
            });
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
                command: resources.join("\n"),
                cwd: None,
            },
            "edit" => ApprovalRequest::FileChange,
            _ => ApprovalRequest::Action {
                description: format!("{action} {}", resources.join(", "))
                    .trim()
                    .to_owned(),
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
                ),
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
        let questions = data["questions"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
            .map(|(index, question)| Question {
                id: index.to_string(),
                header: str_at(question, "header"),
                text: str_at(question, "question").unwrap_or_default(),
                multi_select: question["multiple"].as_bool() == Some(true),
                options: question["options"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|option| QuestionOption {
                        label: str_at(option, "label").unwrap_or_default(),
                        description: str_at(option, "description"),
                    })
                    .collect(),
            })
            .collect();
        self.tools.insert(
            key.clone(),
            Tool {
                session: session.to_owned(),
                status: Status::AwaitingUser,
                kind: ItemKind::Question {
                    blocking: true,
                    questions,
                    answers: None,
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
                    Some(
                        answer
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(Value::as_str)
                            .map(str::to_owned)
                            .collect(),
                    )
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

fn text_key(data: &Value) -> String {
    format!(
        "{}/{}",
        data["assistantMessageID"].as_str().unwrap_or_default(),
        data["textID"].as_str().unwrap_or_default()
    )
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
) -> EngineEvent {
    EngineEvent::Item {
        key: format!("error:{}", event["id"].as_str().unwrap_or_default()),
        parent,
        status: Status::Failed,
        kind: ItemKind::Error {
            kind,
            message: message.to_owned(),
            retryable: false,
        },
    }
}

fn tool_kind(name: &str, input: &Value) -> ToolKind {
    let text = |key: &str| str_at(input, key);
    let path = || {
        text("path")
            .or_else(|| text("filePath"))
            .unwrap_or_default()
    };
    match name {
        "bash" => ToolKind::Shell {
            command: text("command").unwrap_or_default(),
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
                        limit.map_or(u64::MAX, |limit| start + limit.saturating_sub(1)),
                    ))
                }
            },
        },
        "write" => ToolKind::Edit {
            changes: vec![FileChange {
                path: path(),
                op: FileOp::Add,
                diff: None,
            }],
        },
        "edit" | "multiedit" => ToolKind::Edit {
            changes: vec![FileChange {
                path: path(),
                op: FileOp::Modify,
                diff: None,
            }],
        },
        "apply_patch" | "patch" => ToolKind::Edit {
            changes: patch_changes(input),
        },
        "grep" => ToolKind::Search {
            pattern: text("pattern").unwrap_or_default(),
            path: text("path"),
            mode: SearchMode::Grep,
        },
        "glob" => ToolKind::Search {
            pattern: text("pattern").unwrap_or_default(),
            path: text("path"),
            mode: SearchMode::Glob,
        },
        "list" | "ls" => ToolKind::Search {
            pattern: text("path").unwrap_or_else(|| ".".to_owned()),
            path: text("path"),
            mode: SearchMode::List,
        },
        "webfetch" => ToolKind::WebFetch {
            url: text("url").unwrap_or_default(),
        },
        "websearch" | "codesearch" => ToolKind::WebSearch {
            query: text("query").unwrap_or_default(),
        },
        _ => {
            let input = (!input.is_null()).then(|| input.to_string());
            match name.split_once('_') {
                Some((server, tool)) if !BUILTIN_TOOLS.contains(&name) => ToolKind::Mcp {
                    server: server.to_owned(),
                    tool: tool.to_owned(),
                    input,
                },
                _ => ToolKind::Other {
                    name: name.to_owned(),
                    input,
                },
            }
        }
    }
}

/// Files named in an `apply_patch` envelope (`*** Add File: path`, …).
fn patch_changes(input: &Value) -> Vec<FileChange> {
    let patch = input
        .as_object()
        .into_iter()
        .flat_map(|object| object.values())
        .find_map(|value| value.as_str().filter(|text| text.contains("*** ")))
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
fn settle_changes(changes: &mut Vec<FileChange>, structured: &Value) {
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
                diff: str_at(file, "patch"),
            })
            .collect();
    } else if structured["existed"].as_bool() == Some(true) {
        for change in changes {
            change.op = FileOp::Modify;
        }
    }
}

fn todo_steps(todos: &Value) -> Vec<PlanStep> {
    todos
        .as_array()
        .into_iter()
        .flatten()
        .map(|todo| PlanStep {
            text: str_at(todo, "content").unwrap_or_default(),
            status: match todo["status"].as_str() {
                Some("in_progress") => StepStatus::InProgress,
                Some("completed") => StepStatus::Done,
                Some("cancelled") => StepStatus::Cancelled,
                _ => StepStatus::Pending,
            },
        })
        .collect()
}
