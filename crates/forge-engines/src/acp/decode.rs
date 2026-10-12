//! The pure half of the ACP adapter: JSON-RPC lines in, [`EngineEvent`]s
//! and replies out. No I/O; the driver in `acp.rs` writes `outbox` to the
//! engine and reacts to `responses`.

use super::Profile;
use crate::{EngineEvent, LiveKind};
use artisan_api::{
    Answer, Approval, ApprovalRequest, Decision, ErrorKind, FileChange, FileOp, ItemKind,
    NoticeKind, Output, Phase, PlanStep, Question, QuestionOption, SearchMode, Status, StepStatus,
    ToolKind, Usage,
};
use serde_json::{Map, Value, json};
use std::collections::{HashMap, HashSet};

const PLAN_KEY: &str = "plan";
const GOAL_KEY: &str = "goal";

/// A response to one of our requests.
#[derive(Debug)]
pub(crate) struct Response {
    pub id: u64,
    pub method: String,
    pub result: Result<Value, String>,
}

/// An engine request waiting on the user, by item key.
#[derive(Debug)]
enum Pending {
    Permission {
        rpc: Value,
        options: Vec<(String, Decision)>,
    },
    /// `elicitation/create`: one schema property per question.
    Elicitation {
        rpc: Value,
        fields: Vec<(String, String)>,
        asked: Vec<Question>,
    },
    /// `cursor/ask_question`: question ids and their `(option id, label)`s.
    CursorQuestion {
        rpc: Value,
        questions: Vec<(String, Vec<(String, String)>)>,
        asked: Vec<Question>,
    },
    CursorPlan {
        rpc: Value,
    },
}

#[derive(Debug)]
struct Message {
    key: String,
    native: Option<String>,
    text: String,
}

#[derive(Debug, Default)]
struct Tool {
    kind: Option<String>,
    title: Option<String>,
    raw_input: Value,
    raw_output: Value,
    text: String,
    changes: Vec<FileChange>,
    locations: Vec<String>,
    status: Option<Status>,
    approval: Option<Approval>,
}

#[derive(Debug, Default)]
struct Plan {
    steps: Vec<PlanStep>,
    proposal: Option<String>,
    approval: Option<Approval>,
    status: Option<Status>,
}

pub(crate) struct AcpDecoder {
    profile: Profile,
    next_id: u64,
    requests: HashMap<u64, String>,
    outbox: Vec<Value>,
    responses: Vec<Response>,
    /// Session updates outside a prompt are `session/load` history.
    prompting: bool,
    message: Option<Message>,
    messages: u64,
    questions: u64,
    tools: HashMap<String, Tool>,
    /// Tool cards Cursor also reports as a plan or a question.
    hidden: HashSet<String>,
    pending: HashMap<String, Pending>,
    plan: Option<Plan>,
    /// The last tool or plan item emitted per key, so unchanged updates stay quiet.
    emitted: HashMap<String, (Status, ItemKind)>,
    usage: Usage,
    pub(crate) session_id: Option<String>,
    pub(crate) image_blocks: bool,
    pub(crate) embedded_context: bool,
    pub(crate) stop_reason: Option<String>,
}

impl AcpDecoder {
    pub(crate) fn new(profile: Profile) -> Self {
        Self {
            profile,
            next_id: 0,
            requests: HashMap::new(),
            outbox: Vec::new(),
            responses: Vec::new(),
            prompting: false,
            message: None,
            messages: 0,
            questions: 0,
            tools: HashMap::new(),
            hidden: HashSet::new(),
            pending: HashMap::new(),
            plan: None,
            emitted: HashMap::new(),
            usage: Usage::default(),
            session_id: None,
            image_blocks: false,
            embedded_context: false,
            stop_reason: None,
        }
    }

    /// Queues a request and returns its id.
    pub(crate) fn request(&mut self, method: &str, params: Value) -> u64 {
        self.next_id += 1;
        let message =
            json!({ "jsonrpc": "2.0", "id": self.next_id, "method": method, "params": params });
        self.sent(&message);
        self.outbox.push(message);
        self.next_id
    }

    pub(crate) fn notify(&mut self, method: &str, params: Value) {
        self.outbox
            .push(json!({ "jsonrpc": "2.0", "method": method, "params": params }));
    }

    /// Records a request we sent, so its response can be told apart.
    pub(crate) fn sent(&mut self, message: &Value) {
        let (Some(id), Some(method)) = (message["id"].as_u64(), message["method"].as_str()) else {
            return;
        };
        self.next_id = self.next_id.max(id);
        self.requests.insert(id, method.to_owned());
        match method {
            "session/prompt" => {
                self.prompting = true;
                self.stop_reason = None;
            }
            "session/load" => {
                self.session_id = message["params"]["sessionId"].as_str().map(str::to_owned)
            }
            _ => {}
        }
    }

    pub(crate) fn take_outbox(&mut self) -> Vec<Value> {
        std::mem::take(&mut self.outbox)
    }

    pub(crate) fn take_responses(&mut self) -> Vec<Response> {
        std::mem::take(&mut self.responses)
    }

