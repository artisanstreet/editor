//! Pure mapping from Claude's stream-json lines onto items.

use super::tools::{questions, todo_steps, tool_kind, validate_input};
use crate::{EngineEvent, LiveKind, Outcome, Retry};
use anyhow::Context as _;
use artisan_api::{
    Answer, Approval, ApprovalRequest, Decision, ErrorKind, ItemId, ItemKind, MessageDirection,
    NoticeKind, Output, Phase, RunStatus, Status, Timestamp, ToolKind, Usage,
};
use serde_json::{Map, Value, json};
use std::collections::{HashMap, HashSet, VecDeque};

#[derive(serde::Deserialize)]
struct IndexedBlock {
    index: u64,
}

const TITLE_REQUEST_ID: &str = "artisan-title";

/// Decodes one Claude process's output. Holds just enough to settle items
/// and answer the CLI; `feed` never does I/O.
#[derive(Default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent facts about the session, not one state in disguise"
)]
pub struct ClaudeDecoder {
    reasoning_summary: crate::reasoning::Summary,
    session: Option<String>,
    model: Option<String>,
    /// Messages announced by `message_start`; they settle on `message_delta`.
    streamed: HashSet<String>,
    current: Option<String>,
    messages: HashMap<String, Message>,
    tools: HashMap<String, Tool>,
    /// Open `can_use_tool` requests by item key.
    pending: HashMap<String, Pending>,
    replies: Vec<Value>,
    /// User lines written and not yet echoed; `Some` for steers.
    replays: VecDeque<Option<ItemId>>,
    title_pending: bool,
    titled: bool,
    usage: Usage,
    refused: bool,
    /// The usage limit the CLI said it hit, from a rejected
    /// `rate_limit_event` until the turn's `result`.
    limit_reached: Option<LimitReached>,
    result_seen: bool,
    /// The main session is mid-turn: its own frames arrived since the last
    /// `result`. Subagent frames don't count.
    in_turn: bool,
    /// Background agents and workflows the CLI reports live, and the
    /// background work of agents (an agent waiting on its own shell is
    /// resumed when it ends): they report back into the session, so it must
    /// keep taking input.
    background: usize,
    /// Agent tasks with background work of their own still live.
    waiting_agents: HashSet<String>,
    /// Agent tool calls by the task id the CLI gives them.
    agent_tasks: HashMap<String, String>,
    /// Every task's tool call, for a task moved to the background later.
    task_tools: HashMap<String, String>,
    outcome: Option<Outcome>,
    compacting: Option<String>,
    counter: u32,
}

#[derive(Default)]
struct Message {
    parent: Option<String>,
    blocks: u64,
    /// Text parts not yet settled: key and text.
    texts: Vec<(String, String)>,
}

struct Tool {
    key: String,
    parent: Option<String>,
    kind: ItemKind,
    status: Status,
    /// Runs on after its `tool_result`; a task notification settles it.
    background: bool,
}

struct Pending {
    request_id: String,
    input: Value,
    suggestions: Vec<Value>,
}

impl ClaudeDecoder {
    /// Maps one stdout line. Replies for the CLI collect in [`Self::take_replies`].
    pub fn feed(&mut self, line: &str) -> anyhow::Result<Vec<EngineEvent>> {
        let frame: Value = serde_json::from_str(line).context("decoding Claude protocol JSON")?;
        anyhow::ensure!(frame.is_object(), "Claude protocol frame must be an object");
        anyhow::ensure!(
            frame["type"].is_string(),
            "Claude protocol frame is missing its type"
        );
        if str_at(&frame, "type") == Some("assistant") {
            anyhow::ensure!(
                str_at(&frame["message"], "id")
                    .or_else(|| str_at(&frame, "uuid"))
                    .is_some_and(|id| !id.is_empty()),
                "Claude assistant frame is missing its message id"
            );
        }
        let events = self.feed_raw(&frame);
        let events = self.reasoning_summary.normalize(events, true);
        Ok(crate::retained::settle(self, events))
    }

    fn feed_raw(&mut self, frame: &Value) -> Vec<EngineEvent> {
        let mut out = Vec::new();
        let parent = str_at(frame, "parent_tool_use_id").map(str::to_owned);
        if parent.is_none()
            && matches!(
                str_at(frame, "type"),
                Some("assistant" | "user" | "stream_event")
            )
        {
            self.in_turn = true;
        }
        match str_at(frame, "type").unwrap_or_default() {
            "system" => self.system(frame, &mut out),
            "stream_event" if parent.is_none() => self.stream_event(&frame["event"], &mut out),
            "assistant" => self.assistant(frame, parent, &mut out),
            "user" => self.user(frame, parent, &mut out),
            "result" => self.result(frame, &mut out),
            "control_request" => self.control_request(frame, &mut out),
            "control_response" => self.control_response(frame, &mut out),
            "rate_limit_event" => self.rate_limit(&frame["rate_limit_info"]),
            _ => tracing::debug!(
                kind = frame["type"].as_str(),
                "ignored unsupported Claude frame"
            ),
        }
        out
    }

    /// Answers the approval or question on `key`. The reply for the CLI goes
    /// to [`Self::take_replies`].
    pub fn answer(&mut self, key: &str, answer: &Answer) -> Vec<EngineEvent> {
        let events = self.answer_raw(key, answer);
        crate::retained::settle(self, events)
    }

