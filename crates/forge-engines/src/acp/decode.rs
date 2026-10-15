//! The pure half of the ACP adapter: JSON-RPC lines in, [`EngineEvent`]s
//! and replies out. No I/O; the driver in `acp.rs` writes `outbox` to the
//! engine and reacts to `responses`.
//!
//! This module reads the session stream (messages, thoughts, usage, goals,
//! the prompt's end); `tools` turns tool calls into items, `requests`
//! answers what the engine asks the user (permissions, Cursor questions),
//! `elicitation` maps MCP-style forms onto questions and `plan` keeps the
//! run's plan.

mod elicitation;
mod plan;
mod requests;
mod tools;

use super::Profile;
use crate::{EngineEvent, LiveKind};
use anyhow::Context as _;
use artisan_api::{ErrorKind, ItemKind, NoticeKind, Phase, Status, Usage};
use plan::Plan;
use requests::{Pending, Permission};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet, VecDeque};
use tools::Tool;

const PLAN_KEY: &str = "plan";
const GOAL_KEY: &str = "goal";

pub(crate) use crate::rpc::Response;

#[derive(Debug)]
struct Message {
    key: String,
    native: Option<String>,
    text: String,
}

pub(crate) struct AcpDecoder {
    reasoning_summary: crate::reasoning::Summary,
    profile: Profile,
    rpc: crate::rpc::State,
    /// Session updates outside a prompt are `session/load` history.
    prompting: bool,
    message: Option<Message>,
    messages: u64,
    questions: u64,
    tools: HashMap<String, Tool>,
    /// Tool cards Cursor also reports as a plan or a question.
    hidden: HashSet<String>,
    pending: HashMap<String, Pending>,
    /// Permission requests by the tool they gate, oldest first: the tool
    /// shows the first until it is answered.
    permissions: HashMap<String, VecDeque<Permission>>,
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
            reasoning_summary: crate::reasoning::Summary::default(),
            rpc: crate::rpc::State::acp(),
            prompting: false,
            message: None,
            messages: 0,
            questions: 0,
            tools: HashMap::new(),
            hidden: HashSet::new(),
            pending: HashMap::new(),
            permissions: HashMap::new(),
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
        self.on_sent(method, &params);
        self.rpc.request(method, params)
    }

    pub(crate) fn notify(&mut self, method: &str, params: Value) {
        self.rpc.notify(method, Some(params));
    }

    /// Records the exact requests from a transcript fixture.
    #[cfg(test)]
    pub(crate) fn sent(&mut self, message: &Value) {
        let (Some(_), Some(method)) = (message["id"].as_u64(), message["method"].as_str()) else {
            return;
        };
        self.rpc.sent(message);
        self.on_sent(method, &message["params"]);
    }

    fn on_sent(&mut self, method: &str, params: &Value) {
        match method {
            "session/prompt" => {
                self.prompting = true;
                self.stop_reason = None;
            }
            "session/load" => self.session_id = params["sessionId"].as_str().map(str::to_owned),
            _ => {}
        }
    }

    pub(crate) fn take_outbox(&mut self) -> Vec<Value> {
        std::mem::take(&mut self.rpc.outbox)
    }

    pub(crate) fn take_responses(&mut self) -> Vec<Response> {
        std::mem::take(&mut self.rpc.responses)
    }

    pub(crate) fn feed(&mut self, line: &str) -> anyhow::Result<Vec<EngineEvent>> {
        let message: Value = serde_json::from_str(line).context("decoding ACP protocol JSON")?;
        anyhow::ensure!(message.is_object(), "ACP protocol frame must be an object");
        anyhow::ensure!(
            message["method"].is_string() || message.get("id").is_some(),
            "ACP protocol frame has neither a method nor an id"
        );
        let events = self.feed_raw(&message);
        let events = self.reasoning_summary.normalize(events, false);
        Ok(crate::retained::settle(self, events))
    }

    fn feed_raw(&mut self, message: &Value) -> Vec<EngineEvent> {
        let mut events = Vec::new();
        match (
            message.get("method").and_then(Value::as_str),
            message.get("id"),
        ) {
            (Some(method), Some(id)) => {
                self.engine_request(method, id.clone(), &message["params"], &mut events);
            }
            (Some(method), None) => self.notification(method, &message["params"], &mut events),
            (None, Some(_)) => self.response(message, &mut events),
            (None, None) => {}
        }
        events
    }

    fn response(&mut self, message: &Value, events: &mut Vec<EngineEvent>) {
        let Some(Response { id, method, result }) = self.rpc.response(message) else {
            return;
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
        self.rpc.responses.push(Response { id, method, result });
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
                key: format!("stop:{}", self.rpc.next_id),
                parent: None,
                status: Status::Done,
                kind: ItemKind::Error {
                    kind,
                    message: (message.to_owned()).into(),
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
                self.goal(update, events);
            }
            _ => tracing::debug!(method, "ignored unsupported ACP notification"),
        }
    }

    fn session_update(&mut self, update: &Value, events: &mut Vec<EngineEvent>) {
        match update["sessionUpdate"].as_str().unwrap_or_default() {
            "agent_message_chunk" => {
                let text = crate::native::content_text(&update["content"]).unwrap_or_default();
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
                            text: (String::new()).into(),
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
                let text = crate::native::content_text(&update["content"]).unwrap_or_default();
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
            "plan" => self.session_plan(&update["entries"], events),
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
                text: text.into(),
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
            "cursor/update_todos" => self.update_todos(rpc, params, events),
            // Informational extension calls; Cursor ignores the reply.
            "cursor/task" | "cursor/generate_image" => self.reply(rpc, json!({})),
            _ => {
                tracing::debug!(engine = ?self.profile, method, "declining an unsupported engine request");
                let message = format!("method not found: {method}");
                self.rpc
                    .reply_error(rpc, crate::rpc::METHOD_NOT_FOUND, &message);
            }
        }
    }

    /// Answers the approval or question on `key`. Unknown keys are ignored:
    /// the request may already have been cancelled.
    pub(crate) fn answer(&mut self, key: &str, answer: &artisan_api::Answer) -> Vec<EngineEvent> {
        let events = self.answer_raw(key, answer);
        crate::retained::settle(self, events)
    }

    /// `session/cancel` obliges the client to settle every open request as
    /// cancelled.
    pub(crate) fn cancel_pending(&mut self) -> Vec<EngineEvent> {
        self.cancel_pending_inner(true)
    }

    fn cancel_pending_inner(&mut self, include_nonblocking: bool) -> Vec<EngineEvent> {
        let mut events = Vec::new();
        self.cancel_permissions(&mut events);
        let mut keys: Vec<String> = self.pending.keys().cloned().collect();
        keys.sort();
        for key in keys {
            if !include_nonblocking
                && self.profile == Profile::Grok
                && matches!(self.pending.get(&key), Some(Pending::Elicitation { .. }))
            {
                continue;
            }
            let Some(pending) = self.pending.remove(&key) else {
                continue;
            };
            match pending {
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
        let mut events = self.cancel_pending_inner(cancelled || self.profile == Profile::Cursor);
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
        self.rpc.reply(rpc, result);
    }

    fn close_message(&mut self, phase: Phase, events: &mut Vec<EngineEvent>) {
        if let Some(message) = self.message.take() {
            events.push(EngineEvent::Item {
                key: message.key,
                parent: None,
                status: Status::Done,
                kind: ItemKind::AssistantMessage {
                    text: (message.text).into(),
                    phase,
                },
            });
        }
    }
}

/// ACP's `AUTH_REQUIRED` error code: the engine is signed out. Grok 1.0.50
/// and Cursor both answer `session/new` with it (see the `signed-out`
/// transcripts).
const AUTH_REQUIRED: i64 = -32000;

/// Whether an error response means the engine is signed out.
pub(crate) fn needs_sign_in(error: &crate::rpc::RpcError) -> bool {
    error.code == Some(AUTH_REQUIRED)
}

impl crate::retained::Retains for AcpDecoder {
    fn retained(&mut self, key: &str) -> Option<&mut ItemKind> {
        self.tools.get_mut(key)?.settled.as_mut()
    }
}
