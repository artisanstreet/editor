//! Pure decoding of `codex app-server` JSON-RPC (one JSON object per line).
//!
//! The decoder follows the run's root thread plus every child thread its
//! agents spawn. Each child thread becomes an `Agent` item (keyed by the
//! `spawnAgent` call) and everything the child does is decoded the same way
//! with `parent` set to that item. It also keeps the JSON-RPC bookkeeping:
//! request ids, responses for the driver, and replies to Codex's own
//! requests (approvals, questions). Shapes were captured from Codex 0.162.1
//! (see `transcripts/codex`) and its generated app-server schema.

use crate::{EngineEvent, LiveKind, Outcome};
use artisan_api::{
    Answer, Approval, ApprovalRequest, BlobHash, Decision, ErrorKind, FileChange, FileOp, ItemKind,
    NoticeKind, Output, Phase, PlanStep, Question, QuestionOption, RunStatus, Status, StepStatus,
    ToolKind, Usage,
};
use serde_json::{Map, Value, json};
use std::collections::HashMap;

/// The key of the run's plan (`turn/plan/updated`); one per run.
const PLAN_KEY: &str = "plan";

/// What Codex answered to one of our requests.
#[derive(Debug)]
pub(crate) struct Response {
    pub id: u64,
    pub method: String,
    pub result: Result<Value, String>,
}

#[derive(Default)]
pub(crate) struct Decoder {
    next_id: u64,
    outbox: Vec<Value>,
    requests: HashMap<u64, String>,
    responses: Vec<Response>,
    root: Option<String>,
    /// Followed child threads → the `Agent` item their items belong to.
    children: HashMap<String, String>,
    /// `spawnAgent` calls whose child thread isn't known yet, oldest first.
    spawning: Vec<String>,
    /// Child thread → text of its last agent message, the agent's result.
    child_replies: HashMap<String, String>,
    items: HashMap<String, Tracked>,
    /// Item key → the Codex request waiting for the user.
    pending: HashMap<String, Pending>,
    turn: Option<String>,
    turn_end: Option<Outcome>,
    /// A user message reached the root turn and nothing ran after it yet.
    unanswered: bool,
    /// The last turn completed without answering a steer it took.
    stranded: bool,
    usage: Usage,
    /// The last error Codex reported, so a failed turn doesn't repeat it.
    last_error: Option<String>,
    notices: u32,
    /// User messages to send for answered async questions.
    messages: Vec<String>,
}

struct Tracked {
    parent: Option<String>,
    status: Status,
    kind: ItemKind,
}

enum Pending {
    Command {
        rpc: Value,
    },
    FileChange {
        rpc: Value,
    },
    Permissions {
        rpc: Value,
        requested: Value,
    },
    Question {
        rpc: Value,
        ids: Vec<String>,
    },
    /// Questions inside an agent message; the answer goes back as a message.
    AsyncQuestion {
        texts: Vec<String>,
    },
}

/// Where a frame belongs: the root thread (`None`) or a child thread's
/// agent item (`Some`).
type Scope = Option<String>;

impl Decoder {
    /// Queues a request and returns its id.
    pub fn request(&mut self, method: &str, params: Value) -> u64 {
        self.next_id += 1;
        let id = self.next_id;
        self.requests.insert(id, method.to_owned());
        self.outbox
            .push(json!({ "id": id, "method": method, "params": params }));
        id
    }

    pub fn notify(&mut self, method: &str) {
        self.outbox.push(json!({ "method": method }));
    }

    pub fn take_outbox(&mut self) -> Vec<Value> {
        std::mem::take(&mut self.outbox)
    }

    pub fn take_responses(&mut self) -> Vec<Response> {
        std::mem::take(&mut self.responses)
    }

    /// How the root thread's last turn ended, once.
    pub fn take_turn_end(&mut self) -> Option<Outcome> {
        self.turn_end.take()
    }

    /// Whether the last turn ended right after taking a steer, so Codex
    /// recorded it but never answered (seen on 0.162.1 when `turn/steer`
    /// lands a few ms before the turn completes). Once.
    pub fn take_stranded(&mut self) -> bool {
        std::mem::take(&mut self.stranded)
    }

    /// Messages answering async questions, to send as user input.
    pub fn take_messages(&mut self) -> Vec<String> {
        std::mem::take(&mut self.messages)
    }

    pub fn thread(&self) -> Option<&str> {
        self.root.as_deref()
    }

    /// The root thread's running turn.
    pub fn active_turn(&self) -> Option<&str> {
        self.turn.as_deref()
    }

    /// One line from Codex.
    pub fn feed(&mut self, line: &str) -> Vec<EngineEvent> {
        let Ok(message) = serde_json::from_str::<Value>(line) else {
            tracing::warn!("Codex wrote a line that is not JSON");
            return Vec::new();
        };
        let mut out = Vec::new();
        match (message["method"].as_str(), message.get("id")) {
            (Some(method), Some(id)) => {
                self.server_request(id.clone(), method, &message["params"], &mut out)
            }
            (Some(method), None) => self.notification(method, &message["params"], &mut out),
            (None, Some(id)) => self.response(id, &message, &mut out),
            (None, None) => {}
        }
        out
    }