    pub(crate) fn feed(&mut self, line: &str) -> Vec<EngineEvent> {
        let Ok(message) = serde_json::from_str::<Value>(line) else {
            tracing::debug!(engine = ?self.profile, "skipping a non-JSON line from the engine");
            return Vec::new();
        };
        let mut events = Vec::new();
        match (
            message.get("method").and_then(Value::as_str),
            message.get("id"),
        ) {
            (Some(method), Some(id)) => {
                self.engine_request(method, id.clone(), &message["params"], &mut events)
            }
            (Some(method), None) => self.notification(method, &message["params"], &mut events),
            (None, Some(id)) => self.response(id, &message, &mut events),
            (None, None) => {}
        }
        events
    }

    fn response(&mut self, id: &Value, message: &Value, events: &mut Vec<EngineEvent>) {
        let Some(id) = id.as_u64() else { return };
        let Some(method) = self.requests.remove(&id) else {
            return;
        };
        let result = match message.get("error") {
            Some(error) => Err(rpc_error(error)),
            None => Ok(message.get("result").cloned().unwrap_or(Value::Null)),
        };
        if let Ok(value) = &result {
            match method.as_str() {
                "initialize" => {
                    let prompt = &value["agentCapabilities"]["promptCapabilities"];
                    self.image_blocks = prompt["image"].as_bool().unwrap_or(false);
                    self.embedded_context = prompt["embeddedContext"].as_bool().unwrap_or(false);
                }
                "session/new" => {
                    if let Some(session) = value["sessionId"].as_str() {
                        self.session_id = Some(session.to_owned());
                        events.push(EngineEvent::Session {
                            id: session.to_owned(),
                        });
                    }
                    if let Some(model) = value["models"]["currentModelId"]
                        .as_str()
                        .filter(|model| !model.is_empty())
                    {
                        events.push(EngineEvent::Model {
                            id: model.to_owned(),
                        });
                    }
                }
                "session/load" => {
                    if let Some(session) = &self.session_id {
                        events.push(EngineEvent::Session {
                            id: session.clone(),
                        });
                    }
                    if let Some(model) = value["models"]["currentModelId"]
                        .as_str()
                        .filter(|model| !model.is_empty())
                    {
                        events.push(EngineEvent::Model {
                            id: model.to_owned(),
                        });
                    }
                }
                "session/prompt" => self.prompt_ended(value, events),
                _ => {}
            }
        }
        if method == "session/prompt" {
            self.prompting = false;
            if result.is_err() {
                self.close_message(Phase::Commentary, events);
            }
        }
        self.responses.push(Response { id, method, result });
    }

    fn prompt_ended(&mut self, result: &Value, events: &mut Vec<EngineEvent>) {
        let reason = result["stopReason"]
            .as_str()
            .unwrap_or("end_turn")
            .to_owned();
        let phase = if reason == "cancelled" {
            Phase::Commentary
        } else {
            Phase::Final
        };
        self.close_message(phase, events);
        if let Some(usage) = result.get("usage").filter(|usage| usage.is_object()) {
            let count = |name: &str| usage[name].as_u64().unwrap_or(0);
            self.usage.input += count("inputTokens");
            self.usage.output += count("outputTokens");
            self.usage.reasoning += count("thoughtTokens");
            self.usage.cache_read += count("cachedReadTokens");
            if let Some(write) = usage["cachedWriteTokens"].as_u64() {
                self.usage.cache_write = Some(self.usage.cache_write.unwrap_or(0) + write);
            }
            events.push(EngineEvent::Usage(self.usage.clone()));
        }
        let error = match reason.as_str() {
            "refusal" => Some((ErrorKind::Refusal, "The model declined to continue.")),
            "max_tokens" => Some((ErrorKind::Api, "The reply hit the model's output limit.")),
            "max_turn_requests" => Some((ErrorKind::MaxTurns, "The turn hit its step limit.")),
            _ => None,
        };
        if let Some((kind, message)) = error {
            events.push(EngineEvent::Item {
                key: format!("stop:{}", self.next_id),
                parent: None,
                status: Status::Done,
                kind: ItemKind::Error {
                    kind,
                    message: message.to_owned(),
                    retryable: false,
                },
            });
        }
        self.stop_reason = Some(reason);
    }

    fn notification(&mut self, method: &str, params: &Value, events: &mut Vec<EngineEvent>) {
        let update = &params["update"];
        match method {
            "session/update" if self.prompting => self.session_update(update, events),
            "_x.ai/session/update" if update["sessionUpdate"] == "goal_updated" => {
                self.goal(update, events)
            }
            _ => {}
        }
    }