    fn answer_raw(&mut self, key: &str, answer: &Answer) -> Vec<EngineEvent> {
        let native = if let Answer::Question { answers } = answer {
            let Some(tool) = self.tools.get(key) else {
                return Vec::new();
            };
            let ItemKind::Question { questions, .. } = &tool.kind else {
                return Vec::new();
            };
            match crate::question::native_collection_answers(questions, answers) {
                Ok(native) => native,
                Err(message) => return vec![crate::question::invalid_answer(key, message)],
            }
        } else {
            Vec::new()
        };
        let (Some(pending), Some(tool)) = (self.pending.remove(key), self.tools.get_mut(key))
        else {
            tracing::debug!(key, "answer for an item Claude is not waiting on");
            return Vec::new();
        };
        let deny = |message: &str, interrupt: bool| json!({ "behavior": "deny", "message": message, "interrupt": interrupt });
        let (response, status) = match (answer, &mut tool.kind) {
            (
                Answer::Question { answers },
                ItemKind::Question {
                    questions,
                    answers: given,
                    ..
                },
            ) => {
                let map = answer_map(questions, &native);
                *given = Some(answers.clone().into());
                if map.is_empty() {
                    (
                        deny("The user skipped the question.", false),
                        Status::Declined,
                    )
                } else {
                    let mut input = pending.input;
                    if let Value::Object(fields) = &mut input {
                        fields.insert("answers".to_owned(), Value::Object(map));
                    }
                    (
                        json!({ "behavior": "allow", "updatedInput": input }),
                        Status::Running,
                    )
                }
            }
            (Answer::Approval { decision }, kind) => {
                if let ItemKind::Tool {
                    approval: Some(approval),
                    ..
                }
                | ItemKind::Plan {
                    approval: Some(approval),
                    ..
                } = kind
                {
                    approval.decision = Some(*decision);
                }
                match decision {
                    Decision::AllowOnce | Decision::AllowAlways => {
                        let mut allow =
                            json!({ "behavior": "allow", "updatedInput": pending.input });
                        if *decision == Decision::AllowAlways {
                            // The CLI suggests rules for its settings files;
                            // Artisan keeps them for this session only.
                            let rules: Vec<Value> = pending
                                .suggestions
                                .iter()
                                .filter(|rule| str_at(rule, "type") == Some("addRules"))
                                .map(|rule| {
                                    let mut rule = rule.clone();
                                    rule["destination"] = json!("session");
                                    rule
                                })
                                .collect();
                            allow["updatedPermissions"] = Value::Array(rules);
                        }
                        (allow, Status::Running)
                    }
                    Decision::DenyOnce | Decision::DenyAlways => (
                        deny("The user denied this request.", false),
                        Status::Declined,
                    ),
                    Decision::Cancelled => (deny("The user cancelled.", true), Status::Cancelled),
                }
            }
            (Answer::Question { .. }, _) => (
                deny("The user denied this request.", false),
                Status::Declined,
            ),
        };
        self.replies.push(json!({
            "type": "control_response",
            "response": { "subtype": "success", "request_id": pending.request_id, "response": response },
        }));
        tool.status = status;
        vec![tool.event()]
    }

    /// Lines to write to the CLI's stdin.
    pub fn take_replies(&mut self) -> Vec<Value> {
        std::mem::take(&mut self.replies)
    }

    /// A user line was written; `Some` marks a steer to acknowledge.
    pub fn expect_replay(&mut self, steer: Option<ItemId>) {
        self.replays.push_back(steer);
    }

    /// Asks Claude to name a fresh session after `text`; the answer comes
    /// back as a `control_response` and becomes `Title`.
    pub fn title_request(&mut self, text: &str) -> Value {
        self.title_pending = true;
        json!({
            "type": "control_request",
            "request_id": TITLE_REQUEST_ID,
            "request": { "subtype": "generate_session_title", "description": text, "persist": true },
        })
    }

    /// Something written to the CLI still awaits its answer.
    #[must_use]
    pub fn awaiting_input(&self) -> bool {
        !self.replays.is_empty() || self.title_pending
    }

    /// Whether a `result` arrived since the last call.
    /// Whether background agents or workflows are still running.
    #[must_use]
    pub const fn background_running(&self) -> bool {
        self.background > 0
    }

    /// What the decoder believes runs in the background, for the trace.
    #[must_use]
    pub fn background_summary(&self) -> String {
        format!(
            "{} background task(s); agents waiting on their own work: {:?}",
            self.background, self.waiting_agents
        )
    }

    /// Whether the main session is mid-turn.
    #[must_use]
    pub const fn in_turn(&self) -> bool {
        self.in_turn
    }

    pub fn take_result_seen(&mut self) -> bool {
        std::mem::take(&mut self.result_seen)
    }

    #[must_use]
    pub fn session(&self) -> Option<&str> {
        self.session.as_deref()
    }

    pub const fn finish_title(&mut self) {
        self.title_pending = false;
    }

    #[must_use]
    pub const fn titled(&self) -> bool {
        self.titled
    }

    /// How the last turn ended, once a `result` arrived.
    pub const fn take_outcome(&mut self) -> Option<Outcome> {
        self.outcome.take()
    }