    fn response(&mut self, id: &Value, message: &Value, out: &mut Vec<EngineEvent>) {
        let result = &message["result"];
        // `thread/start` and `thread/resume` answer with the thread;
        // `turn/start` with the turn.
        if let Some(thread) = result["thread"]["id"].as_str() {
            self.root = Some(thread.to_owned());
            out.push(EngineEvent::Session {
                id: thread.to_owned(),
            });
            if let Some(model) = str_at(result, "model") {
                out.push(EngineEvent::Model {
                    id: model.to_owned(),
                });
            }
        }
        if let Some(turn) = result["turn"]["id"].as_str() {
            self.turn = Some(turn.to_owned());
        }
        let Some(id) = id.as_u64() else {
            return;
        };
        let method = self.requests.remove(&id).unwrap_or_default();
        let result = match message.get("error") {
            Some(error) => Err(error["message"]
                .as_str()
                .map_or_else(|| error.to_string(), str::to_owned)),
            None => Ok(result.clone()),
        };
        self.responses.push(Response { id, method, result });
    }

    /// The scope of a frame's `threadId`; `None` for threads we don't follow.
    fn scope(&mut self, params: &Value) -> Option<Scope> {
        let thread = params["threadId"].as_str()?;
        if self.root.as_deref() == Some(thread) {
            return Some(None);
        }
        if let Some(agent) = self.children.get(thread) {
            return Some(Some(agent.clone()));
        }
        // A child can speak before its `spawnAgent` call names it.
        let agent = self.spawning.first()?.clone();
        self.bind_child(thread, &agent);
        Some(Some(agent))
    }

    fn bind_child(&mut self, thread: &str, agent: &str) {
        self.spawning.retain(|key| key != agent);
        self.children.insert(thread.to_owned(), agent.to_owned());
    }

    fn notification(&mut self, method: &str, params: &Value, out: &mut Vec<EngineEvent>) {
        if method == "thread/started" {
            self.thread_started(&params["thread"], out);
            return;
        }
        if method == "thread/name/updated" {
            if params["threadId"]
                .as_str()
                .is_some_and(|thread| self.root.as_deref() == Some(thread))
                && let Some(title) = str_at(params, "threadName")
                    .map(str::trim)
                    .filter(|title| !title.is_empty())
            {
                out.push(EngineEvent::Title {
                    title: title.to_owned(),
                });
            }
            return;
        }
        let Some(scope) = self.scope(params) else {
            return;
        };
        let item_key = || params["itemId"].as_str().unwrap_or_default().to_owned();
        match method {
            "turn/started" if scope.is_none() => {
                self.turn = params["turn"]["id"].as_str().map(str::to_owned);
                self.unanswered = false;
            }
            "turn/completed" => self.turn_completed(scope, params, out),
            "item/started" => self.item(scope, &params["item"], false, out),
            "item/completed" => self.item(scope, &params["item"], true, out),
            "item/agentMessage/delta" | "item/plan/delta" => {
                if let Some(delta) = str_at(params, "delta") {
                    out.push(live(LiveKind::Text { key: item_key() }, delta));
                }
            }
            "item/reasoning/summaryTextDelta" if scope.is_none() => {
                if let Some(delta) = str_at(params, "delta") {
                    out.push(live(LiveKind::Reasoning, delta));
                }
            }
            "item/reasoning/summaryPartAdded" if scope.is_none() => {
                if params["summaryIndex"]
                    .as_u64()
                    .is_some_and(|index| index > 0)
                {
                    out.push(live(LiveKind::Reasoning, "\n\n"));
                }
            }
            "item/commandExecution/outputDelta" => {
                if let Some(delta) = str_at(params, "delta") {
                    out.push(live(LiveKind::Shell { key: item_key() }, delta));
                }
            }
            "item/mcpToolCall/progress" => {
                if let Some(message) = str_at(params, "message") {
                    out.push(EngineEvent::Live {
                        target: LiveKind::Progress { key: item_key() },
                        text: message.to_owned(),
                        append: false,
                    });
                }
            }
            "turn/plan/updated" => self.plan(scope, params, out),
            "thread/tokenUsage/updated" => {
                self.token_usage(scope.is_none(), &params["tokenUsage"], out)
            }
            "model/rerouted" => {
                let to = str_at(params, "toModel").unwrap_or_default();
                let text = format!(
                    "Codex moved this turn from {} to {to} ({}).",
                    str_at(params, "fromModel").unwrap_or("the chosen model"),
                    str_at(params, "reason").unwrap_or("reroute"),
                );
                let key = format!("reroute:{}", str_at(params, "turnId").unwrap_or_default());
                self.notice(
                    key,
                    scope,
                    NoticeKind::Reroute {
                        model: to.to_owned(),
                    },
                    text,
                    out,
                );
            }
            "model/safetyBuffering/updated" => {
                let key = format!("safety:{}", str_at(params, "turnId").unwrap_or_default());
                if params["showBufferingUi"].as_bool() == Some(true)
                    && !self.items.contains_key(&key)
                {
                    let text = "Codex is holding this reply for a safety check.".to_owned();
                    self.notice(key, scope, NoticeKind::Safety, text, out);
                }
            }
            "thread/goal/updated" => {
                let goal = &params["goal"];
                let state = str_at(goal, "status").unwrap_or("active").to_owned();
                let text = str_at(goal, "objective").unwrap_or_default().to_owned();
                if text.trim().is_empty() {
                    return;
                }
                self.notice(
                    goal_key(scope.as_ref()),
                    scope,
                    NoticeKind::Goal { state },
                    text,
                    out,
                );
            }
            "thread/goal/cleared" => {
                let key = goal_key(scope.as_ref());
                // Codex clears at every turn, goal or not: only a goal this
                // session showed is reported cleared.
                let text = match self.items.get(&key).map(|item| &item.kind) {
                    Some(ItemKind::Notice { text, .. }) if !text.trim().is_empty() => text.clone(),
                    _ => return,
                };
                self.notice(
                    key,
                    scope,
                    NoticeKind::Goal {
                        state: "cleared".to_owned(),
                    },
                    text,
                    out,
                );
            }
            "error" => self.error(scope, params, out),
            "warning" => {
                if let Some(message) = str_at(params, "message") {
                    self.notices += 1;
                    let key = format!("warning-{}", self.notices);
                    self.notice(key, scope, NoticeKind::Info, message.to_owned(), out);
                }
            }
            "serverRequest/resolved" => self.resolved(&params["requestId"], out),
            _ => {}
        }
    }