    fn session_update(&mut self, update: &Value, events: &mut Vec<EngineEvent>) {
        match update["sessionUpdate"].as_str().unwrap_or_default() {
            "agent_message_chunk" => {
                let text = content_text(&update["content"]);
                if text.is_empty() {
                    return;
                }
                let native = update["messageId"].as_str().map(str::to_owned);
                if self
                    .message
                    .as_ref()
                    .is_some_and(|open| native.is_some() && open.native != native)
                {
                    self.close_message(Phase::Commentary, events);
                }
                let message = if let Some(message) = &mut self.message {
                    message
                } else {
                    self.messages += 1;
                    let key = match &native {
                        Some(id) => format!("message:{id}"),
                        None => format!("message:{}", self.messages),
                    };
                    events.push(EngineEvent::Item {
                        key: key.clone(),
                        parent: None,
                        status: Status::Running,
                        kind: ItemKind::AssistantMessage {
                            text: String::new(),
                            phase: Phase::Commentary,
                        },
                    });
                    self.message.insert(Message {
                        key,
                        native,
                        text: String::new(),
                    })
                };
                message.text.push_str(&text);
                events.push(EngineEvent::Live {
                    target: LiveKind::Text {
                        key: message.key.clone(),
                    },
                    text,
                    append: true,
                });
            }
            "agent_thought_chunk" => {
                let text = content_text(&update["content"]);
                if !text.is_empty() {
                    events.push(EngineEvent::Live {
                        target: LiveKind::Reasoning,
                        text,
                        append: true,
                    });
                }
            }
            "tool_call" | "tool_call_update" => {
                let Some(id) = update["toolCallId"].as_str() else {
                    return;
                };
                self.close_message(Phase::Commentary, events);
                self.merge_tool(id, update);
                self.emit_tool(id, events);
            }
            "plan" => {
                let steps = update["entries"]
                    .as_array()
                    .map(|entries| plan_steps(entries, "content"))
                    .unwrap_or_default();
                self.plan.get_or_insert_default().steps = steps;
                self.emit_plan(events);
            }
            "session_info_update" => {
                if let Some(title) = update["title"]
                    .as_str()
                    .map(str::trim)
                    .filter(|title| !title.is_empty())
                {
                    events.push(EngineEvent::Title {
                        title: title.to_owned(),
                    });
                }
            }
            "usage_update" => {
                self.usage.context_used = update["used"].as_u64().or(self.usage.context_used);
                self.usage.context_window = update["size"].as_u64().or(self.usage.context_window);
                if update["cost"]["currency"] == "USD" {
                    self.usage.cost_usd = update["cost"]["amount"].as_f64().or(self.usage.cost_usd);
                }
                events.push(EngineEvent::Usage(self.usage.clone()));
            }
            "goal_updated" => self.goal(update, events),
            // user_message_chunk is history replayed by session/load;
            // commands, modes and config options don't change the thread.
            _ => {}
        }
    }

    /// Grok's goal report; fields sit on the update or on a nested `goal`.
    fn goal(&mut self, update: &Value, events: &mut Vec<EngineEvent>) {
        let field = |name: &str| {
            update
                .get(name)
                .or_else(|| update.get("goal").and_then(|goal| goal.get(name)))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|text| !text.is_empty())
        };
        let state = if field("last_event") == Some("goal_cleared") {
            "cleared"
        } else {
            field("status").unwrap_or("active")
        };
        let text = [
            field("objective"),
            field("pause_message").or_else(|| field("result_summary")),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join("\n");
        events.push(EngineEvent::Item {
            key: GOAL_KEY.to_owned(),
            parent: None,
            status: Status::Done,
            kind: ItemKind::Notice {
                kind: NoticeKind::Goal {
                    state: state.to_owned(),
                },
                text,
            },
        });
    }

    fn engine_request(
        &mut self,
        method: &str,
        rpc: Value,
        params: &Value,
        events: &mut Vec<EngineEvent>,
    ) {
        match method {
            "session/request_permission" => self.permission(rpc, params, events),
            "elicitation/create" => self.elicitation(rpc, params, events),
            "cursor/ask_question" => self.cursor_question(rpc, params, events),
            "cursor/create_plan" => self.cursor_plan(rpc, params, events),
            "cursor/update_todos" => {
                if let Some(todos) = params["todos"].as_array() {
                    let steps = plan_steps(todos, "content");
                    let plan = self.plan.get_or_insert_default();
                    if params["merge"].as_bool().unwrap_or(false) {
                        for step in steps {
                            match plan.steps.iter_mut().find(|known| known.text == step.text) {
                                Some(known) => known.status = step.status,
                                None => plan.steps.push(step),
                            }
                        }
                    } else {
                        plan.steps = steps;
                    }
                    self.emit_plan(events);
                }
                self.reply(rpc, json!({}));
            }
            // Informational extension calls; Cursor ignores the reply.
            "cursor/task" | "cursor/generate_image" => self.reply(rpc, json!({})),
            _ => {
                tracing::debug!(engine = ?self.profile, method, "declining an unsupported engine request");
                self.outbox.push(json!({
                    "jsonrpc": "2.0",
                    "id": rpc,
                    "error": { "code": -32601, "message": format!("method not found: {method}") },
                }));
            }
        }
    }