    /// The process is gone: whatever is still open was cut off.
    pub fn finish(&mut self) -> Vec<EngineEvent> {
        let mut out = Vec::new();
        self.settle_open_texts(Status::Cancelled, &mut out);
        let mut open: Vec<&mut Tool> = self
            .tools
            .values_mut()
            .filter(|tool| !tool.status.is_terminal())
            .collect();
        open.sort_by(|left, right| left.key.cmp(&right.key));
        for tool in open {
            tool.status = Status::Cancelled;
            out.push(EngineEvent::Status {
                key: tool.key.clone(),
                status: Status::Cancelled,
            });
        }
        if let Some(key) = self.compacting.take() {
            out.push(EngineEvent::Status {
                key,
                status: Status::Cancelled,
            });
        }
        self.pending.clear();
        self.replays.clear();
        self.title_pending = false;
        self.current = None;
        out
    }

    fn system(&mut self, frame: &Value, out: &mut Vec<EngineEvent>) {
        match str_at(frame, "subtype").unwrap_or_default() {
            "init" => {
                if let Some(id) = str_at(frame, "session_id")
                    && self.session.as_deref() != Some(id)
                {
                    self.session = Some(id.to_owned());
                    out.push(EngineEvent::Session { id: id.to_owned() });
                }
                if let Some(model) = str_at(frame, "model")
                    && self.model.as_deref() != Some(model)
                {
                    self.model = Some(model.to_owned());
                    out.push(EngineEvent::Model {
                        id: model.to_owned(),
                    });
                }
            }
            // Live background tasks. The CLI waits for agents and workflows
            // (they report back); shells it reaps once input closes, and
            // ambient watchers are never work.
            "background_tasks_changed" => {
                let tasks = frame["tasks"].as_array().map_or(&[][..], Vec::as_slice);
                self.background = tasks
                    .iter()
                    .filter(|task| {
                        (matches!(
                            str_at(task, "task_type"),
                            Some("local_agent" | "local_workflow")
                        ) || task.get("parent_task_id").is_some())
                            && task["ambient"].as_bool() != Some(true)
                    })
                    .count();
                self.waiting_agents = tasks
                    .iter()
                    .filter_map(|task| str_at(task, "parent_task_id").map(str::to_owned))
                    .collect();
            }
            // Claude repeats `compacting` as progress (with a growing
            // `compact_output_chars`) and ends it with a null status that
            // carries `compact_result` (2.1.296): one compaction, one item.
            "status" if str_at(frame, "status") == Some("compacting") => {
                if self.compacting.is_none() {
                    let key = self.next_key("compaction");
                    out.push(compaction(&key, Status::Running, None, None));
                    self.compacting = Some(key);
                }
            }
            "status" => {
                let result = str_at(frame, "compact_result");
                if result.is_some_and(|result| result != "success")
                    && let Some(key) = self.compacting.take()
                {
                    out.push(compaction(&key, Status::Failed, None, None));
                }
            }
            "compact_boundary" => {
                let metadata = &frame["compact_metadata"];
                let key = self
                    .compacting
                    .take()
                    .unwrap_or_else(|| self.next_key("compaction"));
                out.push(compaction(
                    &key,
                    Status::Done,
                    metadata["pre_tokens"].as_u64(),
                    metadata["post_tokens"].as_u64(),
                ));
            }
            "api_retry" => {
                let attempt = frame["attempt"].as_u64().unwrap_or(1);
                out.push(EngineEvent::Live {
                    target: LiveKind::Status,
                    text: format!("Retrying (attempt {attempt})"),
                    append: false,
                });
            }
            "task_started" => self.task_started(frame, out),
            // A foreground command past its timeout moves to the background
            // just before its tool result says so.
            "task_updated" if frame["patch"]["is_backgrounded"].as_bool() == Some(true) => {
                if let Some(tool) = str_at(frame, "task_id")
                    .and_then(|task| self.task_tools.get(task))
                    .cloned()
                {
                    self.backgrounded(&tool, out);
                }
            }
            "task_notification" => self.task_notification(frame, out),
            _ => {}
        }
    }

    /// A background task started: an agent's id, a backgrounded tool.
    fn task_started(&mut self, frame: &Value, out: &mut Vec<EngineEvent>) {
        if let (Some(task), Some(tool)) = (str_at(frame, "task_id"), str_at(frame, "tool_use_id")) {
            self.task_tools.insert(task.to_owned(), tool.to_owned());
        }
        if let (Some(task), Some(tool)) = (str_at(frame, "task_id"), str_at(frame, "tool_use_id"))
            && str_at(frame, "task_type") == Some("local_agent")
        {
            self.agent_tasks.insert(task.to_owned(), tool.to_owned());
            // The id its `SendMessage`s address it by.
            if let Some(known) = self.tools.get_mut(tool)
                && let ItemKind::Agent {
                    agent_id: slot @ None,
                    ..
                } = &mut known.kind
            {
                *slot = Some(task.to_owned());
                out.push(known.event());
            }
            // An agent from an earlier process of this session,
            // resumed: adopt it so its report reaches its item
            // (Forge finds that by the tool id).
            if !self.tools.contains_key(tool) {
                let tool = Tool {
                    key: tool.to_owned(),
                    parent: None,
                    kind: ItemKind::Agent {
                        engine: None,
                        thread: None,
                        name: str_at(frame, "subagent_type").map(str::to_owned),
                        description: (str_at(frame, "description").unwrap_or_default().to_owned())
                            .into(),
                        prompt: None,
                        agent_id: Some(task.to_owned()),
                        context_used: None,
                        model: None,
                        background: true,
                        result: None,
                        label: None,
                    },
                    status: Status::Running,
                    background: true,
                };
                out.push(tool.event());
                self.tools.insert(tool.key.clone(), tool);
            }
        }
        if frame["is_backgrounded"].as_bool() == Some(true)
            && let Some(tool) = str_at(frame, "tool_use_id")
        {
            self.backgrounded(tool, out);
        }
    }