    fn thread_started(&mut self, thread: &Value, out: &mut Vec<EngineEvent>) {
        let (Some(id), Some(parent)) = (str_at(thread, "id"), str_at(thread, "parentThreadId"))
        else {
            return;
        };
        if self.root.as_deref() == Some(id) || self.children.contains_key(id) {
            return;
        }
        let parent_scope = if self.root.as_deref() == Some(parent) {
            None
        } else if let Some(agent) = self.children.get(parent) {
            Some(agent.clone())
        } else {
            return;
        };
        let name = str_at(thread, "agentNickname").map(str::to_owned);
        // Prefer the spawn call that made it; otherwise the thread is its
        // own agent item.
        let agent = if let Some(agent) = self.spawning.first() {
            agent.clone()
        } else {
            let kind = ItemKind::Agent {
                name: None,
                description: String::new(),
                prompt: None,
                agent_id: None,
                background: false,
                result: None,
                label: None,
            };
            self.track(id.to_owned(), parent_scope, Status::Running, kind);
            id.to_owned()
        };
        self.bind_child(id, &agent);
        if let Some(name) = name
            && let Some(tracked) = self.items.get_mut(&agent)
            && let ItemKind::Agent {
                name: slot @ None, ..
            } = &mut tracked.kind
        {
            *slot = Some(name);
            out.push(self.event(&agent));
        }
    }

    fn turn_completed(&mut self, scope: Scope, params: &Value, out: &mut Vec<EngineEvent>) {
        let turn = &params["turn"];
        let error = &turn["error"];
        let message = str_at(error, "message").map(str::to_owned);
        match scope {
            None => {
                self.turn = None;
                let outcome = match str_at(turn, "status") {
                    Some("interrupted") => Outcome::cancelled(),
                    Some("failed") => {
                        let refused = is_refusal(error);
                        let message =
                            message.unwrap_or_else(|| "Codex failed this turn".to_owned());
                        if self.last_error.as_deref() != Some(message.as_str()) {
                            self.error_item(
                                None,
                                if refused {
                                    ErrorKind::Refusal
                                } else {
                                    ErrorKind::Api
                                },
                                &message,
                                out,
                            );
                        }
                        Outcome {
                            status: if refused {
                                RunStatus::Refused
                            } else {
                                RunStatus::Failed
                            },
                            error: Some(message),
                        }
                    }
                    _ => {
                        self.stranded = std::mem::take(&mut self.unanswered);
                        Outcome::done()
                    }
                };
                self.unanswered = false;
                self.last_error = None;
                self.turn_end = Some(outcome);
            }
            Some(agent) => {
                let status = match str_at(turn, "status") {
                    Some("interrupted") => Status::Cancelled,
                    Some("failed") => Status::Failed,
                    _ => Status::Done,
                };
                let thread = params["threadId"].as_str().unwrap_or_default();
                let reply = self.child_replies.get(thread).cloned().or(message);
                if let Some(tracked) = self.items.get_mut(&agent)
                    && let ItemKind::Agent { result, .. } = &mut tracked.kind
                {
                    if reply.is_some() {
                        *result = reply;
                    }
                    tracked.status = tracked.status.advance(status);
                    out.push(self.event(&agent));
                }
            }
        }
    }