    fn permission(&mut self, rpc: Value, params: &Value, events: &mut Vec<EngineEvent>) {
        let call = &params["toolCall"];
        let Some(id) = call["toolCallId"].as_str() else {
            self.reply(rpc, json!({ "outcome": { "outcome": "cancelled" } }));
            return;
        };
        self.close_message(Phase::Commentary, events);
        self.merge_tool(id, call);
        let options: Vec<(String, Decision)> = params["options"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|option| {
                let decision = match option["kind"].as_str()? {
                    "allow_once" => Decision::AllowOnce,
                    "allow_always" => Decision::AllowAlways,
                    "reject_once" => Decision::DenyOnce,
                    "reject_always" => Decision::DenyAlways,
                    _ => return None,
                };
                Some((option["optionId"].as_str()?.to_owned(), decision))
            })
            .collect();
        let tool = self.tools.get_mut(id).expect("merged above");
        let reason = content_blocks_text(&call["content"]);
        let request = match tool.kind.as_deref() {
            Some("execute") => ApprovalRequest::Command {
                command: shell_command(tool),
                cwd: str_field(&tool.raw_input, &["cwd"]),
            },
            Some("edit" | "delete" | "move") => ApprovalRequest::FileChange,
            _ => ApprovalRequest::Action {
                description: tool
                    .title
                    .clone()
                    .unwrap_or_else(|| "Approve this tool call?".to_owned()),
            },
        };
        tool.approval = Some(Approval {
            request,
            reason: (!reason.is_empty()).then_some(reason),
            options: options.iter().map(|(_, decision)| *decision).collect(),
            decision: None,
        });
        tool.status = Some(Status::AwaitingUser);
        self.pending
            .insert(id.to_owned(), Pending::Permission { rpc, options });
        self.emit_tool(id, events);
    }

    fn elicitation(&mut self, rpc: Value, params: &Value, events: &mut Vec<EngineEvent>) {
        let schema = &params["requestedSchema"];
        let Some(properties) = schema["properties"]
            .as_object()
            .filter(|_| params["mode"].as_str().is_none_or(|mode| mode == "form"))
        else {
            self.reply(rpc, json!({ "action": "decline" }));
            return;
        };
        let message = params["message"].as_str().unwrap_or_default();
        let mut fields = Vec::new();
        let mut questions = Vec::new();
        for (key, property) in properties {
            let kind = property["type"].as_str().unwrap_or("string").to_owned();
            let title = property["title"].as_str().filter(|title| !title.is_empty());
            let description = property["description"]
                .as_str()
                .filter(|text| !text.is_empty());
            questions.push(Question {
                id: key.clone(),
                header: title.map(str::to_owned),
                text: description.or(title).unwrap_or(message).to_owned(),
                multi_select: kind == "array",
                options: schema_choices(property),
            });
            fields.push((key.clone(), kind));
        }
        self.questions += 1;
        let key = format!("question:{}", self.questions);
        // Grok asks in passing and keeps working; nothing waits on the answer.
        let blocking = self.profile == Profile::Cursor;
        self.pending.insert(
            key.clone(),
            Pending::Elicitation {
                rpc,
                fields,
                asked: questions.clone(),
            },
        );
        self.close_message(Phase::Commentary, events);
        events.push(EngineEvent::Item {
            key,
            parent: None,
            status: Status::AwaitingUser,
            kind: ItemKind::Question {
                blocking,
                questions,
                answers: None,
            },
        });
    }

    fn cursor_question(&mut self, rpc: Value, params: &Value, events: &mut Vec<EngineEvent>) {
        let key = params["toolCallId"].as_str().map_or_else(
            || {
                self.questions += 1;
                format!("question:{}", self.questions)
            },
            str::to_owned,
        );
        self.hidden.insert(key.clone());
        let header = params["title"]
            .as_str()
            .filter(|title| !title.is_empty())
            .map(str::to_owned);
        let mut ids = Vec::new();
        let mut questions = Vec::new();
        for question in params["questions"].as_array().into_iter().flatten() {
            let id = question["id"].as_str().unwrap_or_default().to_owned();
            let options: Vec<(String, String)> = question["options"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|option| {
                    Some((
                        option["id"].as_str()?.to_owned(),
                        option["label"].as_str()?.to_owned(),
                    ))
                })
                .collect();
            questions.push(Question {
                id: id.clone(),
                header: header.clone(),
                text: question["prompt"].as_str().unwrap_or_default().to_owned(),
                multi_select: question["allowMultiple"].as_bool().unwrap_or(false),
                options: options
                    .iter()
                    .map(|(_, label)| QuestionOption {
                        label: label.clone(),
                        description: None,
                    })
                    .collect(),
            });
            ids.push((id, options));
        }
        self.pending.insert(
            key.clone(),
            Pending::CursorQuestion {
                rpc,
                questions: ids,
                asked: questions.clone(),
            },
        );
        self.close_message(Phase::Commentary, events);
        events.push(EngineEvent::Item {
            key,
            parent: None,
            status: Status::AwaitingUser,
            kind: ItemKind::Question {
                blocking: true,
                questions,
                answers: None,
            },
        });
    }