    /// The tool runs on in the background: its result only says so, and its
    /// task notification ends it.
    fn backgrounded(&mut self, tool: &str, out: &mut Vec<EngineEvent>) {
        if let Some(tool) = self.tools.get_mut(tool)
            && !tool.background
        {
            tool.background = true;
            if let ItemKind::Agent { background, .. }
            | ItemKind::Tool {
                tool: ToolKind::Shell { background, .. },
                ..
            } = &mut tool.kind
            {
                *background = true;
            }
            out.push(tool.event());
        }
    }

    /// A background task stopped. For an agent this is its report, unless
    /// it only paused to wait on background work of its own.
    fn task_notification(&mut self, frame: &Value, out: &mut Vec<EngineEvent>) {
        // An agent that ended its turn to wait on its own background
        // work is resumed when that work ends: its "completed"
        // summary is a progress line, not its report.
        if str_at(frame, "task_id").is_some_and(|task| {
            self.agent_tasks.contains_key(task) && self.waiting_agents.contains(task)
        }) {
            return;
        }
        let Some(tool) = str_at(frame, "tool_use_id").and_then(|id| self.tools.get_mut(id)) else {
            return;
        };
        if !tool.background {
            return;
        }
        let status = match str_at(frame, "status") {
            Some("completed") => Status::Done,
            Some("failed") => Status::Failed,
            _ => Status::Cancelled,
        };
        match &mut tool.kind {
            // An agent notifies each time it stops, and a resumed
            // one reports again: the latest report is its result.
            // Preserve the report verbatim, even when it repeats the task.
            ItemKind::Agent { result, .. } => {
                let summary = str_at(frame, "summary").filter(|_| status == Status::Done);
                if let Some(summary) = summary {
                    *result = Some(summary.into());
                } else if tool.status.is_terminal() {
                    return;
                }
            }
            _ if tool.status.is_terminal() => return,
            ItemKind::UserMessage { .. }
            | ItemKind::AssistantMessage { .. }
            | ItemKind::Tool { .. }
            | ItemKind::Question { .. }
            | ItemKind::Plan { .. }
            | ItemKind::Image { .. }
            | ItemKind::Compaction { .. }
            | ItemKind::Notice { .. }
            | ItemKind::Error { .. } => {}
        }
        tool.status = status;
        out.push(tool.event());
    }

    /// A subagent's message carries its own usage: its agent's context and
    /// model follow it, for the context meter in the agent's view.
    fn agent_context(&mut self, parent: &str, message: &Value, out: &mut Vec<EngineEvent>) {
        let usage = &message["usage"];
        let Some(input) = usage["input_tokens"].as_u64() else {
            return;
        };
        let used = input
            + usage["cache_creation_input_tokens"]
                .as_u64()
                .unwrap_or_default()
            + usage["cache_read_input_tokens"]
                .as_u64()
                .unwrap_or_default();
        let Some(tool) = self.tools.get_mut(parent) else {
            return;
        };
        if let ItemKind::Agent {
            context_used,
            model,
            ..
        } = &mut tool.kind
        {
            let seen = str_at(message, "model").map(str::to_owned);
            if *context_used == Some(used) && (seen.is_none() || *model == seen) {
                return;
            }
            *context_used = Some(used);
            if seen.is_some() {
                *model = seen;
            }
            out.push(tool.event());
        }
    }

    /// A content block opens.
    fn block_start(&self, event: &Value, out: &mut Vec<EngineEvent>) {
        let Some(id) = self.current.clone() else {
            return;
        };
        let Ok(IndexedBlock { index }) = <IndexedBlock as serde::Deserialize>::deserialize(event)
        else {
            tracing::warn!(session = ?self.session, message = %id, "Claude content block omitted a valid index");
            return;
        };
        match str_at(&event["content_block"], "type") {
            Some("text") => out.push(EngineEvent::Item {
                key: format!("{id}:{index}"),
                parent: None,
                status: Status::Running,
                kind: ItemKind::AssistantMessage {
                    text: (String::new()).into(),
                    phase: Phase::Commentary,
                },
            }),
            Some("thinking") => out.push(EngineEvent::Live {
                target: LiveKind::Reasoning,
                text: String::new(),
                append: false,
            }),
            _ => {}
        }
    }

    /// A content block grows.
    fn block_delta(&self, event: &Value, out: &mut Vec<EngineEvent>) {
        let Some(id) = self.current.as_deref() else {
            return;
        };
        let Ok(IndexedBlock { index }) = <IndexedBlock as serde::Deserialize>::deserialize(event)
        else {
            tracing::warn!(session = ?self.session, message = %id, "Claude content delta omitted a valid index");
            return;
        };
        let delta = &event["delta"];
        let (target, text) = match str_at(delta, "type") {
            Some("text_delta") => (
                LiveKind::Text {
                    key: format!("{id}:{index}"),
                },
                str_at(delta, "text"),
            ),
            Some("thinking_delta") => (LiveKind::Reasoning, str_at(delta, "thinking")),
            _ => return,
        };
        if let Some(text) = text.filter(|text| !text.is_empty()) {
            out.push(EngineEvent::Live {
                target,
                text: text.to_owned(),
                append: true,
            });
        }
    }