    fn item(&mut self, scope: Scope, item: &Value, completed: bool, out: &mut Vec<EngineEvent>) {
        let Some(key) = str_at(item, "id").map(str::to_owned) else {
            return;
        };
        let item_type = str_at(item, "type").unwrap_or_default();
        if scope.is_none() {
            match (item_type, completed) {
                ("userMessage", true) => self.unanswered = true,
                ("userMessage", false) | (_, true) => {}
                (_, false) => self.unanswered = false,
            }
        }
        let settled = |done: bool| {
            if completed && done {
                Status::Done
            } else {
                Status::Running
            }
        };
        let (status, kind) = match item_type {
            "agentMessage" => {
                let text = str_at(item, "text").unwrap_or_default().to_owned();
                let phase = match str_at(item, "phase") {
                    Some("commentary") => Phase::Commentary,
                    _ => Phase::Final,
                };
                if completed {
                    self.remember_reply(scope.as_ref(), &text);
                    self.async_questions(&key, scope.clone(), item, out);
                }
                (settled(true), ItemKind::AssistantMessage { text, phase })
            }
            "plan" => {
                let text = str_at(item, "text").unwrap_or_default().to_owned();
                (
                    settled(true),
                    ItemKind::AssistantMessage {
                        text,
                        phase: Phase::Final,
                    },
                )
            }
            "userMessage" => {
                // Ours are stored by Forge. A child's first message is its
                // task; keep it when the spawn call didn't carry one.
                if let Some(agent) = &scope
                    && let Some(tracked) = self.items.get_mut(agent)
                    && let ItemKind::Agent { description, .. } = &mut tracked.kind
                    && description.is_empty()
                {
                    *description = user_text(item);
                    out.push(self.event(agent));
                }
                return;
            }
            "commandExecution" => {
                let status = match str_at(item, "status") {
                    Some("completed") => Status::Done,
                    Some("failed") => Status::Failed,
                    Some("declined") => Status::Declined,
                    _ => Status::Running,
                };
                let tool = ToolKind::Shell {
                    command: display_command(str_at(item, "command").unwrap_or_default()),
                    cwd: str_at(item, "cwd").map(str::to_owned),
                    background: false,
                };
                let output = str_at(item, "aggregatedOutput")
                    .filter(|text| !text.is_empty())
                    .map(Output::from_text);
                let exit_code = item["exitCode"]
                    .as_i64()
                    .and_then(|code| i32::try_from(code).ok());
                (status, self.tool_kind(&key, tool, output, exit_code))
            }
            "fileChange" => {
                let status = match str_at(item, "status") {
                    Some("completed") => Status::Done,
                    Some("failed") => Status::Failed,
                    Some("declined") => Status::Declined,
                    _ => Status::Running,
                };
                let changes = item["changes"]
                    .as_array()
                    .map(|changes| changes.iter().filter_map(file_change).collect())
                    .unwrap_or_default();
                (
                    status,
                    self.tool_kind(&key, ToolKind::Edit { changes }, None, None),
                )
            }
            "mcpToolCall" => {
                let status = tool_status(item, completed);
                let tool = ToolKind::Mcp {
                    server: str_at(item, "server").unwrap_or_default().to_owned(),
                    tool: str_at(item, "tool").unwrap_or_default().to_owned(),
                    input: arguments(&item["arguments"]),
                };
                let output = str_at(&item["error"], "message")
                    .map(str::to_owned)
                    .or_else(|| content_text(&item["result"]["content"]))
                    .map(|text| Output::from_text(&text));
                (status, self.tool_kind(&key, tool, output, None))
            }
            "dynamicToolCall" => {
                let status = if completed && item["success"].as_bool() == Some(false) {
                    Status::Failed
                } else {
                    tool_status(item, completed)
                };
                let tool = ToolKind::Other {
                    name: str_at(item, "tool").unwrap_or_default().to_owned(),
                    input: arguments(&item["arguments"]),
                };
                let output =
                    content_text(&item["contentItems"]).map(|text| Output::from_text(&text));
                (status, self.tool_kind(&key, tool, output, None))
            }
            "webSearch" => {
                let action = &item["action"];
                let query = str_at(item, "query")
                    .filter(|query| !query.is_empty())
                    .or_else(|| str_at(action, "query"))
                    .or_else(|| str_at(action, "url"))
                    .unwrap_or_default()
                    .to_owned();
                (
                    settled(true),
                    self.tool_kind(&key, ToolKind::WebSearch { query }, None, None),
                )
            }
            "imageView" => {
                let path = str_at(item, "path").unwrap_or_default().to_owned();
                (
                    settled(true),
                    self.tool_kind(&key, ToolKind::ViewImage { path }, None, None),
                )
            }
            "imageGeneration" => {
                let failed = !item["failure"].is_null()
                    || matches!(
                        str_at(item, "status"),
                        Some("failed" | "error" | "cancelled")
                    );
                let status = if completed && failed {
                    Status::Failed
                } else {
                    settled(true)
                };
                // Codex saves the image itself; the path rides in `caption`
                // until Forge reads it into a blob and fills `blob`.
                let caption = str_at(item, "savedPath")
                    .or_else(|| str_at(item, "revisedPrompt"))
                    .map(str::to_owned);
                (
                    status,
                    ItemKind::Image {
                        blob: BlobHash(String::new()),
                        caption,
                    },
                )
            }
            "contextCompaction" => {
                let tokens_before = match self.items.get(&key).map(|tracked| &tracked.kind) {
                    Some(ItemKind::Compaction { tokens_before, .. }) => *tokens_before,
                    _ => self.usage.context_used,
                };
                (
                    settled(true),
                    ItemKind::Compaction {
                        tokens_before,
                        tokens_after: None,
                    },
                )
            }
            "collabAgentToolCall" => {
                self.collab(key, scope, item, completed, out);
                return;
            }
            _ => return,
        };
        self.track(key.clone(), scope, status, kind);
        out.push(self.event(&key));
    }

    /// A tool kind for `key`, keeping an approval already on the item.
    fn tool_kind(
        &self,
        key: &str,
        tool: ToolKind,
        output: Option<Output>,
        exit_code: Option<i32>,
    ) -> ItemKind {
        let (title, approval) = match self.items.get(key).map(|tracked| &tracked.kind) {
            Some(ItemKind::Tool {
                title, approval, ..
            }) => (title.clone(), approval.clone()),
            _ => (None, None),
        };
        ItemKind::Tool {
            tool,
            title,
            output,
            exit_code,
            approval,
        }
    }