    fn cursor_plan(&mut self, rpc: Value, params: &Value, events: &mut Vec<EngineEvent>) {
        if let Some(id) = params["toolCallId"].as_str() {
            self.hidden.insert(id.to_owned());
        }
        let field = |name: &str| {
            params[name]
                .as_str()
                .map(str::trim)
                .filter(|text| !text.is_empty())
        };
        let description = field("overview")
            .or_else(|| field("name"))
            .unwrap_or("Approve this plan?")
            .to_owned();
        let plan = self.plan.get_or_insert_default();
        if let Some(todos) = params["todos"].as_array().filter(|todos| !todos.is_empty()) {
            plan.steps = plan_steps(todos, "content");
        }
        plan.proposal = field("plan").map(str::to_owned);
        plan.approval = Some(Approval {
            request: ApprovalRequest::Action { description },
            reason: None,
            options: vec![Decision::AllowOnce, Decision::DenyOnce],
            decision: None,
        });
        plan.status = Some(Status::AwaitingUser);
        self.pending
            .insert(PLAN_KEY.to_owned(), Pending::CursorPlan { rpc });
        self.close_message(Phase::Commentary, events);
        self.emit_plan(events);
    }

    /// Answers the approval or question on `key`. Unknown keys are ignored:
    /// the request may already have been cancelled.
    pub(crate) fn answer(&mut self, key: &str, answer: &Answer) -> Vec<EngineEvent> {
        let mut events = Vec::new();
        let Some(pending) = self.pending.remove(key) else {
            tracing::debug!(engine = ?self.profile, key, "answer for nothing pending");
            return events;
        };
        match (pending, answer) {
            (Pending::Permission { rpc, options }, Answer::Approval { decision }) => {
                let chosen = options
                    .iter()
                    .find(|(_, offered)| offered == decision)
                    .or_else(|| {
                        options
                            .iter()
                            .find(|(_, offered)| offered.allows() == decision.allows())
                    })
                    .filter(|_| *decision != Decision::Cancelled);
                let outcome = match chosen {
                    Some((option, _)) => json!({ "outcome": "selected", "optionId": option }),
                    None => json!({ "outcome": "cancelled" }),
                };
                self.reply(rpc, json!({ "outcome": outcome }));
                if let Some(tool) = self.tools.get_mut(key) {
                    if let Some(approval) = &mut tool.approval {
                        approval.decision = Some(*decision);
                    }
                    tool.status = Some(if decision.allows() {
                        Status::Running
                    } else {
                        Status::Declined
                    });
                }
                self.emit_tool(key, &mut events);
            }
            (Pending::Elicitation { rpc, fields, asked }, Answer::Question { answers }) => {
                if answers.iter().all(Option::is_none) {
                    self.reply(rpc, json!({ "action": "decline" }));
                } else {
                    let mut content = Map::new();
                    for ((name, kind), values) in fields.iter().zip(answers) {
                        let Some(values) = values else { continue };
                        let first = values.first().map_or("", String::as_str);
                        let value = match kind.as_str() {
                            "array" => json!(values),
                            "boolean" => json!(
                                first.eq_ignore_ascii_case("true")
                                    || first.eq_ignore_ascii_case("yes")
                            ),
                            "integer" => {
                                first.trim().parse::<i64>().map_or(Value::Null, Value::from)
                            }
                            "number" => {
                                first.trim().parse::<f64>().map_or(Value::Null, Value::from)
                            }
                            _ => json!(first),
                        };
                        content.insert(name.clone(), value);
                    }
                    self.reply(rpc, json!({ "action": "accept", "content": content }));
                }
                events.push(self.answered_question(key, asked, answers));
            }
            (
                Pending::CursorQuestion {
                    rpc,
                    questions,
                    asked,
                },
                Answer::Question { answers },
            ) => {
                let outcome = if answers.iter().all(Option::is_none) {
                    json!({ "outcome": "skipped", "reason": "User skipped questions" })
                } else {
                    let answers: Vec<Value> = questions
                        .iter()
                        .zip(answers)
                        .filter_map(|((id, options), values)| {
                            let selected: Vec<&str> = values
                                .as_ref()?
                                .iter()
                                .map(|value| {
                                    options
                                        .iter()
                                        .find(|(option, label)| option == value || label == value)
                                        .map_or(value.as_str(), |(option, _)| option.as_str())
                                })
                                .collect();
                            Some(json!({ "questionId": id, "selectedOptionIds": selected }))
                        })
                        .collect();
                    json!({ "outcome": "answered", "answers": answers })
                };
                self.reply(rpc, json!({ "outcome": outcome }));
                events.push(self.answered_question(key, asked, answers));
            }
            (Pending::CursorPlan { rpc }, Answer::Approval { decision }) => {
                let outcome = match decision {
                    Decision::Cancelled => "cancelled",
                    decision if decision.allows() => "accepted",
                    _ => "rejected",
                };
                self.reply(rpc, json!({ "outcome": { "outcome": outcome } }));
                let plan = self.plan.get_or_insert_default();
                if let Some(approval) = &mut plan.approval {
                    approval.decision = Some(*decision);
                }
                plan.status = Some(if decision.allows() {
                    Status::Running
                } else {
                    Status::Declined
                });
                self.emit_plan(&mut events);
            }
            (pending, answer) => {
                tracing::warn!(engine = ?self.profile, key, ?answer, "answer does not fit the pending request");
                self.pending.insert(key.to_owned(), pending);
            }
        }
        events
    }