    fn stream_event(&mut self, event: &Value, out: &mut Vec<EngineEvent>) {
        match str_at(event, "type").unwrap_or_default() {
            "message_start" => {
                if let Some(id) = str_at(&event["message"], "id") {
                    self.streamed.insert(id.to_owned());
                    self.current = Some(id.to_owned());
                }
            }
            "content_block_start" => self.block_start(event, out),
            "content_block_delta" => self.block_delta(event, out),
            "message_delta" => {
                let Some(id) = self.current.clone() else {
                    return;
                };
                let stop = str_at(&event["delta"], "stop_reason");
                self.settle_message(&id, stop == Some("end_turn"), out);
                if stop == Some("refusal") {
                    self.refused = true;
                    let key = self.next_key("error");
                    out.push(error_item(
                        key,
                        ErrorKind::Refusal,
                        "Claude declined this request.".to_owned(),
                        false,
                    ));
                }
                let usage = &event["usage"];
                if let Some(input) = usage["input_tokens"].as_u64() {
                    self.usage.context_used = Some(
                        input
                            + usage["cache_creation_input_tokens"]
                                .as_u64()
                                .unwrap_or_default()
                            + usage["cache_read_input_tokens"]
                                .as_u64()
                                .unwrap_or_default(),
                    );
                    out.push(EngineEvent::Usage(self.usage.clone()));
                }
            }
            _ => {}
        }
    }

    fn assistant(&mut self, frame: &Value, parent: Option<String>, out: &mut Vec<EngineEvent>) {
        let message = &frame["message"];
        // The CLI's own reply to `/goal` is the goal, for the goal strip; it
        // isn't the model talking.
        if frame["local_command_run"]["command"] == "goal" {
            let text = message["content"]
                .as_array()
                .into_iter()
                .flatten()
                .find_map(|block| str_at(block, "text"))
                .or_else(|| str_at(message, "content"))
                .unwrap_or_default()
                .trim();
            let goal = [("Goal set:", "active"), ("Goal cleared:", "cleared")]
                .into_iter()
                .find_map(|(prefix, state)| Some((state, text.strip_prefix(prefix)?.trim())));
            // A status line or usage reply changes nothing.
            if let Some((state, objective)) = goal {
                let key = parent
                    .as_ref()
                    .map_or_else(|| "goal".to_owned(), |parent| format!("goal:{parent}"));
                out.push(EngineEvent::Item {
                    key,
                    parent,
                    status: Status::Done,
                    kind: ItemKind::Notice {
                        kind: NoticeKind::Goal {
                            state: state.to_owned(),
                        },
                        text: objective.into(),
                    },
                });
            }
            return;
        }
        let Some(id) = str_at(message, "id")
            .or_else(|| str_at(frame, "uuid"))
            .map(str::to_owned)
        else {
            return;
        };
        let streamed = self.streamed.contains(&id);
        if let Some(parent) = &parent {
            self.agent_context(parent, message, out);
        }
        for block in message["content"].as_array().into_iter().flatten() {
            let entry = self.messages.entry(id.clone()).or_insert_with(|| Message {
                parent: parent.clone(),
                ..Message::default()
            });
            let index = entry.blocks;
            entry.blocks += 1;
            match str_at(block, "type") {
                Some("text") => {
                    let text = str_at(block, "text").unwrap_or_default();
                    if text.trim().is_empty() {
                        continue;
                    }
                    let key = format!("{id}:{index}");
                    entry.texts.push((key.clone(), text.to_owned()));
                    if !streamed {
                        out.push(EngineEvent::Item {
                            key: key.clone(),
                            parent: parent.clone(),
                            status: Status::Running,
                            kind: ItemKind::AssistantMessage {
                                text: (String::new()).into(),
                                phase: Phase::Commentary,
                            },
                        });
                    }
                    out.push(EngineEvent::Live {
                        target: LiveKind::Text { key },
                        text: text.to_owned(),
                        append: false,
                    });
                }
                Some("thinking") if !streamed && parent.is_none() => {
                    if let Some(text) = str_at(block, "thinking").filter(|text| !text.is_empty()) {
                        out.push(EngineEvent::Live {
                            target: LiveKind::Reasoning,
                            text: text.to_owned(),
                            append: false,
                        });
                    }
                }
                Some("tool_use") => {
                    let (Some(tool_id), Some(name)) = (str_at(block, "id"), str_at(block, "name"))
                    else {
                        continue;
                    };
                    self.tool_use(tool_id, name, &block["input"], parent.clone(), out);
                }
                _ => {}
            }
        }
        // Without a stream (subagent frames) nothing else will settle it.
        if !streamed {
            self.settle_message(&id, str_at(message, "stop_reason") == Some("end_turn"), out);
        }
    }

