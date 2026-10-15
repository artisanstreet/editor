//! The pure half of the ACP adapter: JSON-RPC lines in, [`EngineEvent`]s
//! and replies out. No I/O; the driver in `acp.rs` writes `outbox` to the
//! engine and reacts to `responses`.

use super::Profile;
use crate::{EngineEvent, LiveKind};
use anyhow::Context as _;
use artisan_api::{
    Answer, Approval, ApprovalRequest, Decision, ErrorKind, FileChange, FileOp, ItemKind,
    NoticeKind, Output, Phase, PlanStep, Question, QuestionOption, SearchMode, Status, StepStatus,
    ToolKind, Usage,
};
use serde_json::{Map, Value, json};
use std::collections::{HashMap, HashSet};

#[derive(serde::Deserialize)]
struct CursorQuestion {
    id: String,
    prompt: String,
    #[serde(default)]
    options: Vec<CursorChoice>,
    #[serde(default, rename = "allowMultiple")]
    allow_multiple: bool,
}
#[derive(serde::Deserialize)]
struct CursorChoice {
    id: String,
    label: String,
}

fn permission_options(value: &Value) -> Result<Vec<(String, Decision)>, String> {
    #[derive(serde::Deserialize)]
    struct OptionEntry {
        #[serde(rename = "optionId")]
        id: String,
        kind: OptionKind,
    }
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "snake_case")]
    enum OptionKind {
        AllowOnce,
        AllowAlways,
        RejectOnce,
        RejectAlways,
    }
    let entries: Vec<OptionEntry> = serde_json::from_value(value.clone())
        .map_err(|error| format!("invalid permission options: {error}"))?;
    if entries.is_empty() {
        return Err("permission request requires offered options".into());
    }
    let mut ids = HashSet::new();
    let mut options = Vec::with_capacity(entries.len());
    for entry in entries {
        if entry.id.trim().is_empty() || !ids.insert(entry.id.clone()) {
            return Err("permission options require nonempty unique native IDs".into());
        }
        let decision = match entry.kind {
            OptionKind::AllowOnce => Decision::AllowOnce,
            OptionKind::AllowAlways => Decision::AllowAlways,
            OptionKind::RejectOnce => Decision::DenyOnce,
            OptionKind::RejectAlways => Decision::DenyAlways,
        };
        options.push((entry.id, decision));
    }
    Ok(options)
}

const PLAN_KEY: &str = "plan";
const GOAL_KEY: &str = "goal";

pub(crate) use crate::rpc::Response;

#[derive(Debug, serde::Serialize)]
struct ElicitationField {
    name: String,
    schema: Value,
    required: bool,
}

impl ElicitationField {
    fn value(&self, answer: &Option<artisan_api::QuestionAnswer>) -> Result<Option<Value>, String> {
        let Some(answer) = answer else {
            return if self.required {
                Err(format!("{} is required", self.name))
            } else {
                Ok(None)
            };
        };
        let kind = self.schema["type"].as_str().unwrap_or("string");
        let source = if kind == "array" {
            &self.schema["items"]
        } else {
            &self.schema
        };
        let choices = source["oneOf"]
            .as_array()
            .or_else(|| source["anyOf"].as_array())
            .or_else(|| source["enum"].as_array());
        let values = match answer {
            artisan_api::QuestionAnswer::Choices { ids } if choices.is_some() => ids
                .inline()
                .ok_or_else(|| format!("{} submitted choices must be inline", self.name))?
                .to_vec(),
            artisan_api::QuestionAnswer::Text { text } if choices.is_none() => {
                if kind == "array" {
                    let values: Vec<Value> = serde_json::from_str(text)
                        .map_err(|_| format!("{} requires a JSON array", self.name))?;
                    values
                        .into_iter()
                        .map(|value| {
                            value
                                .as_str()
                                .map(str::to_owned)
                                .unwrap_or_else(|| value.to_string())
                        })
                        .collect()
                } else {
                    vec![text.clone()]
                }
            }
            _ => return Err(format!("{} has an answer of the wrong kind", self.name)),
        };
        for unsupported in [
            "pattern",
            "format",
            "allOf",
            "not",
            "$ref",
            "multipleOf",
            "uniqueItems",
        ] {
            if source.get(unsupported).is_some() || self.schema.get(unsupported).is_some() {
                return Err(format!(
                    "{} uses unsupported schema constraint {unsupported}",
                    self.name
                ));
            }
        }
        let scalar_kind = source["type"].as_str().unwrap_or("string");
        let scalar = |text: &str| -> Result<Value, String> {
            if let Some(choices) = choices {
                return choices
                    .iter()
                    .enumerate()
                    .find_map(|(index, choice)| {
                        (text == format!("option-{index}")).then(|| {
                            choice
                                .get("const")
                                .cloned()
                                .unwrap_or_else(|| choice.clone())
                        })
                    })
                    .ok_or_else(|| format!("{} has an invalid choice", self.name));
            }
            let value = match scalar_kind {
                "boolean" => match text.trim().to_ascii_lowercase().as_str() {
                    "true" | "yes" => Value::Bool(true),
                    "false" | "no" => Value::Bool(false),
                    _ => return Err(format!("{} must be true or false", self.name)),
                },
                "integer" => text
                    .trim()
                    .parse::<i64>()
                    .map(Value::from)
                    .map_err(|_| format!("{} must be an integer", self.name))?,
                "number" => {
                    let number = text
                        .trim()
                        .parse::<f64>()
                        .map_err(|_| format!("{} must be a number", self.name))?;
                    serde_json::Number::from_f64(number)
                        .map(Value::Number)
                        .ok_or_else(|| format!("{} must be finite", self.name))?
                }
                "string" => Value::String(text.to_owned()),
                other => return Err(format!("{} has unsupported schema type {other}", self.name)),
            };
            if let Some(number) = value.as_f64() {
                if source["minimum"]
                    .as_f64()
                    .is_some_and(|minimum| number < minimum)
                    || source["maximum"]
                        .as_f64()
                        .is_some_and(|maximum| number > maximum)
                    || source["exclusiveMinimum"]
                        .as_f64()
                        .is_some_and(|minimum| number <= minimum)
                    || source["exclusiveMaximum"]
                        .as_f64()
                        .is_some_and(|maximum| number >= maximum)
                {
                    return Err(format!("{} is outside its allowed range", self.name));
                }
            }
            if let Some(text) = value.as_str() {
                let count = text.chars().count() as u64;
                if source["minLength"]
                    .as_u64()
                    .is_some_and(|minimum| count < minimum)
                    || source["maxLength"]
                        .as_u64()
                        .is_some_and(|maximum| count > maximum)
                {
                    return Err(format!("{} has an invalid length", self.name));
                }
            }
            Ok(value)
        };
        if kind == "array" {
            let count = values.len() as u64;
            if self.schema["minItems"]
                .as_u64()
                .is_some_and(|minimum| count < minimum)
                || self.schema["maxItems"]
                    .as_u64()
                    .is_some_and(|maximum| count > maximum)
            {
                return Err(format!("{} has an invalid number of selections", self.name));
            }
            Ok(Some(Value::Array(
                values
                    .iter()
                    .map(|value| scalar(value))
                    .collect::<Result<_, _>>()?,
            )))
        } else {
            if values.len() != 1 {
                return Err(format!("{} needs one value", self.name));
            }
            scalar(&values[0]).map(Some)
        }
    }
}