    fn answered_question(
        &self,
        key: &str,
        questions: Vec<Question>,
        answers: &[Option<Vec<String>>],
    ) -> EngineEvent {
        EngineEvent::Item {
            key: key.to_owned(),
            parent: None,
            status: Status::Done,
            kind: ItemKind::Question {
                blocking: self.profile == Profile::Cursor,
                questions,
                answers: Some(answers.to_vec()),
            },
        }
    }

    /// `session/cancel` obliges the client to settle every open request as
    /// cancelled.
    pub(crate) fn cancel_pending(&mut self) -> Vec<EngineEvent> {
        let mut events = Vec::new();
        let mut keys: Vec<String> = self.pending.keys().cloned().collect();
        keys.sort();
        for key in keys {
            let Some(pending) = self.pending.remove(&key) else {
                continue;
            };
            match pending {
                Pending::Permission { rpc, .. } => {
                    self.reply(rpc, json!({ "outcome": { "outcome": "cancelled" } }));
                    if let Some(tool) = self.tools.get_mut(&key) {
                        if let Some(approval) = &mut tool.approval {
                            approval.decision = Some(Decision::Cancelled);
                        }
                        tool.status = Some(Status::Cancelled);
                    }
                    self.emit_tool(&key, &mut events);
                }
                Pending::Elicitation { rpc, .. } => {
                    self.reply(rpc, json!({ "action": "cancel" }));
                    events.push(EngineEvent::Status {
                        key,
                        status: Status::Cancelled,
                    });
                }
                Pending::CursorQuestion { rpc, .. } | Pending::CursorPlan { rpc } => {
                    self.reply(rpc, json!({ "outcome": { "outcome": "cancelled" } }));
                    events.push(EngineEvent::Status {
                        key,
                        status: Status::Cancelled,
                    });
                }
            }
        }
        events
    }

    /// Settles what the engine left open when the run ends.
    pub(crate) fn end_run(&mut self, cancelled: bool) -> Vec<EngineEvent> {
        let mut events = self.cancel_pending();
        self.close_message(Phase::Commentary, &mut events);
        if let Some(plan) = &self.plan
            && !plan.status.is_some_and(Status::is_terminal)
        {
            events.push(EngineEvent::Status {
                key: PLAN_KEY.to_owned(),
                status: Status::Done,
            });
        }
        if cancelled {
            let mut open: Vec<&String> = self
                .tools
                .iter()
                .filter(|(key, tool)| {
                    !self.hidden.contains(*key) && !tool.status.is_some_and(Status::is_terminal)
                })
                .map(|(key, _)| key)
                .collect();
            open.sort();
            events.extend(open.into_iter().map(|key| EngineEvent::Status {
                key: key.clone(),
                status: Status::Cancelled,
            }));
        }
        events
    }

    fn reply(&mut self, rpc: Value, result: Value) {
        self.outbox
            .push(json!({ "jsonrpc": "2.0", "id": rpc, "result": result }));
    }

    fn close_message(&mut self, phase: Phase, events: &mut Vec<EngineEvent>) {
        if let Some(message) = self.message.take() {
            events.push(EngineEvent::Item {
                key: message.key,
                parent: None,
                status: Status::Done,
                kind: ItemKind::AssistantMessage {
                    text: message.text,
                    phase,
                },
            });
        }
    }

    fn merge_tool(&mut self, id: &str, update: &Value) {
        let tool = self.tools.entry(id.to_owned()).or_default();
        if let Some(kind) = update["kind"].as_str() {
            tool.kind = Some(kind.to_owned());
        }
        if let Some(title) = update["title"].as_str().filter(|title| !title.is_empty()) {
            tool.title = Some(title.to_owned());
        }
        if !update["rawInput"].is_null() {
            tool.raw_input = update["rawInput"].clone();
        }
        if !update["rawOutput"].is_null() {
            tool.raw_output = update["rawOutput"].clone();
        }
        if let Some(locations) = update["locations"].as_array() {
            tool.locations = locations
                .iter()
                .filter_map(|location| location["path"].as_str().map(str::to_owned))
                .collect();
        }
        if let Some(content) = update["content"].as_array() {
            let text = content_blocks_text(&update["content"]);
            if !text.is_empty() {
                tool.text = text;
            }
            let changes: Vec<FileChange> = content
                .iter()
                .filter(|block| block["type"] == "diff")
                .filter_map(|block| file_change(tool.kind.as_deref(), block))
                .collect();
            if !changes.is_empty() {
                tool.changes = changes;
            }
        }
        let status = match update["status"].as_str() {
            Some("completed") => Some(Status::Done),
            Some("failed") => Some(Status::Failed),
            Some("pending" | "in_progress") if tool.status != Some(Status::AwaitingUser) => {
                Some(Status::Running)
            }
            _ => None,
        };
        if let Some(status) = status {
            tool.status = Some(
                tool.status
                    .map_or(status, |current| current.advance(status)),
            );
        }
    }