    fn tool_use(
        &mut self,
        id: &str,
        name: &str,
        input: &Value,
        parent: Option<String>,
        out: &mut Vec<EngineEvent>,
    ) {
        if self.tools.contains_key(id) {
            return;
        }
        if let Err(message) = validate_input(name, input) {
            out.push(error_item(
                format!("invalid-tool:{id}"),
                ErrorKind::Execution,
                message,
                false,
            ));
            return;
        }
        let text = |field: &str| str_at(input, field).map(str::to_owned);
        let (key, kind, status) = match name {
            "Task" | "Agent" => (
                id.to_owned(),
                ItemKind::Agent {
                    engine: None,
                    thread: None,
                    name: text("subagent_type"),
                    description: (text("description").unwrap_or_default()).into(),
                    prompt: (text("prompt")).map(Into::into),
                    agent_id: None,
                    context_used: None,
                    model: None,
                    background: input["run_in_background"].as_bool() == Some(true),
                    result: None,
                    label: None,
                },
                Status::Running,
            ),
            // One todo list per run, replaced in place.
            "TodoWrite" => (
                parent
                    .as_ref()
                    .map_or_else(|| "plan".to_owned(), |parent| format!("plan:{parent}")),
                ItemKind::Plan {
                    steps: (todo_steps(input)).into(),
                    proposal: None,
                    approval: None,
                },
                Status::Done,
            ),
            "ExitPlanMode" => (
                id.to_owned(),
                ItemKind::Plan {
                    steps: (Vec::new()).into(),
                    proposal: (text("plan")).map(Into::into),
                    approval: None,
                },
                Status::Running,
            ),
            "AskUserQuestion" => (
                id.to_owned(),
                ItemKind::Question {
                    blocking: true,
                    questions: (questions(input)).into(),
                    answers: None,
                    answer_text: None,
                },
                Status::Running,
            ),
            _ => (
                id.to_owned(),
                match tool_kind(name, input) {
                    Ok(tool) => ItemKind::Tool {
                        tool,
                        title: text("description"),
                        output: None,
                        exit_code: None,
                        approval: None,
                    },
                    Err(message) => ItemKind::Error {
                        kind: ErrorKind::Execution,
                        message: message.into(),
                        retryable: false,
                    },
                },
                Status::Running,
            ),
        };
        // A watcher and a workflow always run in the background.
        let background = input["run_in_background"].as_bool() == Some(true)
            || matches!(name, "Monitor" | "Workflow");
        let tool = Tool {
            key,
            parent,
            kind,
            status,
            background,
        };
        out.push(tool.event());
        self.tools.insert(id.to_owned(), tool);
    }

    fn user(&mut self, frame: &Value, parent: Option<String>, out: &mut Vec<EngineEvent>) {
        // Claude replays what others put in its turn too: a message from one
        // of its agents or another session, or a task notification. Only
        // the rest are our own inputs.
        match str_at(&frame["origin"], "kind") {
            Some("peer") => return self.peer_message(frame, parent, out),
            Some(kind) if kind != "user" => return,
            _ => {}
        }
        if frame["isReplay"].as_bool() == Some(true) {
            if let Some(Some(id)) = self.replays.pop_front() {
                out.push(EngineEvent::InputAccepted { id });
            }
            return;
        }
        for block in frame["message"]["content"].as_array().into_iter().flatten() {
            if str_at(block, "type") != Some("tool_result") {
                continue;
            }
            let Some(tool) = str_at(block, "tool_use_id").and_then(|id| self.tools.get_mut(id))
            else {
                continue;
            };
            if tool.status.is_terminal() {
                continue;
            }
            let failed = block["is_error"].as_bool() == Some(true);
            let text = crate::native::content_text(&block["content"]).unwrap_or_default();
            let settled = if failed { Status::Failed } else { Status::Done };
            match &mut tool.kind {
                // A background agent's tool result only says it started.
                ItemKind::Agent { .. } if tool.background && !failed => continue,
                ItemKind::Agent { result, .. } => {
                    *result = Some(text.into());
                    tool.status = settled;
                }
                ItemKind::Tool { output, .. } => {
                    *output = (!text.is_empty()).then(|| Output::from_text(&text));
                    if !tool.background || failed {
                        tool.status = settled;
                    }
                }
                ItemKind::UserMessage { .. }
                | ItemKind::AssistantMessage { .. }
                | ItemKind::Question { .. }
                | ItemKind::Plan { .. }
                | ItemKind::Image { .. }
                | ItemKind::Compaction { .. }
                | ItemKind::Notice { .. }
                | ItemKind::Error { .. } => tool.status = settled,
            }
            out.push(tool.event());
        }
    }

    /// A message that came in from one of this session's agents (`from` is
    /// its id) or from another session (`name` is the session's).
    fn peer_message(&self, frame: &Value, parent: Option<String>, out: &mut Vec<EngineEvent>) {
        let origin = &frame["origin"];
        let Some(body) = str_at(origin, "body").filter(|body| !body.trim().is_empty()) else {
            return;
        };
        let recipient = if str_at(origin, "senderTaskId").is_some() {
            str_at(origin, "from")
        } else {
            str_at(origin, "name").or_else(|| str_at(origin, "from"))
        };
        let Some(recipient) = recipient else {
            tracing::warn!("Claude peer message omitted its sender");
            return;
        };
        let key = str_at(origin, "msg_id")
            .or_else(|| str_at(frame, "uuid"))
            .map_or_else(
                || format!("peer:{}", self.tools.len()),
                |id| format!("peer:{id}"),
            );
        out.push(EngineEvent::Item {
            key,
            parent,
            status: Status::Done,
            kind: ItemKind::Tool {
                tool: ToolKind::AgentMessage {
                    recipient: recipient.to_owned(),
                    target: None,
                    summary: None,
                    text: body.into(),
                    direction: MessageDirection::Received,
                },
                title: None,
                output: None,
                exit_code: None,
                approval: None,
            },
        });
    }