/// An engine request waiting on the user, by item key.
#[derive(Debug, serde::Serialize)]
enum Pending {
    Permission {
        rpc: Value,
        options: Vec<(String, Decision)>,
        target: String,
        approval: Approval,
    },
    /// `elicitation/create`: one schema property per question.
    Elicitation {
        rpc: Value,
        fields: Vec<ElicitationField>,
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
    exit_code: Option<i32>,
    text: String,
    output: Option<Output>,
    retained_kind: Option<ToolKind>,
    retained_agent: Option<ItemKind>,
    changes: Vec<FileChange>,
    locations: Vec<String>,
    status: Option<Status>,
}

#[derive(Debug, Default)]
struct Plan {
    todo_index: HashMap<String, usize>,
    steps: Vec<PlanStep>,
    proposal: Option<artisan_api::TextContent>,
    approval: Option<Approval>,
    status: Option<Status>,
}

pub(crate) struct AcpDecoder {
    ownership: crate::retained::Ownership,
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
            ownership: crate::retained::Ownership::default(),
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
            "native protocol frame is missing its required envelope identity"
        );
        let pending_frame = matches!(
            message["method"].as_str(),
            Some(
                "session/request_permission"
                    | "elicitation/create"
                    | "cursor/ask_question"
                    | "cursor/create_plan"
            )
        );
        if pending_frame {
            crate::retained::admit_pending(self.pending.len(), self.ownership.bytes(), &message)?;
        }
        let pending_before = self.pending.len();
        let events = self.feed_raw(&message);
        let events = self.reasoning_summary.normalize(events, false);
        self.refresh_ownership(
            &events,
            pending_frame || self.pending.len() != pending_before,
        )?;
        // Hidden provider tool cards can mutate without a canonical event.
        for id in [
            message["params"]["update"]["toolCallId"].as_str(),
            message["params"]["toolCall"]["toolCallId"].as_str(),
        ]
        .into_iter()
        .flatten()
        {
            self.charge_tool(id)?;
        }
        self.ownership.check()?;
        Ok(events)
    }

    fn charge_tool(&mut self, key: &str) -> anyhow::Result<()> {
        if let Some(tool) = self.tools.get(key) {
            let charge = crate::retained::owned_charge(&(
                &tool.kind,
                &tool.title,
                &tool.raw_input,
                &tool.text,
                &tool.output,
                &tool.retained_kind,
                &tool.retained_agent,
                &tool.changes,
                &tool.locations,
            ))?
            .saturating_add(crate::retained::owned_charge(&self.emitted.get(key))?);
            self.ownership.replace(key, charge);
        }
        Ok(())
    }

    fn refresh_ownership(
        &mut self,
        events: &[EngineEvent],
        pending_changed: bool,
    ) -> anyhow::Result<()> {
        for key in crate::retained::changed_keys(events) {
            self.charge_tool(key)?;
        }
        if events.iter().any(|event| matches!(event, EngineEvent::Item { key, .. } | EngineEvent::Status { key, .. } if key == PLAN_KEY)) {
            if let Some(plan) = &self.plan {
                let charge = crate::retained::owned_charge(&(&plan.todo_index, &plan.steps, &plan.proposal, &plan.approval))?
                    .saturating_add(crate::retained::owned_charge(&self.emitted.get(PLAN_KEY))?);
                self.ownership.replace_buffer(PLAN_KEY, charge);
            } else {
                self.ownership.remove_buffer(PLAN_KEY);
            }
        }
        if let Some(message) = &self.message {
            let charge = message
                .text
                .capacity()
                .saturating_add(message.key.capacity())
                .saturating_add(message.native.as_ref().map_or(0, String::capacity))
                .saturating_add(128)
                .saturating_mul(4);
            self.ownership.replace_buffer("open-message", charge);
        } else {
            self.ownership.remove_buffer("open-message");
        }
        if pending_changed {
            self.ownership
                .set_pending(crate::retained::owned_charge(&self.pending)?);
        }
        self.ownership.check()
    }

    fn feed_raw(&mut self, message: &Value) -> Vec<EngineEvent> {
        let mut events = Vec::new();
        match (
            message.get("method").and_then(Value::as_str),
            message.get("id"),
        ) {
            (Some(method), Some(id)) => {
                self.engine_request(method, id.clone(), &message["params"], &mut events)
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
                self.goal(update, events)
            }
            _ => tracing::debug!(method, "ignored unsupported ACP notification"),
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
                let parsed = match plan_steps(&update["entries"], false) {
                    Ok(parsed) => parsed,
                    Err(message) => {
                        self.plan_error(message, events);
                        return;
                    }
                };
                let plan = self.plan.get_or_insert_default();
                plan.steps = parsed.steps;
                plan.todo_index = parsed.index;
                if plan.status != Some(Status::AwaitingUser) {
                    plan.status = None;
                }
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
            "cursor/update_todos" => {
                let parsed = match plan_steps(&params["todos"], true) {
                    Ok(parsed) => parsed,
                    Err(message) => {
                        self.invalid_plan_request(rpc, message, events);
                        return;
                    }
                };
                let plan = self.plan.get_or_insert_default();
                if params["merge"].as_bool().unwrap_or(false) {
                    let mut incoming = parsed.index.into_iter().collect::<Vec<_>>();
                    incoming.sort_by_key(|(_, index)| *index);
                    for (id, index) in incoming {
                        let step = parsed.steps[index].clone();
                        if let Some(index) = plan.todo_index.get(&id) {
                            plan.steps[*index] = step;
                        } else {
                            plan.todo_index.insert(id, plan.steps.len());
                            plan.steps.push(step);
                        }
                    }
                } else {
                    plan.steps = parsed.steps;
                    plan.todo_index = parsed.index;
                }
                if plan.status != Some(Status::AwaitingUser) {
                    plan.status = None;
                }
                self.emit_plan(events);
                self.reply(rpc, json!({}));
            }
            // Informational extension calls; Cursor ignores the reply.
            "cursor/task" | "cursor/generate_image" => self.reply(rpc, json!({})),
            _ => {
                tracing::debug!(engine = ?self.profile, method, "declining an unsupported engine request");
                self.rpc.outbox.push(json!({
                    "jsonrpc": "2.0",
                    "id": rpc,
                    "error": { "code": -32601, "message": format!("method not found: {method}") },
                }));
            }
        }
    }

    fn permission(&mut self, rpc: Value, params: &Value, events: &mut Vec<EngineEvent>) {
        let call = &params["toolCall"];
        let parsed = permission_options(&params["options"]);
        let id = call["toolCallId"]
            .as_str()
            .filter(|id| !id.trim().is_empty());
        let (id, options) = match (id, parsed) {
            (Some(id), Ok(options)) => (id, options),
            (_, parsed) => {
                let message = parsed
                    .err()
                    .unwrap_or_else(|| "permission request requires a nonempty toolCallId".into());
                self.rpc.outbox.push(
                    json!({"jsonrpc":"2.0","id":rpc,"error":{"code":-32602,"message":message}}),
                );
                self.questions += 1;
                events.push(EngineEvent::Item {
                    key: format!("permission-error:{}", self.questions),
                    parent: None,
                    status: Status::Failed,
                    kind: ItemKind::Error {
                        kind: ErrorKind::Execution,
                        message: message.into(),
                        retryable: false,
                    },
                });
                return;
            }
        };
        self.close_message(Phase::Commentary, events);
        self.merge_tool(id, call);
        let tool = self.tools.get_mut(id).expect("merged above");
        let reason = content_blocks_text(&call["content"]);
        let request = match tool.kind.as_deref() {
            Some("execute") => ApprovalRequest::Command {
                command: (match shell_command(tool) {
                    Ok(command) => command,
                    Err(message) => {
                        self.reply(rpc, json!({ "outcome": { "outcome": "cancelled" } }));
                        events.push(EngineEvent::Item {
                            key: format!("{id}:permission-error"),
                            parent: None,
                            status: Status::Failed,
                            kind: ItemKind::Error {
                                kind: ErrorKind::Execution,
                                message: message.into(),
                                retryable: false,
                            },
                        });
                        return;
                    }
                })
                .into(),
                cwd: str_field(&tool.raw_input, &["cwd"]),
            },
            Some("edit" | "delete" | "move") => ApprovalRequest::FileChange,
            _ => ApprovalRequest::Action {
                description: (tool
                    .title
                    .clone()
                    .unwrap_or_else(|| "Approve this tool call?".to_owned()))
                .into(),
            },
        };
        let approval = Approval {
            request,
            reason: ((!reason.is_empty()).then_some(reason)).map(Into::into),
            options: options.iter().map(|(_, decision)| *decision).collect(),
            decision: None,
        };
        let key = format!("permission-request:{rpc}");
        self.pending.insert(
            key.clone(),
            Pending::Permission {
                rpc,
                options,
                target: id.to_owned(),
                approval: approval.clone(),
            },
        );
        self.emit_tool(id, events);
        events.push(EngineEvent::Approval {
            key,
            parent: None,
            target: Some(id.to_owned()),
            status: Status::AwaitingUser,
            approval,
        });
    }

    fn elicitation(&mut self, rpc: Value, params: &Value, events: &mut Vec<EngineEvent>) {
        let schema = &params["requestedSchema"];
        let Some(properties) = schema["properties"]
            .as_object()
            .filter(|properties| {
                !properties.is_empty() && properties.keys().all(|name| !name.trim().is_empty())
            })
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
                text: (description
                    .or(title)
                    .or_else(|| (!message.trim().is_empty()).then_some(message))
                    .unwrap_or(key)
                    .to_owned())
                .into(),
                multi_select: kind == "array",
                options: (schema_choices(property)).into(),
            });
            fields.push(ElicitationField {
                name: key.clone(),
                schema: property.clone(),
                required: schema["required"]
                    .as_array()
                    .is_some_and(|required| required.iter().any(|name| name.as_str() == Some(key))),
            });
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
                questions: questions.into(),
                answers: None,
                answer_text: None,
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
        let parsed = <Vec<CursorQuestion> as serde::Deserialize>::deserialize(&params["questions"]);
        let parsed = match parsed {
            Ok(questions)
                if !questions.is_empty()
                    && questions.iter().all(|question| {
                        !question.id.trim().is_empty()
                            && !question.prompt.trim().is_empty()
                            && question.options.iter().all(|option| {
                                !option.id.trim().is_empty() && !option.label.trim().is_empty()
                            })
                    }) =>
            {
                questions
            }
            _ => {
                tracing::warn!("Cursor supplied malformed questions");
                self.rpc.outbox.push(json!({ "jsonrpc": "2.0", "id": rpc, "error": { "code": -32602, "message": "questions require nonempty IDs and prompts, with valid option IDs and labels" } }));
                return;
            }
        };
        let duplicates = {
            let mut ids = HashSet::new();
            parsed.iter().any(|question| {
                !ids.insert(question.id.as_str()) || {
                    let mut options = HashSet::new();
                    question
                        .options
                        .iter()
                        .any(|option| !options.insert(option.id.as_str()))
                }
            })
        };
        if duplicates {
            self.rpc.outbox.push(json!({"jsonrpc":"2.0","id":rpc,"error":{"code":-32602,"message":"questions and their options require unique native IDs"}}));
            return;
        }
        self.hidden.insert(key.clone());
        let header = params["title"]
            .as_str()
            .filter(|title| !title.is_empty())
            .map(str::to_owned);
        let mut ids = Vec::new();
        let mut questions = Vec::new();
        for question in parsed {
            let id = question.id;
            let options: Vec<(String, String)> = question
                .options
                .into_iter()
                .map(|option| (option.id, option.label))
                .collect();
            questions.push(Question {
                id: id.clone(),
                header: header.clone(),
                text: (question.prompt).into(),
                multi_select: question.allow_multiple,
                options: options
                    .iter()
                    .map(|(id, label)| QuestionOption {
                        id: id.clone(),
                        label: label.clone(),
                        description: None,
                    })
                    .collect::<Vec<_>>()
                    .into(),
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
                questions: questions.into(),
                answers: None,
                answer_text: None,
            },
        });
    }

    fn cursor_plan(&mut self, rpc: Value, params: &Value, events: &mut Vec<EngineEvent>) {
        let parsed = match params
            .get("todos")
            .map(|todos| plan_steps(todos, true))
            .transpose()
        {
            Ok(parsed) => parsed,
            Err(message) => {
                self.invalid_plan_request(rpc, message, events);
                return;
            }
        };
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
        if let Some(parsed) = parsed {
            plan.steps = parsed.steps;
            plan.todo_index = parsed.index;
        }
        plan.proposal = field("plan").map(|text| text.to_owned().into());
        plan.approval = Some(Approval {
            request: ApprovalRequest::Action {
                description: description.into(),
            },
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
        let mut events = self.answer_raw(key, answer);
        if let Err(error) = self.refresh_ownership(&events, true) {
            events.push(crate::retained::ownership_error(key, error));
        }
        events
    }

    fn answer_raw(&mut self, key: &str, answer: &Answer) -> Vec<EngineEvent> {
        let mut events = Vec::new();
        let Some(pending) = self.pending.remove(key) else {
            tracing::debug!(engine = ?self.profile, key, "answer for nothing pending");
            return events;
        };
        match (pending, answer) {
            (
                Pending::Permission {
                    rpc,
                    options,
                    target,
                    mut approval,
                },
                Answer::Approval { decision },
            ) => {
                let Some((option, _)) = options.iter().find(|(_, offered)| offered == decision)
                else {
                    self.pending.insert(
                        key.to_owned(),
                        Pending::Permission {
                            rpc,
                            options,
                            target,
                            approval,
                        },
                    );
                    tracing::warn!(
                        key,
                        ?decision,
                        "approval answer was not offered; request remains pending"
                    );
                    return events;
                };
                self.reply(
                    rpc,
                    json!({"outcome":{"outcome":"selected","optionId":option}}),
                );
                approval.decision = Some(*decision);
                events.push(EngineEvent::Approval {
                    key: key.to_owned(),
                    parent: None,
                    target: Some(target),
                    status: if decision.allows() {
                        Status::Done
                    } else {
                        Status::Declined
                    },
                    approval,
                });
            }
            (Pending::Elicitation { rpc, fields, asked }, Answer::Question { answers }) => {
                if answers.iter().all(Option::is_none) {
                    self.reply(rpc, json!({ "action": "decline" }));
                } else {
                    let mut content = Map::new();
                    let validated = fields
                        .iter()
                        .enumerate()
                        .map(|(index, field)| {
                            field
                                .value(answers.get(index).unwrap_or(&None))
                                .map(|value| (field.name.clone(), value))
                        })
                        .collect::<Result<Vec<_>, _>>();
                    let values = match validated {
                        Ok(values) => values,
                        Err(message) => {
                            tracing::warn!(
                                key,
                                message,
                                "invalid elicitation answer; request remains pending"
                            );
                            events.push(EngineEvent::Item {
                                key: format!("answer-error:{key}"),
                                parent: None,
                                status: Status::Done,
                                kind: ItemKind::Error {
                                    kind: ErrorKind::Execution,
                                    message: (message.clone()).into(),
                                    retryable: true,
                                },
                            });
                            self.pending.insert(
                                key.to_owned(),
                                Pending::Elicitation { rpc, fields, asked },
                            );
                            return events;
                        }
                    };
                    for (name, value) in values {
                        if let Some(value) = value {
                            content.insert(name, value);
                        }
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
                    let selected = questions
                        .iter()
                        .zip(answers)
                        .filter_map(|((id, options), answer)| {
                            let answer = answer.as_ref()?;
                            let artisan_api::QuestionAnswer::Choices { ids } = answer else {
                                return Some(Err("Cursor accepts only choice IDs".to_owned()));
                            };
                            let Some(ids) = ids.inline() else {
                                return Some(Err(
                                    "Cursor submitted choices must be inline".to_owned()
                                ));
                            };
                            if ids.iter().any(|selected| {
                                !options.iter().any(|(option, _)| option == selected)
                            }) {
                                return Some(
                                    Err("Cursor received an unknown choice ID".to_owned()),
                                );
                            }
                            Some(Ok(json!({ "questionId": id, "selectedOptionIds": ids })))
                        })
                        .collect::<Result<Vec<_>, String>>();
                    let answers = match selected {
                        Ok(answers) => answers,
                        Err(message) => {
                            events.push(crate::question::invalid_answer(key, message));
                            self.pending.insert(
                                key.to_owned(),
                                Pending::CursorQuestion {
                                    rpc,
                                    questions,
                                    asked,
                                },
                            );
                            return events;
                        }
                    };
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
        answers: &[Option<artisan_api::QuestionAnswer>],
    ) -> EngineEvent {
        EngineEvent::Item {
            key: key.to_owned(),
            parent: None,
            status: Status::Done,
            kind: ItemKind::Question {
                blocking: self.profile == Profile::Cursor,
                questions: questions.into(),
                answers: Some(answers.to_vec().into()),
                answer_text: None,
            },
        }
    }

    /// `session/cancel` obliges the client to settle every open request as
    /// cancelled.
    pub(crate) fn cancel_pending(&mut self) -> Vec<EngineEvent> {
        self.cancel_pending_inner(true)
    }

    fn cancel_pending_inner(&mut self, include_nonblocking: bool) -> Vec<EngineEvent> {
        let mut events = Vec::new();
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
                Pending::Permission {
                    rpc,
                    target,
                    mut approval,
                    ..
                } => {
                    self.reply(rpc, json!({"outcome":{"outcome":"cancelled"}}));
                    approval.decision = Some(Decision::Cancelled);
                    events.push(EngineEvent::Approval {
                        key,
                        parent: None,
                        target: Some(target),
                        status: Status::Cancelled,
                        approval,
                    });
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
        if let Some(plan) = &mut self.plan {
            // A Status settlement reuses the already emitted source. Its native
            // editable state must release source bodies just like a final Item.
            for step in &mut plan.steps {
                crate::retained::compact_text(&mut step.text);
            }
            if let Some(proposal) = &mut plan.proposal {
                crate::retained::compact_text(proposal);
            }
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
        self.rpc
            .outbox
            .push(json!({ "jsonrpc": "2.0", "id": rpc, "result": result }));
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
            tool.retained_kind = None;
            tool.retained_agent = None;
        }
        if !update["rawOutput"].is_null() {
            let output = &update["rawOutput"];
            tool.exit_code = ["exitCode", "exit_code"]
                .iter()
                .find_map(|name| output[name].as_i64())
                .and_then(|code| i32::try_from(code).ok());
            let text = match output {
                Value::String(text) => text.clone(),
                Value::Object(_) => ["output", "stdout", "content", "stderr", "error"]
                    .iter()
                    .filter_map(|name| output[name].as_str())
                    .filter(|text| !text.is_empty())
                    .collect::<Vec<_>>()
                    .join("\n"),
                _ => String::new(),
            };
            if !text.is_empty() {
                tool.text = text;
            }
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
                if let Some(ToolKind::Edit { changes }) = &mut tool.retained_kind {
                    *changes = tool.changes.clone().into();
                }
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
        let Some(tool) = self.tools.get_mut(id) else {
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
        let kind = if let Some(mut agent) = tool.retained_agent.clone() {
            if let ItemKind::Agent { result, .. } = &mut agent
                && !tool.text.is_empty()
                && status.is_terminal()
            {
                *result = Some(tool.text.clone().into());
            }
            agent
        } else if input_name == Some("task") {
            let prompt = str_field(&tool.raw_input, &["prompt"]);
            let description = str_field(&tool.raw_input, &["description"]);
            ItemKind::Agent {
                name: str_field(&tool.raw_input, &["subagentType"]),
                description: (description.unwrap_or_default()).into(),
                prompt: prompt.map(Into::into),
                agent_id: None,
                context_used: None,
                model: None,
                background: false,
                result: ((status.is_terminal() && !tool.text.is_empty())
                    .then(|| tool.text.clone()))
                .map(Into::into),
                label: None,
            }
        } else {
            if !status.is_terminal() && !tool.text.is_empty() {
                let key = id.to_owned();
                let target = if tool.kind.as_deref() == Some("execute") {
                    LiveKind::Shell { key }
                } else {
                    LiveKind::Progress { key }
                };
                events.push(EngineEvent::Live {
                    target,
                    text: tool.text.clone(),
                    append: false,
                });
            }
            if status.is_terminal() && !tool.text.is_empty() {
                tool.output = Some(Output::from_text(&tool.text));
                tool.text = String::new();
            }
            let canonical = match tool
                .retained_kind
                .clone()
                .map(Ok)
                .unwrap_or_else(|| tool_kind(tool))
            {
                Ok(kind) => kind,
                Err(message) => {
                    // ToolCall updates may announce identity before their input.
                    // Only completed malformed calls become explicit errors.
                    if status.is_terminal() {
                        self.emit_changed(
                            id,
                            Status::Done,
                            ItemKind::Error {
                                kind: ErrorKind::Execution,
                                message: message.into(),
                                retryable: false,
                            },
                            events,
                        );
                    }
                    return;
                }
            };
            ItemKind::Tool {
                tool: canonical,
                title: tool.title.clone(),
                output: status.is_terminal().then(|| tool.output.clone()).flatten(),
                exit_code: tool.exit_code,
                approval: None,
            }
        };
        // Preserve the first full payload in the ordered output, while retaining
        // only immutable references for later partial status/output callbacks.
        let mut retained = kind.clone();
        if status.is_terminal() {
            if let Err(error) = crate::retained::compact_terminal(&mut retained) {
                self.plan_error(
                    format!("cannot normalize native tool records: {error}"),
                    events,
                );
                return;
            }
            match retained {
                ItemKind::Tool {
                    tool: canonical,
                    output,
                    ..
                } => {
                    tool.retained_kind = Some(canonical);
                    tool.output = output;
                }
                agent @ ItemKind::Agent { .. } => tool.retained_agent = Some(agent),
                _ => {}
            }
            let native_name = tool.raw_input["_toolName"].as_str().map(str::to_owned);
            tool.raw_input = native_name
                .map(|name| json!({"_toolName": name}))
                .unwrap_or(Value::Null);
            tool.changes = Vec::new();
            tool.text = String::new();
        }
        self.emit_changed(id, status, kind, events);
    }

    fn plan_error(&mut self, message: String, events: &mut Vec<EngineEvent>) {
        self.questions += 1;
        events.push(EngineEvent::Item {
            key: format!("plan-error:{}", self.questions),
            parent: None,
            status: Status::Failed,
            kind: ItemKind::Error {
                kind: ErrorKind::Execution,
                message: message.into(),
                retryable: false,
            },
        });
    }
    fn invalid_plan_request(&mut self, rpc: Value, message: String, events: &mut Vec<EngineEvent>) {
        self.rpc
            .outbox
            .push(json!({"jsonrpc":"2.0","id":rpc,"error":{"code":-32602,"message":message}}));
        self.plan_error(message, events);
    }
    fn emit_plan(&mut self, events: &mut Vec<EngineEvent>) {
        let Some(plan) = &self.plan else { return };
        let status = plan.status.unwrap_or_else(|| {
            if plan
                .steps
                .iter()
                .all(|step| matches!(step.status, StepStatus::Done | StepStatus::Cancelled))
            {
                Status::Done
            } else {
                Status::Running
            }
        });
        let kind = ItemKind::Plan {
            steps: (plan.steps.clone()).into(),
            proposal: plan.proposal.clone(),
            approval: plan.approval.clone(),
        };
        self.emit_changed(PLAN_KEY, status, kind, events);
        if status.is_terminal() {
            // Keep native todo IDs and editable records for real late deltas;
            // release only source strings already present in the full event.
            if let Some(plan) = &mut self.plan {
                for step in &mut plan.steps {
                    crate::retained::compact_text(&mut step.text);
                }
                if let Some(proposal) = &mut plan.proposal {
                    crate::retained::compact_text(proposal);
                }
            }
        }
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
        if status.is_terminal() {
            self.emitted.remove(key);
        } else {
            self.emitted.insert(key.to_owned(), item.clone());
        }
        events.push(EngineEvent::Item {
            key: key.to_owned(),
            parent: None,
            status: item.0,
            kind: item.1,
        });
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

fn shell_command(tool: &Tool) -> Result<artisan_api::TextContent, String> {
    if let Some(ToolKind::Shell { command, .. }) = &tool.retained_kind {
        return Ok(command.clone());
    }
    match &tool.raw_input["command"] {
        Value::Array(parts) => {
            let Some(parts) = parts.iter().map(Value::as_str).collect::<Option<Vec<_>>>() else {
                return Err("ACP command contains a non-string argument".into());
            };
            shlex::try_join(parts)
                .map(Into::into)
                .map_err(|error| format!("ACP command cannot be represented: {error}"))
        }
        _ => str_field(&tool.raw_input,&["command","cmd"])
            // Cursor permission messages may carry the actual command in title.
            .or_else(||tool.title.as_deref().map(|title|title.trim().trim_matches('`').to_owned()))
            .map(Into::into)
            .ok_or_else(||"ACP command is missing".to_owned()),
    }
}

fn tool_path(tool: &Tool) -> String {
    str_field(
        &tool.raw_input,
        &["path", "file_path", "filePath", "abs_path"],
    )
    .or_else(|| tool.locations.first().cloned())
    .unwrap_or_default()
}

fn tool_kind(tool: &Tool) -> Result<ToolKind, String> {
    let input = &tool.raw_input;
    let kind = match tool.kind.as_deref() {
        Some("execute") => ToolKind::Shell {
            command: (shell_command(tool)?).into(),
            cwd: str_field(input, &["cwd"]),
            background: false,
        },
        Some("read") => {
            let offset = input["offset"].as_u64();
            let range = input["limit"]
                .as_u64()
                .filter(|limit| *limit > 0)
                .map(|limit| {
                    let start = offset.unwrap_or(0).saturating_add(1);
                    (start, start.saturating_add(limit - 1))
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
            ToolKind::Edit {
                changes: changes.into(),
            }
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
                input: (input.get("args").map(Value::to_string)).map(Into::into),
            },
            _ => ToolKind::Other {
                name: tool
                    .title
                    .clone()
                    .or_else(|| tool.kind.clone())
                    .unwrap_or_else(|| "tool".to_owned()),
                input: ((!input.is_null()).then(|| input.to_string())).map(Into::into),
            },
        },
    };
    let invalid =
        match &kind {
            ToolKind::Shell { command, .. } => command
                .inline()
                .expect("adapter command is inline before storage")
                .trim()
                .is_empty(),
            ToolKind::Read { path, .. } => path.trim().is_empty(),
            ToolKind::Edit { changes } => changes.is_empty()
                || changes
                    .inline()
                    .expect("new native changes are inline")
                    .iter()
                    .any(|change| {
                        change.path.trim().is_empty()
                            || matches!(&change.op,FileOp::Move {from} if from.trim().is_empty())
                    }),
            ToolKind::Search { pattern, .. } => pattern.trim().is_empty(),
            ToolKind::WebFetch { url } => url.trim().is_empty(),
            ToolKind::WebSearch { query } => query.trim().is_empty(),
            ToolKind::Other { name, .. } => name.trim().is_empty(),
            _ => false,
        };
    if invalid {
        Err("ACP tool omitted required native fields".into())
    } else {
        Ok(kind)
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
        diff: ((!diff.is_empty()).then_some(diff)).map(Into::into),
    })
}

#[derive(serde::Deserialize)]
struct NativePlanEntry {
    #[serde(default)]
    id: Option<String>,
    content: String,
    status: NativePlanStatus,
}
#[derive(serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum NativePlanStatus {
    Pending,
    InProgress,
    Completed,
    Cancelled,
}
struct ParsedPlan {
    steps: Vec<PlanStep>,
    index: HashMap<String, usize>,
}
fn plan_steps(value: &Value, require_ids: bool) -> Result<ParsedPlan, String> {
    let entries = <Vec<NativePlanEntry> as serde::Deserialize>::deserialize(value)
        .map_err(|error| format!("malformed native plan entries: {error}"))?;
    let mut steps = Vec::with_capacity(entries.len());
    let mut index = HashMap::new();
    for entry in entries {
        if entry.content.trim().is_empty() {
            return Err("native plan step is empty".into());
        }
        if require_ids && entry.id.as_ref().is_none_or(|id| id.trim().is_empty()) {
            return Err("Cursor todo omitted its native ID".into());
        }
        if let Some(id) = entry.id {
            if id.trim().is_empty() || index.insert(id, steps.len()).is_some() {
                return Err("native todo IDs must be nonempty and unique".into());
            }
        }
        steps.push(PlanStep {
            text: (entry.content).into(),
            status: match entry.status {
                NativePlanStatus::Pending => StepStatus::Pending,
                NativePlanStatus::InProgress => StepStatus::InProgress,
                NativePlanStatus::Completed => StepStatus::Done,
                NativePlanStatus::Cancelled => StepStatus::Cancelled,
            },
        });
    }
    Ok(ParsedPlan { steps, index })
}

/// Choices offered by one elicitation schema property.
fn schema_choices(property: &Value) -> Vec<QuestionOption> {
    let items = &property["items"];
    let choices = property["oneOf"]
        .as_array()
        .or_else(|| property["anyOf"].as_array())
        .or_else(|| property["enum"].as_array())
        .or_else(|| items["anyOf"].as_array())
        .or_else(|| items["oneOf"].as_array())
        .or_else(|| items["enum"].as_array());
    choices
        .into_iter()
        .flatten()
        .enumerate()
        .filter_map(|(index, choice)| match choice {
            Value::String(label) => Some(QuestionOption {
                id: format!("option-{index}"),
                label: label.clone(),
                description: None,
            }),
            Value::Object(_) => Some(QuestionOption {
                id: format!("option-{index}"),
                label: str_field(choice, &["title"]).or_else(|| {
                    choice.get("const").map(|value| {
                        value
                            .as_str()
                            .map(str::to_owned)
                            .unwrap_or_else(|| value.to_string())
                    })
                })?,
                description: (str_field(choice, &["description"])).map(Into::into),
            }),
            Value::Number(_) | Value::Bool(_) => Some(QuestionOption {
                id: format!("option-{index}"),
                label: choice.to_string(),
                description: None,
            }),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod reform_validation_tests {
    use super::*;
    #[test]
    fn completed_tool_output_releases_raw_text_and_survives_later_status_updates() {
        let mut decoder = AcpDecoder::new(Profile::Cursor);
        let text = "x".repeat(1024 * 1024);
        decoder.merge_tool("shell", &json!({"kind":"execute","rawInput":{"command":"echo hello"},"rawOutput":text,"status":"completed"}));
        let mut events = Vec::new();
        decoder.emit_tool("shell", &mut events);
        let original = events
            .iter()
            .find_map(|event| match event {
                EngineEvent::Item {
                    kind: ItemKind::Tool { output, .. },
                    ..
                } => output.clone(),
                _ => None,
            })
            .unwrap();
        assert!(decoder.tools["shell"].text.is_empty());
        assert_eq!(original.text.inline(), Some(text.as_str()));
        assert_eq!(original.omitted_bytes, 0);
        decoder.merge_tool("shell", &json!({"status":"completed"}));
        events.clear();
        decoder.emit_tool("shell", &mut events);
        assert!(events.iter().any(|event| matches!(event, EngineEvent::Item { kind: ItemKind::Tool { output: Some(output), .. }, .. } if output.text == artisan_api::TextContent::Stored(artisan_api::ContentRef::from_text(&text)))));
    }

    #[test]
    fn terminal_source_refs_preserve_large_command_and_genuine_late_output() {
        let mut decoder = AcpDecoder::new(Profile::Cursor);
        let command = "printf 😀 ".repeat(8192);
        let output = "result\n".repeat(8192);
        decoder.merge_tool("shell", &json!({"kind":"execute","rawInput":{"command":command},"rawOutput":output,"status":"completed"}));
        let mut events = Vec::new();
        decoder.emit_tool("shell", &mut events);
        assert!(decoder.tools["shell"].raw_input.is_null());
        assert!(
            decoder.tools["shell"]
                .output
                .as_ref()
                .unwrap()
                .text
                .resident_bytes()
                < 256
        );
        decoder.merge_tool(
            "shell",
            &json!({"rawOutput":"later real output","status":"completed"}),
        );
        events.clear();
        decoder.emit_tool("shell", &mut events);
        assert!(events.iter().any(|event| matches!(event, EngineEvent::Item { kind: ItemKind::Tool { tool: ToolKind::Shell { command: value, .. }, output: Some(output), .. }, .. } if value == &artisan_api::TextContent::Stored(artisan_api::ContentRef::from_text(&command)) && output.text.inline() == Some("later real output"))));
    }

    #[test]
    fn read_ranges_never_overflow_or_reverse_for_zero_limits() {
        let mut tool = Tool {
            kind: Some("read".into()),
            raw_input: json!({"path":"file","offset":u64::MAX,"limit":u64::MAX}),
            ..Default::default()
        };
        assert!(
            matches!(tool_kind(&tool), Ok(ToolKind::Read { range: Some((start, end)), .. }) if start <= end)
        );
        tool.raw_input["limit"] = json!(0);
        assert!(matches!(
            tool_kind(&tool),
            Ok(ToolKind::Read { range: None, .. })
        ));
    }

    #[test]
    fn duplicate_display_labels_deliver_the_selected_native_value() {
        let mut decoder = AcpDecoder::new(Profile::Grok);
        let events = decoder.feed(&json!({"jsonrpc":"2.0","id":91,"method":"elicitation/create","params":{
            "mode":"form","requestedSchema":{"type":"object","properties":{"colour":{
                "type":"string","oneOf":[{"title":"Same","const":"first"},{"title":"Same","const":"second"}]
            }}}
        }}).to_string()).unwrap();
        let (key, options) = events
            .iter()
            .find_map(|event| match event {
                EngineEvent::Item {
                    key,
                    kind: ItemKind::Question { questions, .. },
                    ..
                } => Some((
                    key.clone(),
                    questions.inline().unwrap()[0]
                        .options
                        .inline()
                        .unwrap()
                        .to_vec(),
                )),
                _ => None,
            })
            .expect("native question");
        assert_eq!(options[0].label, options[1].label);
        assert_ne!(options[0].id, options[1].id);
        decoder.answer(
            &key,
            &Answer::Question {
                answers: vec![Some(artisan_api::QuestionAnswer::Choices {
                    ids: (vec![options[1].id.clone()]).into(),
                })],
            },
        );
        assert_eq!(
            decoder.take_outbox()[0]["result"]["content"]["colour"],
            "second"
        );
    }

    #[test]
    fn duplicate_cursor_labels_keep_native_option_identity() {
        let mut decoder = AcpDecoder::new(Profile::Cursor);
        let mut events = Vec::new();
        decoder.cursor_question(json!(7), &json!({"toolCallId":"question","questions":[{
            "id":"q","prompt":"Choose","options":[{"id":"first","label":"Same"},{"id":"second","label":"Same"}]
        }]}), &mut events);
        decoder.answer(
            "question",
            &Answer::Question {
                answers: vec![Some(artisan_api::QuestionAnswer::Choices {
                    ids: (vec!["second".into()]).into(),
                })],
            },
        );
        assert_eq!(
            decoder.take_outbox()[0]["result"]["outcome"]["answers"][0]["selectedOptionIds"],
            json!(["second"])
        );
    }

    #[test]
    fn malformed_cursor_questions_are_rejected_without_empty_items() {
        let mut decoder = AcpDecoder::new(Profile::Cursor);
        let mut events = Vec::new();
        decoder.cursor_question(json!(7), &json!({ "toolCallId": "question", "questions": [{"id": "", "prompt": "Choose", "options": []}] }), &mut events);
        assert!(events.is_empty() && decoder.pending.is_empty());
        assert_eq!(decoder.take_outbox()[0]["error"]["code"], -32602);
    }

    #[test]
    fn array_item_types_and_numeric_choices_are_preserved() {
        let field = ElicitationField {
            name: "counts".into(),
            required: true,
            schema: json!({ "type": "array", "items": { "type": "integer", "minimum": 2 }, "minItems": 1 }),
        };
        assert_eq!(
            field
                .value(&Some(artisan_api::QuestionAnswer::Text {
                    text: "[2,3]".into()
                }))
                .unwrap(),
            Some(json!([2, 3]))
        );
        assert!(
            field
                .value(&Some(artisan_api::QuestionAnswer::Text {
                    text: "[\"one\"]".into()
                }))
                .is_err()
        );
        assert!(
            field
                .value(&Some(artisan_api::QuestionAnswer::Text {
                    text: "[1]".into()
                }))
                .is_err()
        );
        let choices = ElicitationField {
            name: "count".into(),
            required: true,
            schema: json!({ "type": "integer", "enum": [1,2] }),
        };
        assert_eq!(
            choices
                .value(&Some(artisan_api::QuestionAnswer::Choices {
                    ids: (vec!["option-1".into()]).into()
                }))
                .unwrap(),
            Some(json!(2))
        );
        assert_eq!(schema_choices(&choices.schema)[1].label, "2");
    }

    #[test]
    fn elicitation_preserves_choice_values_and_rejects_invalid_scalars() {
        let choice = ElicitationField {
            name: "color".into(),
            required: true,
            schema: json!({ "type": "string", "oneOf": [{ "title": "Blue", "const": "blue-id" }] }),
        };
        assert_eq!(
            choice
                .value(&Some(artisan_api::QuestionAnswer::Choices {
                    ids: (vec!["option-0".into()]).into()
                }))
                .unwrap(),
            Some(json!("blue-id"))
        );
        assert!(choice.value(&None).is_err());
        let number = ElicitationField {
            name: "count".into(),
            required: true,
            schema: json!({ "type": "integer", "minimum": 1, "maximum": 4 }),
        };
        assert!(
            number
                .value(&Some(artisan_api::QuestionAnswer::Text {
                    text: "nope".into()
                }))
                .is_err()
        );
        assert!(
            number
                .value(&Some(artisan_api::QuestionAnswer::Text {
                    text: "9".into()
                }))
                .is_err()
        );
        let boolean = ElicitationField {
            name: "flag".into(),
            required: true,
            schema: json!({ "type": "boolean" }),
        };
        assert!(
            boolean
                .value(&Some(artisan_api::QuestionAnswer::Text {
                    text: "maybe".into()
                }))
                .is_err()
        );
        assert_eq!(
            boolean
                .value(&Some(artisan_api::QuestionAnswer::Text {
                    text: "false".into()
                }))
                .unwrap(),
            Some(json!(false))
        );
    }
}

#[cfg(test)]
mod missing_tool_input_tests {
    use super::*;
    #[test]
    fn native_argv_keeps_argument_boundaries_and_quoted_whitespace() {
        let native = vec!["printf", "a  b", "$literal"];
        let tool = Tool {
            kind: Some("execute".into()),
            raw_input: json!({"command":native}),
            ..Default::default()
        };
        let ToolKind::Shell { command, .. } = tool_kind(&tool).unwrap() else {
            panic!("shell")
        };
        let artisan_api::TextContent::Inline(command) = command else {
            panic!("native argv must remain inline before persistence")
        };
        assert_eq!(shlex::split(&command).unwrap(), native);
    }

    #[test]
    fn completed_read_without_native_path_is_an_explicit_error() {
        let mut decoder = AcpDecoder::new(Profile::Cursor);
        decoder.merge_tool(
            "read",
            &json!({"kind":"read","title":"Read file","status":"completed"}),
        );
        let mut events = Vec::new();
        decoder.emit_tool("read", &mut events);
        assert!(events.iter().any(|event| matches!(
            event,
            EngineEvent::Item {
                kind: ItemKind::Error { .. },
                ..
            }
        )));
        assert!(!events.iter().any(|event| matches!(
            event,
            EngineEvent::Item {
                kind: ItemKind::Tool { .. },
                ..
            }
        )));
    }
}

#[cfg(test)]
mod native_question_identity_tests {
    use super::*;
    #[test]
    fn duplicate_native_ids_are_rejected_without_an_unanswerable_question() {
        for questions in [
            json!([{"id":"q","prompt":"Choose","options":[{"id":"same","label":"First"},{"id":"same","label":"Second"}]}]),
            json!([{"id":"q","prompt":"First","options":[]},{"id":"q","prompt":"Second","options":[]}]),
        ] {
            let mut decoder = AcpDecoder::new(Profile::Cursor);
            let mut events = Vec::new();
            decoder.cursor_question(json!(7), &json!({"questions":questions}), &mut events);
            assert!(events.is_empty());
            assert!(decoder.pending.is_empty());
            assert_eq!(decoder.take_outbox()[0]["error"]["code"], -32602);
        }
    }
}

#[cfg(test)]
mod malformed_frame_tests {
    use super::*;
    #[test]
    fn malformed_json_and_nonobject_frames_are_protocol_errors() {
        let mut decoder = AcpDecoder::new(Profile::Cursor);
        assert!(decoder.feed("{broken-json").is_err());
        assert!(decoder.feed("null").is_err());
        assert!(decoder.feed("[]").is_err());
        assert!(decoder.feed("{}").is_err());
    }
}

#[cfg(test)]
mod native_todo_merge_tests {
    use super::*;
    #[test]
    fn duplicate_display_text_uses_native_todo_identity_and_invalid_batch_is_atomic() {
        let mut decoder = AcpDecoder::new(Profile::Cursor);
        decoder.feed(&json!({"jsonrpc":"2.0","id":20,"method":"cursor/create_plan","params":{"plan":"# Native plan","todos":[
            {"id":"one","content":"Same","status":"pending"},{"id":"two","content":"Same","status":"pending"}
        ]}}).to_string()).unwrap();
        decoder.answer(
            PLAN_KEY,
            &Answer::Approval {
                decision: Decision::AllowOnce,
            },
        );
        decoder.take_outbox();
        decoder.feed(&json!({"jsonrpc":"2.0","id":21,"method":"cursor/update_todos","params":{"merge":true,"todos":[{"id":"two","content":"Same","status":"completed"}]}}).to_string()).unwrap();
        let plan = decoder.plan.as_ref().unwrap();
        assert_eq!(plan.steps.len(), 2);
        assert_eq!(plan.steps[0].status, StepStatus::Pending);
        assert_eq!(plan.steps[1].status, StepStatus::Done);
        decoder.take_outbox();
        decoder.feed(&json!({"jsonrpc":"2.0","id":22,"method":"cursor/update_todos","params":{"merge":true,"todos":[{"id":"two","content":"Renamed","status":"in_progress"}]}}).to_string()).unwrap();
        let before = decoder.plan.as_ref().unwrap().steps.clone();
        assert_eq!(before.len(), 2);
        assert_eq!(before[1].text, "Renamed".into());
        decoder.take_outbox();
        let errors=decoder.feed(&json!({"jsonrpc":"2.0","id":23,"method":"cursor/update_todos","params":{"merge":true,"todos":[
            {"id":"one","content":"Same","status":"completed"},{"id":"broken","status":"completed"}
        ]}}).to_string()).unwrap();
        assert_eq!(decoder.plan.as_ref().unwrap().steps, before);
        assert!(errors.iter().any(|event| matches!(
            event,
            EngineEvent::Item {
                kind: ItemKind::Error { .. },
                ..
            }
        )));
        let reply = decoder.take_outbox().pop().unwrap();
        assert_eq!(reply["id"], 23);
        assert_eq!(reply["error"]["code"], -32602);
        decoder.feed(&json!({"jsonrpc":"2.0","id":24,"method":"cursor/update_todos","params":{"merge":false,"todos":[]}}).to_string()).unwrap();
        assert!(decoder.plan.as_ref().unwrap().steps.is_empty());
        assert!(decoder.plan.as_ref().unwrap().todo_index.is_empty());
    }
    #[test]
    fn native_plan_rows_are_not_silently_filtered_or_assigned_unknown_status() {
        for invalid in [
            json!([{"status":"pending"}]),
            json!([{"content":"Task","status":"future"}]),
            json!([{"content":"Task","status":"pending","id":"same"},{"content":"Other","status":"pending","id":"same"}]),
        ] {
            assert!(plan_steps(&invalid, false).is_err());
        }
        assert!(plan_steps(&json!([{"content":"Task","status":"pending"}]), false).is_ok());
        assert!(plan_steps(&json!([{"content":"Task","status":"pending"}]), true).is_err());
    }
    #[test]
    fn malformed_permission_options_never_publish_partial_approval() {
        for options in [
            json!(null),
            json!([]),
            json!([{"optionId":"ok","kind":"allow_once"}, {"kind":"reject_once"}]),
            json!([{"optionId":"ok","kind":"unknown"}]),
            json!([{"optionId":"","kind":"allow_once"}]),
            json!([{"optionId":"same","kind":"allow_once"},{"optionId":"same","kind":"reject_once"}]),
        ] {
            let mut decoder = AcpDecoder::new(Profile::Cursor);
            let mut events = Vec::new();
            decoder.permission(json!(7),&json!({"toolCall":{"toolCallId":"tool","kind":"execute","title":"echo ok"},"options":options}),&mut events);
            assert!(decoder.pending.is_empty());
            assert!(decoder.tools.is_empty());
            assert_eq!(decoder.take_outbox()[0]["error"]["code"], json!(-32602));
            assert!(matches!(
                &events[..],
                [EngineEvent::Item {
                    status: Status::Failed,
                    kind: ItemKind::Error { .. },
                    ..
                }]
            ));
        }
    }

    #[test]
    fn permission_options_preserve_native_ids_and_decisions() {
        let options=permission_options(&json!([{"optionId":"native-yes","kind":"allow_once"},{"optionId":"native-no","kind":"reject_always"}])).unwrap();
        assert_eq!(
            options,
            vec![
                ("native-yes".into(), Decision::AllowOnce),
                ("native-no".into(), Decision::DenyAlways)
            ]
        );
    }
    #[test]
    fn simultaneous_permission_callbacks_preserve_one_tool_and_both_native_replies() {
        let mut decoder = AcpDecoder::new(Profile::Cursor);
        let mut events = Vec::new();
        let params = json!({"toolCall":{"toolCallId":"tool","kind":"execute","title":"echo ok"},"options":[{"optionId":"yes","kind":"allow_once"},{"optionId":"no","kind":"reject_once"}]});
        decoder.permission(json!(1), &params, &mut events);
        decoder.permission(json!(2), &params, &mut events);
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(
                    event,
                    EngineEvent::Item {
                        kind: ItemKind::Tool { .. },
                        ..
                    }
                ))
                .count(),
            1
        );
        let keys = events
            .iter()
            .filter_map(|event| {
                if let EngineEvent::Approval {
                    key,
                    target: Some(target),
                    status: Status::AwaitingUser,
                    ..
                } = event
                {
                    assert_eq!(target, "tool");
                    Some(key.clone())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        assert_eq!(keys.len(), 2);
        assert_ne!(keys[0], keys[1]);
        assert!(
            decoder
                .answer(
                    &keys[0],
                    &Answer::Approval {
                        decision: Decision::AllowAlways
                    }
                )
                .is_empty()
        );
        assert_eq!(decoder.pending.len(), 2);
        let second = decoder.answer(
            &keys[1],
            &Answer::Approval {
                decision: Decision::DenyOnce,
            },
        );
        assert!(matches!(
            &second[..],
            [EngineEvent::Approval {
                status: Status::Declined,
                ..
            }]
        ));
        let first = decoder.answer(
            &keys[0],
            &Answer::Approval {
                decision: Decision::AllowOnce,
            },
        );
        assert!(matches!(
            &first[..],
            [EngineEvent::Approval {
                status: Status::Done,
                ..
            }]
        ));
        let replies = decoder.take_outbox();
        assert_eq!(replies.len(), 2);
        assert_eq!(replies[0]["id"], 2);
        assert_eq!(replies[0]["result"]["outcome"]["optionId"], "no");
        assert_eq!(replies[1]["id"], 1);
        assert_eq!(replies[1]["result"]["outcome"]["optionId"], "yes");
        assert!(
            decoder
                .answer(
                    &keys[0],
                    &Answer::Approval {
                        decision: Decision::DenyOnce
                    }
                )
                .is_empty()
        );
        assert!(decoder.take_outbox().is_empty());
        decoder.permission(json!(3), &params, &mut events);
        decoder.permission(json!(4), &params, &mut events);
        let cancelled = decoder.cancel_pending();
        assert_eq!(
            cancelled
                .iter()
                .filter(|event| matches!(
                    event,
                    EngineEvent::Approval {
                        status: Status::Cancelled,
                        ..
                    }
                ))
                .count(),
            2
        );
        assert_eq!(decoder.take_outbox().len(), 2);
        assert!(decoder.pending.is_empty());
        assert!(
            decoder
                .answer(
                    "permission-request:3",
                    &Answer::Approval {
                        decision: Decision::AllowOnce
                    }
                )
                .is_empty()
        );
        assert!(decoder.take_outbox().is_empty());
        assert!(
            !decoder.tools["tool"]
                .status
                .is_some_and(Status::is_terminal)
        );
    }
}

#[cfg(test)]
mod editable_terminal_plan_tests {
    use super::*;
    #[test]
    fn completed_unicode_steps_release_source_but_keep_exact_late_native_delta_identity() {
        let mut decoder = AcpDecoder::new(Profile::Cursor);
        let large = "界🙂 original\n".repeat(2048);
        let mut todos = (0..5000).map(|id| json!({"id":format!("todo-{id}"),"content":format!("Step {id}"),"status":"completed"})).collect::<Vec<_>>();
        todos[4999]["content"] = json!(large);
        let events = decoder.feed(&json!({"jsonrpc":"2.0","id":1,"method":"cursor/update_todos","params":{"todos":todos}}).to_string()).unwrap();
        let complete = events
            .iter()
            .find_map(|event| match event {
                EngineEvent::Item {
                    kind: ItemKind::Plan { steps, .. },
                    ..
                } => steps.inline(),
                _ => None,
            })
            .unwrap();
        assert_eq!(complete.len(), 5000);
        assert_eq!(complete[4999].text.inline(), Some(large.as_str()));
        let source = artisan_api::ContentRef::from_text(&large);
        assert_eq!(
            decoder.plan.as_ref().unwrap().steps[4999].text,
            artisan_api::TextContent::Stored(source.clone())
        );
        decoder.feed(&json!({"jsonrpc":"2.0","id":2,"method":"cursor/update_todos","params":{"merge":true,"todos":[{"id":"todo-1","content":"Changed native step","status":"in_progress"}]}}).to_string()).unwrap();
        let plan = decoder.plan.as_ref().unwrap();
        assert_eq!(plan.steps.len(), 5000);
        assert_eq!(plan.steps[1].text.inline(), Some("Changed native step"));
        assert_eq!(plan.steps[1].status, StepStatus::InProgress);
        assert_eq!(
            plan.steps[4999].text,
            artisan_api::TextContent::Stored(source)
        );
        assert_eq!(plan.todo_index["todo-4999"], 4999);
    }
}

#[cfg(test)]
mod retained_agent_tests {
    use super::*;

    #[test]
    fn settled_agent_releases_native_body_and_keeps_partial_and_corrected_results() {
        let mut decoder = AcpDecoder::new(Profile::Cursor);
        let prompt = "task 界🙂 ".repeat(8192);
        let result = "report 😀\n".repeat(8192);
        decoder.merge_tool("agent", &json!({"rawInput":{"_toolName":"task", "prompt":prompt, "description":"Inspect", "subagentType":"reviewer"}, "rawOutput":result, "status":"completed"}));
        let mut events = Vec::new();
        decoder.emit_tool("agent", &mut events);
        assert!(events.iter().any(|event| matches!(event, EngineEvent::Item { kind: ItemKind::Agent { prompt: Some(p), result: Some(r), .. }, .. } if p.inline() == Some(prompt.as_str()) && r.inline() == Some(result.as_str()))));
        let tool = &decoder.tools["agent"];
        assert_eq!(tool.raw_input, json!({"_toolName":"task"}));
        assert!(tool.text.is_empty());
        let Some(ItemKind::Agent {
            prompt: Some(p),
            result: Some(r),
            ..
        }) = &tool.retained_agent
        else {
            panic!("retained canonical agent");
        };
        assert_eq!(
            p,
            &artisan_api::TextContent::Stored(artisan_api::ContentRef::from_text(&prompt))
        );
        assert_eq!(
            r,
            &artisan_api::TextContent::Stored(artisan_api::ContentRef::from_text(&result))
        );
        decoder.merge_tool("agent", &json!({"status":"completed"}));
        events.clear();
        decoder.emit_tool("agent", &mut events);
        assert!(events.iter().any(|event| matches!(event, EngineEvent::Item { kind: ItemKind::Agent { prompt: Some(p), result: Some(r), .. }, .. } if p == &artisan_api::TextContent::Stored(artisan_api::ContentRef::from_text(&prompt)) && r == &artisan_api::TextContent::Stored(artisan_api::ContentRef::from_text(&result)))));
        decoder.merge_tool(
            "agent",
            &json!({"rawOutput":"corrected report", "status":"completed"}),
        );
        events.clear();
        decoder.emit_tool("agent", &mut events);
        assert!(events.iter().any(|event| matches!(event, EngineEvent::Item { kind: ItemKind::Agent { result: Some(r), .. }, .. } if r.inline() == Some("corrected report"))));
        assert!(decoder.tools["agent"].text.is_empty());
    }
}