    fn collab(
        &mut self,
        key: String,
        scope: Scope,
        item: &Value,
        completed: bool,
        out: &mut Vec<EngineEvent>,
    ) {
        let receivers: Vec<&str> = item["receiverThreadIds"]
            .as_array()
            .map(|ids| ids.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        if str_at(item, "tool") != Some("spawnAgent") {
            // `wait` reports what the agents said; fill results we missed.
            for (thread, state) in item["agentsStates"].as_object().into_iter().flatten() {
                let Some(agent) = self.children.get(thread).cloned() else {
                    continue;
                };
                let Some(message) = str_at(state, "message") else {
                    continue;
                };
                if let Some(tracked) = self.items.get_mut(&agent)
                    && let ItemKind::Agent {
                        result: result @ None,
                        ..
                    } = &mut tracked.kind
                {
                    *result = Some(message.to_owned());
                    out.push(self.event(&agent));
                }
            }
            return;
        }
        if !self.items.contains_key(&key) {
            let kind = ItemKind::Agent {
                name: None,
                description: str_at(item, "prompt").unwrap_or_default().to_owned(),
                prompt: None,
                agent_id: None,
                background: false,
                result: None,
                label: None,
            };
            self.track(key.clone(), scope, Status::Running, kind);
            if receivers.is_empty() {
                self.spawning.push(key.clone());
            }
            out.push(self.event(&key));
        }
        for thread in receivers {
            if !self.children.contains_key(thread) {
                self.bind_child(thread, &key);
            }
        }
        if completed && str_at(item, "status") == Some("failed") {
            self.spawning.retain(|spawn| *spawn != key);
            if let Some(tracked) = self.items.get_mut(&key) {
                tracked.status = tracked.status.advance(Status::Failed);
            }
            out.push(self.event(&key));
        }
    }

    fn remember_reply(&mut self, scope: Option<&String>, text: &str) {
        if let Some(agent) = scope
            && let Some(thread) = self
                .children
                .iter()
                .find(|(_, key)| *key == agent)
                .map(|(thread, _)| thread.clone())
            && !text.is_empty()
        {
            self.child_replies.insert(thread, text.to_owned());
        }
    }

    /// An agent message may carry questions it doesn't wait for; answers
    /// go back as the next user message.
    fn async_questions(
        &mut self,
        key: &str,
        scope: Scope,
        item: &Value,
        out: &mut Vec<EngineEvent>,
    ) {
        let Some(questions) = item["questions"]
            .as_array()
            .filter(|questions| !questions.is_empty())
        else {
            return;
        };
        let questions: Vec<Question> = questions
            .iter()
            .enumerate()
            .filter_map(|(index, question)| {
                Some(Question {
                    id: index.to_string(),
                    header: None,
                    text: str_at(question, "title")?.to_owned(),
                    multi_select: false,
                    options: question["options"]
                        .as_array()
                        .map(|options| {
                            options
                                .iter()
                                .filter_map(Value::as_str)
                                .map(|label| QuestionOption {
                                    label: label.to_owned(),
                                    description: None,
                                })
                                .collect()
                        })
                        .unwrap_or_default(),
                })
            })
            .collect();
        if questions.is_empty() {
            return;
        }
        let key = format!("{key}:questions");
        let texts = questions
            .iter()
            .map(|question| question.text.clone())
            .collect();
        self.pending
            .insert(key.clone(), Pending::AsyncQuestion { texts });
        let kind = ItemKind::Question {
            blocking: false,
            questions,
            answers: None,
        };
        self.track(key.clone(), scope, Status::AwaitingUser, kind);
        out.push(self.event(&key));
    }

    fn plan(&mut self, scope: Scope, params: &Value, out: &mut Vec<EngineEvent>) {
        let steps: Vec<PlanStep> = params["plan"]
            .as_array()
            .map(|steps| {
                steps
                    .iter()
                    .filter_map(|step| {
                        Some(PlanStep {
                            text: str_at(step, "step")?.to_owned(),
                            status: match str_at(step, "status") {
                                Some("completed") => StepStatus::Done,
                                Some("inProgress") => StepStatus::InProgress,
                                _ => StepStatus::Pending,
                            },
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        let done = steps.iter().all(|step| step.status == StepStatus::Done);
        let key = scope.as_ref().map_or_else(
            || PLAN_KEY.to_owned(),
            |agent| format!("{PLAN_KEY}:{agent}"),
        );
        let status = if done { Status::Done } else { Status::Running };
        // A plan is replaced in place and may reopen.
        self.items.remove(&key);
        self.track(
            key.clone(),
            scope,
            status,
            ItemKind::Plan {
                steps,
                proposal: None,
                approval: None,
            },
        );
        out.push(self.event(&key));
    }

    fn token_usage(&mut self, root: bool, usage: &Value, out: &mut Vec<EngineEvent>) {
        // `last` is one model call; `total` is the whole thread, history
        // included on resume, so the run sums `last`.
        let last = &usage["last"];
        let count = |field: &str| last[field].as_u64().unwrap_or_default();
        let cached = count("cachedInputTokens");
        self.usage.input += count("inputTokens").saturating_sub(cached);
        self.usage.cache_read += cached;
        self.usage.output += count("outputTokens");
        self.usage.reasoning += count("reasoningOutputTokens");
        if let Some(written) = last["cacheWriteInputTokens"].as_u64() {
            self.usage.cache_write = Some(self.usage.cache_write.unwrap_or_default() + written);
        }
        if root {
            if let Some(used) = last["totalTokens"].as_u64() {
                self.usage.context_used = Some(used);
            }
            if let Some(window) = usage["modelContextWindow"].as_u64() {
                self.usage.context_window = Some(window);
            }
        }
        out.push(EngineEvent::Usage(self.usage.clone()));
    }

    fn error(&mut self, scope: Scope, params: &Value, out: &mut Vec<EngineEvent>) {
        let error = &params["error"];
        let message = str_at(error, "message")
            .unwrap_or("Codex reported an error")
            .to_owned();
        if params["willRetry"].as_bool() == Some(true) {
            out.push(EngineEvent::Live {
                target: LiveKind::Status,
                text: format!("{message} Retrying…"),
                append: false,
            });
            return;
        }
        let kind = if is_refusal(error) {
            ErrorKind::Refusal
        } else {
            ErrorKind::Api
        };
        if scope.is_none() {
            self.last_error = Some(message.clone());
        }
        self.error_item(scope, kind, &message, out);
    }

    fn error_item(
        &mut self,
        scope: Scope,
        kind: ErrorKind,
        message: &str,
        out: &mut Vec<EngineEvent>,
    ) {
        self.notices += 1;
        let key = format!("error-{}", self.notices);
        let retryable = false;
        self.track(
            key.clone(),
            scope,
            Status::Done,
            ItemKind::Error {
                kind,
                message: message.to_owned(),
                retryable,
            },
        );
        out.push(self.event(&key));
    }

    fn notice(
        &mut self,
        key: String,
        scope: Scope,
        kind: NoticeKind,
        text: String,
        out: &mut Vec<EngineEvent>,
    ) {
        self.items.remove(&key);
        self.track(
            key.clone(),
            scope,
            Status::Done,
            ItemKind::Notice { kind, text },
        );
        out.push(self.event(&key));
    }

    // ------------------------------------------------------------ requests

    fn server_request(
        &mut self,
        rpc: Value,
        method: &str,
        params: &Value,
        out: &mut Vec<EngineEvent>,
    ) {
        let scope = self.scope(params);
        let key = str_at(params, "itemId").map_or_else(|| format!("request-{rpc}"), str::to_owned);
        match (method, scope) {
            ("item/commandExecution/requestApproval", Some(scope)) => {
                let command = display_command(str_at(params, "command").unwrap_or_default());
                let cwd = str_at(params, "cwd").map(str::to_owned);
                let request = if str_at(params, "kind") == Some("writeStdin") {
                    ApprovalRequest::Action {
                        description: format!("Send input to `{command}`"),
                    }
                } else {
                    ApprovalRequest::Command {
                        command: command.clone(),
                        cwd: cwd.clone(),
                    }
                };
                let tool = ToolKind::Shell {
                    command,
                    cwd,
                    background: false,
                };
                self.ask(&key, scope, tool, request, params, out);
                self.pending.insert(key, Pending::Command { rpc });
            }
            ("item/fileChange/requestApproval", Some(scope)) => {
                let tool = ToolKind::Edit {
                    changes: Vec::new(),
                };
                self.ask(&key, scope, tool, ApprovalRequest::FileChange, params, out);
                self.pending.insert(key, Pending::FileChange { rpc });
            }
            ("item/permissions/requestApproval", Some(scope)) => {
                let description = str_at(params, "reason").map_or_else(
                    || format!("Grant more permissions: {}", params["permissions"]),
                    str::to_owned,
                );
                let tool = ToolKind::Other {
                    name: "permissions".to_owned(),
                    input: Some(params["permissions"].to_string()),
                };
                self.ask(
                    &key,
                    scope,
                    tool,
                    ApprovalRequest::Action { description },
                    params,
                    out,
                );
                let requested = strip_nulls(&params["permissions"]);
                self.pending
                    .insert(key, Pending::Permissions { rpc, requested });
            }
            ("item/tool/requestUserInput", Some(scope)) => {
                let questions: Vec<Question> = params["questions"]
                    .as_array()
                    .map(|questions| questions.iter().filter_map(question).collect())
                    .unwrap_or_default();
                let ids = questions
                    .iter()
                    .map(|question| question.id.clone())
                    .collect();
                let blocking = params["isBlocking"].as_bool().unwrap_or(true);
                self.items.remove(&key);
                self.track(
                    key.clone(),
                    scope,
                    Status::AwaitingUser,
                    ItemKind::Question {
                        blocking,
                        questions,
                        answers: None,
                    },
                );
                out.push(self.event(&key));
                self.pending.insert(key, Pending::Question { rpc, ids });
            }
            ("mcpServer/elicitation/request", scope) => {
                // Artisan has no form for MCP elicitation yet; say so and
                // let the server go on without the input.
                let server = str_at(params, "serverName").unwrap_or("An MCP server");
                let text = format!(
                    "{server} asked for input Artisan can't show yet: {}",
                    str_at(params, "message").unwrap_or_default()
                );
                self.notices += 1;
                if let Some(scope) = scope {
                    self.notice(
                        format!("elicitation-{}", self.notices),
                        scope,
                        NoticeKind::Info,
                        text,
                        out,
                    );
                }
                self.reply(
                    rpc,
                    json!({ "action": "decline", "content": null, "_meta": null }),
                );
            }
            (method, _) => {
                tracing::debug!(method, "declining a Codex request Artisan doesn't serve");
                self.outbox.push(json!({ "id": rpc, "error": { "code": -32601, "message": format!("Artisan does not handle {method}") } }));
            }
        }
    }

    /// Puts an approval on the item it gates (creating the item if Codex
    /// asks before announcing it).
    fn ask(
        &mut self,
        key: &str,
        scope: Scope,
        tool: ToolKind,
        request: ApprovalRequest,
        params: &Value,
        out: &mut Vec<EngineEvent>,
    ) {
        let approval = Approval {
            request,
            reason: str_at(params, "reason").map(str::to_owned),
            // Codex lists fewer in `availableDecisions`, but takes all four
            // (verified on 0.162.1: `decline` declines the call and
            // `acceptForSession` skips the next identical prompt).
            options: vec![
                Decision::AllowOnce,
                Decision::AllowAlways,
                Decision::DenyOnce,
                Decision::Cancelled,
            ],
            decision: None,
        };
        if let Some(tracked) = self.items.get_mut(key) {
            if let ItemKind::Tool { approval: slot, .. } = &mut tracked.kind {
                *slot = Some(approval);
            }
            tracked.status = tracked.status.advance(Status::AwaitingUser);
        } else {
            let kind = ItemKind::Tool {
                tool,
                title: None,
                output: None,
                exit_code: None,
                approval: Some(approval),
            };
            self.track(key.to_owned(), scope, Status::AwaitingUser, kind);
        }
        out.push(self.event(key));
    }

    /// The user's answer to the approval or question on `key`.
    pub fn answer(&mut self, key: &str, answer: &Answer) -> Vec<EngineEvent> {
        let Some(pending) = self.pending.remove(key) else {
            tracing::debug!(key, "an answer for a Codex item that isn't waiting");
            return Vec::new();
        };
        match (pending, answer) {
            (
                Pending::Command { rpc } | Pending::FileChange { rpc },
                Answer::Approval { decision },
            ) => {
                let reply = match decision {
                    Decision::AllowOnce => "accept",
                    Decision::AllowAlways => "acceptForSession",
                    Decision::DenyOnce | Decision::DenyAlways => "decline",
                    Decision::Cancelled => "cancel",
                };
                self.reply(rpc, json!({ "decision": reply }));
                self.decided(key, *decision)
            }
            (Pending::Permissions { rpc, requested }, Answer::Approval { decision }) => {
                let result = match decision {
                    Decision::AllowOnce => json!({ "permissions": requested, "scope": "turn" }),
                    Decision::AllowAlways => {
                        json!({ "permissions": requested, "scope": "session" })
                    }
                    _ => json!({ "permissions": {}, "scope": "turn" }),
                };
                self.reply(rpc, result);
                self.decided(key, *decision)
            }
            (Pending::Question { rpc, ids }, Answer::Question { answers }) => {
                let mut map = Map::new();
                for (id, answer) in ids.iter().zip(answers) {
                    if let Some(answer) = answer.as_ref().filter(|answer| !answer.is_empty()) {
                        map.insert(id.clone(), json!({ "answers": answer }));
                    }
                }
                self.reply(rpc, json!({ "answers": map }));
                self.answered(key, answers)
            }
            (Pending::AsyncQuestion { texts }, Answer::Question { answers }) => {
                let parts: Vec<String> = texts
                    .iter()
                    .zip(answers)
                    .filter_map(|(text, answer)| {
                        let answer = answer.as_ref().filter(|answer| !answer.is_empty())?;
                        let quoted: Vec<String> =
                            text.lines().map(|line| format!("> {line}")).collect();
                        Some(format!("{}\n\n{}", quoted.join("\n"), answer.join(", ")))
                    })
                    .collect();
                if !parts.is_empty() {
                    self.messages.push(parts.join("\n\n"));
                }
                self.answered(key, answers)
            }
            (pending, _) => {
                tracing::warn!(key, "an answer of the wrong kind for a Codex request");
                self.pending.insert(key.to_owned(), pending);
                Vec::new()
            }
        }
    }

    fn decided(&mut self, key: &str, decision: Decision) -> Vec<EngineEvent> {
        let Some(tracked) = self.items.get_mut(key) else {
            return Vec::new();
        };
        if let ItemKind::Tool {
            approval: Some(approval),
            ..
        } = &mut tracked.kind
        {
            approval.decision = Some(decision);
        }
        tracked.status = tracked.status.advance(match decision {
            Decision::AllowOnce | Decision::AllowAlways => Status::Running,
            Decision::DenyOnce | Decision::DenyAlways => Status::Declined,
            Decision::Cancelled => Status::Cancelled,
        });
        vec![self.event(key)]
    }

    fn answered(&mut self, key: &str, given: &[Option<Vec<String>>]) -> Vec<EngineEvent> {
        let Some(tracked) = self.items.get_mut(key) else {
            return Vec::new();
        };
        if let ItemKind::Question { answers, .. } = &mut tracked.kind {
            *answers = Some(given.to_vec());
        }
        tracked.status = tracked.status.advance(Status::Done);
        vec![self.event(key)]
    }

    /// Codex settled a request without us (the turn ended, or it was
    /// auto-reviewed): the item stops waiting.
    fn resolved(&mut self, rpc: &Value, out: &mut Vec<EngineEvent>) {
        let key = self
            .pending
            .iter()
            .find_map(|(key, pending)| match pending {
                Pending::Command { rpc: id }
                | Pending::FileChange { rpc: id }
                | Pending::Permissions { rpc: id, .. }
                | Pending::Question { rpc: id, .. }
                    if id == rpc =>
                {
                    Some(key.clone())
                }
                _ => None,
            });
        let Some(key) = key else { return };
        self.pending.remove(&key);
        if let Some(tracked) = self.items.get_mut(&key)
            && tracked.status == Status::AwaitingUser
        {
            tracked.status = Status::Running;
            out.push(self.event(&key));
        }
    }

    fn reply(&mut self, rpc: Value, result: Value) {
        self.outbox.push(json!({ "id": rpc, "result": result }));
    }

    /// Settles what is still open when the run ends: plans keep their
    /// steps, everything else stopped with the run.
    pub fn finish(&mut self) -> Vec<EngineEvent> {
        self.pending.clear();
        let mut keys: Vec<String> = self
            .items
            .iter()
            .filter(|(_, tracked)| !tracked.status.is_terminal())
            .map(|(key, _)| key.clone())
            .collect();
        keys.sort();
        keys.into_iter()
            .map(|key| {
                let status = if matches!(self.items[&key].kind, ItemKind::Plan { .. }) {
                    Status::Done
                } else {
                    Status::Cancelled
                };
                if let Some(tracked) = self.items.get_mut(&key) {
                    tracked.status = status;
                }
                EngineEvent::Status { key, status }
            })
            .collect()
    }

    fn track(&mut self, key: String, parent: Scope, status: Status, kind: ItemKind) {
        match self.items.get_mut(&key) {
            Some(tracked) => {
                tracked.status = tracked.status.advance(status);
                tracked.kind = kind;
            }
            None => {
                self.items.insert(
                    key,
                    Tracked {
                        parent,
                        status,
                        kind,
                    },
                );
            }
        }
    }

    fn event(&self, key: &str) -> EngineEvent {
        let tracked = &self.items[key];
        EngineEvent::Item {
            key: key.to_owned(),
            parent: tracked.parent.clone(),
            status: tracked.status,
            kind: tracked.kind.clone(),
        }
    }
}

fn live(target: LiveKind, text: &str) -> EngineEvent {
    EngineEvent::Live {
        target,
        text: text.to_owned(),
        append: true,
    }
}

fn str_at<'a>(value: &'a Value, field: &str) -> Option<&'a str> {
    value.get(field)?.as_str()
}

fn goal_key(scope: Option<&String>) -> String {
    scope.map_or_else(|| "goal".to_owned(), |agent| format!("goal:{agent}"))
}

/// Policy blocks (`cyberPolicy`, `misalignmentPolicyViolation`, …).
fn is_refusal(error: &Value) -> bool {
    str_at(error, "codexErrorInfo")
        .is_some_and(|code| code.ends_with("Policy") || code.ends_with("PolicyViolation"))
}

fn tool_status(item: &Value, completed: bool) -> Status {
    match str_at(item, "status") {
        Some("failed") => Status::Failed,
        Some("completed") => Status::Done,
        _ if completed => Status::Done,
        _ => Status::Running,
    }
}

fn user_text(item: &Value) -> String {
    item["content"]
        .as_array()
        .map(|parts| {
            parts
                .iter()
                .filter_map(|part| str_at(part, "text"))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

/// Tool arguments as compact JSON; nothing for an empty call.
fn arguments(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::Object(map) if map.is_empty() => None,
        Value::String(text) => Some(text.clone()),
        other => Some(other.to_string()),
    }
}

/// The text parts of MCP content (or dynamic tool output) joined.
fn content_text(content: &Value) -> Option<String> {
    let text: Vec<&str> = content
        .as_array()?
        .iter()
        .filter_map(|part| str_at(part, "text"))
        .collect();
    (!text.is_empty()).then(|| text.join("\n"))
}

fn file_change(change: &Value) -> Option<FileChange> {
    let path = str_at(change, "path")?.to_owned();
    let kind = &change["kind"];
    let (path, op) = match str_at(kind, "type")? {
        "add" => (path, FileOp::Add),
        "delete" => (path, FileOp::Delete),
        _ => match str_at(kind, "move_path") {
            Some(to) => (to.to_owned(), FileOp::Move { from: path }),
            None => (path, FileOp::Modify),
        },
    };
    let diff = str_at(change, "diff")
        .filter(|diff| !diff.is_empty())
        .map(str::to_owned);
    Some(FileChange { path, op, diff })
}

fn question(question: &Value) -> Option<Question> {
    Some(Question {
        id: str_at(question, "id")?.to_owned(),
        header: str_at(question, "header")
            .filter(|header| !header.is_empty())
            .map(str::to_owned),
        text: str_at(question, "question")?.to_owned(),
        multi_select: false,
        options: question["options"]
            .as_array()
            .map(|options| {
                options
                    .iter()
                    .filter_map(|option| {
                        Some(QuestionOption {
                            label: str_at(option, "label")?.to_owned(),
                            description: str_at(option, "description")
                                .filter(|text| !text.is_empty())
                                .map(str::to_owned),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default(),
    })
}

fn strip_nulls(value: &Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .filter(|(_, value)| !value.is_null())
                .map(|(key, value)| (key.clone(), strip_nulls(value)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// The command as the model wrote it: Codex wraps it in `<shell> -lc '…'`.
fn display_command(command: &str) -> String {
    let unwrapped = command.split_once(' ').and_then(|(shell, rest)| {
        let shell_name = shell.rsplit('/').next()?;
        if !shell_name.ends_with("sh") {
            return None;
        }
        rest.strip_prefix("-lc ")
            .or_else(|| rest.strip_prefix("-c "))
    });
    let Some(inner) = unwrapped else {
        return command.to_owned();
    };
    for quote in ['\'', '"'] {
        if let Some(body) = inner
            .strip_prefix(quote)
            .and_then(|inner| inner.strip_suffix(quote))
            && !body.contains(quote)
        {
            return body.to_owned();
        }
    }
    inner.to_owned()
}