    fn emit_tool(&mut self, id: &str, events: &mut Vec<EngineEvent>) {
        let Some(tool) = self.tools.get(id) else {
            return;
        };
        let input_name = tool.raw_input["_toolName"].as_str();
        if self.hidden.contains(id)
            || matches!(
                input_name,
                Some("createPlan" | "askQuestion" | "updateTodos")
            )
        {
            return;
        }
        let status = tool.status.unwrap_or(Status::Running);
        let kind = if input_name == Some("task") {
            let prompt = str_field(&tool.raw_input, &["prompt"]);
            let description = str_field(&tool.raw_input, &["description"]);
            ItemKind::Agent {
                name: description
                    .clone()
                    .or_else(|| str_field(&tool.raw_input, &["subagentType"])),
                description: prompt.or(description).unwrap_or_default(),
                prompt: None,
                agent_id: None,
                background: false,
                result: (!tool.text.is_empty()).then(|| tool.text.clone()),
                label: None,
            }
        } else {
            let output = tool_output(tool);
            ItemKind::Tool {
                tool: tool_kind(tool),
                title: tool.title.clone(),
                output: (!output.is_empty()).then(|| Output::from_text(&output)),
                exit_code: ["exitCode", "exit_code"]
                    .iter()
                    .find_map(|name| tool.raw_output[name].as_i64())
                    .and_then(|code| i32::try_from(code).ok()),
                approval: tool.approval.clone(),
            }
        };
        self.emit_changed(id, status, kind, events);
    }

    fn emit_plan(&mut self, events: &mut Vec<EngineEvent>) {
        let Some(plan) = &self.plan else { return };
        let status = plan.status.unwrap_or(Status::Running);
        let kind = ItemKind::Plan {
            steps: plan.steps.clone(),
            proposal: plan.proposal.clone(),
            approval: plan.approval.clone(),
        };
        self.emit_changed(PLAN_KEY, status, kind, events);
    }

    fn emit_changed(
        &mut self,
        key: &str,
        status: Status,
        kind: ItemKind,
        events: &mut Vec<EngineEvent>,
    ) {
        let item = (status, kind);
        if self.emitted.get(key) == Some(&item) {
            return;
        }
        self.emitted.insert(key.to_owned(), item.clone());
        events.push(EngineEvent::Item {
            key: key.to_owned(),
            parent: None,
            status: item.0,
            kind: item.1,
        });
    }
}

fn rpc_error(error: &Value) -> String {
    let message = error["message"].as_str().unwrap_or("engine error");
    let detail = error["data"]["message"]
        .as_str()
        .or_else(|| error["data"].as_str());
    match detail {
        Some(detail) if detail.starts_with(message) => detail.to_owned(),
        Some(detail) => format!("{message}: {detail}"),
        _ => message.to_owned(),
    }
}

/// Whether an error response means the engine is signed out.
pub(crate) fn needs_sign_in(error: &str) -> bool {
    let error = error.to_ascii_lowercase();
    error.contains("authentication required")
        || error.contains("not authenticated")
        || error.contains("not logged in")
}

fn content_text(content: &Value) -> String {
    if content["type"] == "text" {
        content["text"].as_str().unwrap_or_default().to_owned()
    } else {
        String::new()
    }
}