    fn result(&mut self, frame: &Value, out: &mut Vec<EngineEvent>) {
        self.result_seen = true;
        self.in_turn = false;
        let usage = &frame["usage"];
        let count = |field: &str| usage[field].as_u64().unwrap_or_default();
        self.usage.input += count("input_tokens");
        self.usage.output += count("output_tokens");
        self.usage.reasoning += usage["output_tokens_details"]["thinking_tokens"]
            .as_u64()
            .unwrap_or_default();
        self.usage.cache_read += count("cache_read_input_tokens");
        self.usage.cache_write =
            Some(self.usage.cache_write.unwrap_or_default() + count("cache_creation_input_tokens"));
        if let Some(cost) = frame["total_cost_usd"].as_f64() {
            self.usage.cost_usd = Some(self.usage.cost_usd.unwrap_or_default() + cost);
        }
        if let Some(window) = frame["modelUsage"].as_object().and_then(|models| {
            models
                .values()
                .filter_map(|model| model["contextWindow"].as_u64())
                .max()
        }) {
            self.usage.context_window = Some(window);
        }
        out.push(EngineEvent::Usage(self.usage.clone()));

        let aborted =
            str_at(frame, "terminal_reason").is_some_and(|reason| reason.starts_with("aborted"));
        let failed = frame["is_error"].as_bool() == Some(true)
            || str_at(frame, "subtype") != Some("success");
        self.settle_open_texts(
            if aborted {
                Status::Cancelled
            } else {
                Status::Done
            },
            out,
        );
        self.pending.clear();
        let outcome = if aborted {
            Outcome::cancelled()
        } else if failed {
            let subtype = str_at(frame, "subtype").unwrap_or("error");
            let message = frame["errors"]
                .as_array()
                .map(|errors| {
                    errors
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .filter(|message| !message.is_empty())
                .or_else(|| str_at(frame, "result").map(str::to_owned))
                .unwrap_or_else(|| subtype.to_owned());
            let kind = match subtype {
                "error_max_turns" => ErrorKind::MaxTurns,
                "error_during_execution" => ErrorKind::Execution,
                _ => ErrorKind::Api,
            };
            let status = frame["api_error_status"].as_u64();
            let retryable = status.is_some_and(|code| code == 429 || code >= 500);
            let key = self.next_key("error");
            out.push(error_item(key, kind, message.clone(), retryable));
            match self.limit_reached.take() {
                Some(LimitReached { resets_at }) => {
                    Outcome::retryable(message, Retry::UsageLimit { resets_at })
                }
                None if usage_limit_text(&message) => {
                    Outcome::retryable(message, Retry::UsageLimit { resets_at: None })
                }
                None if retryable || message.to_lowercase().contains("overloaded") => {
                    Outcome::retryable(message, Retry::Overloaded)
                }
                None => Outcome::failed(message),
            }
        } else if self.refused {
            Outcome {
                status: RunStatus::Refused,
                error: None,
                retry: None,
            }
        } else {
            Outcome::done()
        };
        self.limit_reached = None;
        self.outcome = Some(outcome);
    }

    /// The CLI's word on the account's limits before each request. Only a
    /// rejection matters: the turn that follows fails, and carries on once
    /// the window it names resets.
    fn rate_limit(&mut self, info: &Value) {
        if str_at(info, "status") == Some("rejected") {
            let resets_at = info["resetsAt"].as_i64().map(|seconds| seconds * 1_000);
            self.limit_reached = Some(LimitReached { resets_at });
        }
    }

    fn control_request(&mut self, frame: &Value, out: &mut Vec<EngineEvent>) {
        let Some(request_id) = str_at(frame, "request_id") else {
            return;
        };
        let request = &frame["request"];
        let subtype = str_at(request, "subtype").unwrap_or_default();
        if subtype != "can_use_tool" {
            // Nothing Artisan registers (hooks, SDK MCP servers) asks this;
            // say so rather than leave the CLI waiting.
            self.replies.push(json!({
                "type": "control_response",
                "response": { "subtype": "error", "request_id": request_id, "error": format!("unsupported request {subtype}") },
            }));
            return;
        }
        let name = str_at(request, "tool_name").unwrap_or("tool");
        let input = request["input"].clone();
        let key = str_at(request, "tool_use_id")
            .unwrap_or(request_id)
            .to_owned();
        if !self.tools.contains_key(&key) {
            self.tool_use(&key, name, &input, None, out);
            out.pop();
        }
        let suggestions = request["permission_suggestions"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let Some(tool) = self.tools.get_mut(&key) else {
            return;
        };
        let reason = str_at(request, "decision_reason")
            .or_else(|| str_at(request, "title"))
            .or_else(|| str_at(request, "description"))
            .map(str::to_owned);
        let mut options = vec![Decision::AllowOnce];
        if suggestions
            .iter()
            .any(|rule| str_at(rule, "type") == Some("addRules"))
        {
            options.push(Decision::AllowAlways);
        }
        options.push(Decision::DenyOnce);
        match &mut tool.kind {
            ItemKind::Tool {
                tool: kind,
                approval,
                ..
            } => {
                *approval = Some(Approval {
                    request: approval_request(kind, name, reason.as_deref()),
                    reason: reason.map(Into::into),
                    options,
                    decision: None,
                });
            }
            ItemKind::Plan { approval, .. } => {
                *approval = Some(Approval {
                    request: ApprovalRequest::Action {
                        description: ("Start implementing this plan".to_owned()).into(),
                    },
                    reason: None,
                    options: vec![Decision::AllowOnce, Decision::DenyOnce],
                    decision: None,
                });
            }
            ItemKind::UserMessage { .. }
            | ItemKind::AssistantMessage { .. }
            | ItemKind::Agent { .. }
            | ItemKind::Question { .. }
            | ItemKind::Image { .. }
            | ItemKind::Compaction { .. }
            | ItemKind::Notice { .. }
            | ItemKind::Error { .. } => {}
        }
        tool.status = Status::AwaitingUser;
        out.push(tool.event());
        self.pending.insert(
            key,
            Pending {
                request_id: request_id.to_owned(),
                input,
                suggestions,
            },
        );
    }

    fn control_response(&mut self, frame: &Value, out: &mut Vec<EngineEvent>) {
        let response = &frame["response"];
        if !self.title_pending || str_at(response, "request_id") != Some(TITLE_REQUEST_ID) {
            return;
        }
        self.title_pending = false;
        if let Some(title) = str_at(&response["response"], "title")
            .map(str::trim)
            .filter(|title| !title.is_empty())
        {
            self.titled = true;
            out.push(EngineEvent::Title {
                title: title.to_owned(),
            });
        }
    }

    fn settle_message(&mut self, id: &str, end_turn: bool, out: &mut Vec<EngineEvent>) {
        let Some(message) = self.messages.get_mut(id) else {
            return;
        };
        let texts = std::mem::take(&mut message.texts);
        let last = texts.len().saturating_sub(1);
        for (index, (key, text)) in texts.into_iter().enumerate() {
            let phase = if end_turn && index == last {
                Phase::Final
            } else {
                Phase::Commentary
            };
            out.push(EngineEvent::Item {
                key,
                parent: message.parent.clone(),
                status: Status::Done,
                kind: ItemKind::AssistantMessage {
                    text: text.into(),
                    phase,
                },
            });
        }
    }

    fn settle_open_texts(&mut self, status: Status, out: &mut Vec<EngineEvent>) {
        let mut keys: Vec<String> = self
            .messages
            .values_mut()
            .flat_map(|message| std::mem::take(&mut message.texts))
            .map(|(key, _)| key)
            .collect();
        keys.sort();
        out.extend(
            keys.into_iter()
                .map(|key| EngineEvent::Status { key, status }),
        );
    }

    fn next_key(&mut self, prefix: &str) -> String {
        self.counter += 1;
        format!("{prefix}-{}", self.counter)
    }
}

impl crate::retained::Retains for ClaudeDecoder {
    fn retained(&mut self, key: &str) -> Option<&mut ItemKind> {
        self.tools.get_mut(key).map(|tool| &mut tool.kind)
    }
}

impl Tool {
    fn event(&self) -> EngineEvent {
        EngineEvent::Item {
            key: self.key.clone(),
            parent: self.parent.clone(),
            status: self.status,
            kind: self.kind.clone(),
        }
    }
}

/// What a `can_use_tool` request asks of the user for a `name` tool call.
fn approval_request(kind: &ToolKind, name: &str, reason: Option<&str>) -> ApprovalRequest {
    match kind {
        ToolKind::Shell { command, cwd, .. } => ApprovalRequest::Command {
            command: command.clone(),
            cwd: cwd.clone(),
        },
        ToolKind::Edit { .. } => ApprovalRequest::FileChange,
        ToolKind::AgentMessage { .. }
        | ToolKind::Agents { .. }
        | ToolKind::Read { .. }
        | ToolKind::Search { .. }
        | ToolKind::WebFetch { .. }
        | ToolKind::WebSearch { .. }
        | ToolKind::Mcp { .. }
        | ToolKind::ViewImage { .. }
        | ToolKind::Other { .. } => ApprovalRequest::Action {
            description: (reason.map_or_else(|| format!("Use {name}"), str::to_owned)).into(),
        },
    }
}

/// Claude's `answers` input: question text to the chosen labels, skipped
/// questions left out.
#[expect(
    clippy::expect_used,
    reason = "native_collection_answers already proved the questions inline"
)]
fn answer_map(
    questions: &artisan_api::Collection<artisan_api::Question>,
    native: &[Option<Vec<String>>],
) -> Map<String, Value> {
    let mut map = Map::new();
    for (question, answer) in questions
        .inline()
        .expect("native question validated before mutation")
        .iter()
        .zip(native)
    {
        if let Some(labels) = answer.as_ref().filter(|labels| !labels.is_empty()) {
            map.insert(
                question
                    .text
                    .inline()
                    .expect("adapter question is inline before storage")
                    .to_owned(),
                Value::String(labels.join(", ")),
            );
        }
    }
    map
}

fn compaction(key: &str, status: Status, before: Option<u64>, after: Option<u64>) -> EngineEvent {
    EngineEvent::Item {
        key: key.to_owned(),
        parent: None,
        status,
        kind: ItemKind::Compaction {
            summary: None,
            tokens_before: before,
            tokens_after: after,
            switch: None,
        },
    }
}

fn error_item(key: String, kind: ErrorKind, message: String, retryable: bool) -> EngineEvent {
    EngineEvent::Item {
        key,
        parent: None,
        status: Status::Done,
        kind: ItemKind::Error {
            kind,
            message: message.into(),
            retryable,
        },
    }
}

pub(super) fn str_at<'a>(value: &'a Value, field: &str) -> Option<&'a str> {
    value.get(field).and_then(Value::as_str)
}

/// A usage limit the CLI hit: when it resets, ms, if it said.
#[derive(Clone, Copy)]
struct LimitReached {
    resets_at: Option<Timestamp>,
}

/// Claude's words for an exhausted plan ("Claude AI usage limit reached",
/// "You've hit your limit · resets 3pm").
fn usage_limit_text(message: &str) -> bool {
    let message = message.to_lowercase();
    message.contains("usage limit") || message.contains("hit your limit")
}