/// The text of ACP `ToolCallContent` blocks (`{ type: "content", content }`).
fn content_blocks_text(blocks: &Value) -> String {
    blocks
        .as_array()
        .into_iter()
        .flatten()
        .filter(|block| block["type"] == "content")
        .map(|block| content_text(&block["content"]))
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn str_field(value: &Value, names: &[&str]) -> Option<String> {
    names
        .iter()
        .find_map(|name| value[name].as_str())
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

fn shell_command(tool: &Tool) -> String {
    match &tool.raw_input["command"] {
        Value::Array(parts) => parts
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(" "),
        _ => str_field(&tool.raw_input, &["command", "cmd"])
            // Cursor's permission requests carry the command only in the title.
            .or_else(|| tool.title.as_deref().map(|title| title.trim().trim_matches('`').to_owned()))
            .unwrap_or_default(),
    }
}

fn tool_path(tool: &Tool) -> String {
    str_field(
        &tool.raw_input,
        &["path", "file_path", "filePath", "abs_path"],
    )
    .or_else(|| tool.locations.first().cloned())
    .or_else(|| tool.title.clone())
    .unwrap_or_default()
}

fn tool_kind(tool: &Tool) -> ToolKind {
    let input = &tool.raw_input;
    match tool.kind.as_deref() {
        Some("execute") => ToolKind::Shell {
            command: shell_command(tool),
            cwd: str_field(input, &["cwd"]),
            background: false,
        },
        Some("read") => {
            let offset = input["offset"].as_u64();
            let range = input["limit"].as_u64().map(|limit| {
                let start = offset.unwrap_or(0) + 1;
                (start, start + limit - 1)
            });
            ToolKind::Read {
                path: tool_path(tool),
                range,
            }
        }
        Some(kind @ ("edit" | "delete" | "move")) => {
            let changes = if tool.changes.is_empty() {
                let op = match kind {
                    "delete" => FileOp::Delete,
                    "move" => FileOp::Move {
                        from: str_field(input, &["from", "source", "old_path"]).unwrap_or_default(),
                    },
                    _ => FileOp::Modify,
                };
                let paths = if tool.locations.is_empty() {
                    vec![tool_path(tool)]
                } else {
                    tool.locations.clone()
                };
                paths
                    .into_iter()
                    .filter(|path| !path.is_empty())
                    .map(|path| FileChange {
                        path,
                        op: op.clone(),
                        diff: None,
                    })
                    .collect()
            } else {
                tool.changes.clone()
            };
            ToolKind::Edit { changes }
        }
        Some("search") => {
            if let Some(query) = str_field(input, &["searchTerm"]) {
                ToolKind::WebSearch { query }
            } else {
                let glob = str_field(input, &["globPattern"]).is_some()
                    || tool
                        .title
                        .as_deref()
                        .is_some_and(|title| title.starts_with("Find"));
                let list = tool
                    .title
                    .as_deref()
                    .is_some_and(|title| title.starts_with("List"));
                ToolKind::Search {
                    pattern: str_field(input, &["pattern", "globPattern", "query", "regex"])
                        .or_else(|| tool.title.clone())
                        .unwrap_or_default(),
                    path: str_field(input, &["path"]),
                    mode: if list {
                        SearchMode::List
                    } else if glob {
                        SearchMode::Glob
                    } else {
                        SearchMode::Grep
                    },
                }
            }
        }
        Some("fetch") => match str_field(input, &["url"]) {
            Some(url) => ToolKind::WebFetch { url },
            None => ToolKind::WebSearch {
                query: str_field(input, &["query"])
                    .or_else(|| tool.title.clone())
                    .unwrap_or_default(),
            },
        },
        _ => match (
            str_field(input, &["providerIdentifier", "server"]),
            str_field(input, &["toolName", "tool"]),
        ) {
            (Some(server), Some(tool_name)) => ToolKind::Mcp {
                server,
                tool: tool_name,
                input: input.get("args").map(Value::to_string),
            },
            _ => ToolKind::Other {
                name: tool
                    .title
                    .clone()
                    .or_else(|| tool.kind.clone())
                    .unwrap_or_else(|| "tool".to_owned()),
                input: (!input.is_null()).then(|| input.to_string()),
            },
        },
    }
}

fn tool_output(tool: &Tool) -> String {
    if !tool.text.is_empty() {
        return tool.text.clone();
    }
    match &tool.raw_output {
        Value::String(text) => text.clone(),
        output @ Value::Object(_) => ["output", "stdout", "content", "stderr", "error"]
            .iter()
            .filter_map(|name| output[name].as_str())
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn file_change(kind: Option<&str>, block: &Value) -> Option<FileChange> {
    let path = block["path"].as_str()?.to_owned();
    let old = block["oldText"].as_str();
    let new = block["newText"].as_str().unwrap_or_default();
    let op = match (kind, old) {
        (_, None) => FileOp::Add,
        (Some("delete"), Some(_)) if new.is_empty() => FileOp::Delete,
        _ => FileOp::Modify,
    };
    let diff = similar::TextDiff::from_lines(old.unwrap_or_default(), new)
        .unified_diff()
        .header(&path, &path)
        .to_string();
    Some(FileChange {
        path,
        op,
        diff: (!diff.is_empty()).then_some(diff),
    })
}

fn plan_steps(entries: &[Value], text_field: &str) -> Vec<PlanStep> {
    entries
        .iter()
        .filter_map(|entry| {
            let text = entry[text_field]
                .as_str()
                .map(str::trim)
                .filter(|text| !text.is_empty())?;
            let status = match entry["status"].as_str() {
                Some("in_progress") => StepStatus::InProgress,
                Some("completed") => StepStatus::Done,
                Some("cancelled") => StepStatus::Cancelled,
                _ => StepStatus::Pending,
            };
            Some(PlanStep {
                text: text.to_owned(),
                status,
            })
        })
        .collect()
}

/// Choices offered by one elicitation schema property.
fn schema_choices(property: &Value) -> Vec<QuestionOption> {
    let items = &property["items"];
    let choices = property["oneOf"]
        .as_array()
        .or_else(|| property["enum"].as_array())
        .or_else(|| items["anyOf"].as_array())
        .or_else(|| items["oneOf"].as_array())
        .or_else(|| items["enum"].as_array());
    choices
        .into_iter()
        .flatten()
        .filter_map(|choice| match choice {
            Value::String(label) => Some(QuestionOption {
                label: label.clone(),
                description: None,
            }),
            Value::Object(_) => Some(QuestionOption {
                label: str_field(choice, &["title", "const"])?,
                description: str_field(choice, &["description"]),
            }),
            _ => None,
        })
        .collect()
}
