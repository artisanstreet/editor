//! Pure decoding of `codex app-server` JSON-RPC (one JSON object per line).
//!
//! The decoder follows the run's root thread plus every child thread its
//! agents spawn. Each child thread becomes an `Agent` item (keyed by the
//! `spawnAgent` call) and everything the child does is decoded the same way
//! with `parent` set to that item. It also keeps the JSON-RPC bookkeeping:
//! request ids, responses for the driver, and replies to Codex's own
//! requests (approvals, questions). Shapes were captured from Codex 0.162.1
//! (see `transcripts/codex`) and its generated app-server schema.

use super::notices::NativeNotice;
use super::required_items::native_tool_status;
use super::required_items::{
    AssistantText, CollabCall, CollabStatus, CollabTool, CompletedTurn, Elicitation,
    ErrorNotification, MessagePhase, NativeFileChange, StartedTurn, TerminalStatus, ThreadIdentity,
    TokenUsage, UserInputRequest, user_text, validate_consumed_items, validate_elicitation,
    validate_user_input_request,
};
use crate::{EngineEvent, LiveKind, Outcome};
use anyhow::Context as _;
use artisan_api::{
    Answer, Approval, ApprovalRequest, Decision, ErrorKind, FileChange, ItemKind, NoticeKind,
    Output, Phase, PlanStep, Question, QuestionOption, RunStatus, Status, StepStatus, ToolKind,
    Usage,
};
use serde_json::{Map, Value, json};
use std::collections::{HashMap, HashSet};

#[derive(serde::Deserialize)]
struct NativeItemIdentity<'a> {
    id: &'a str,
    #[serde(rename = "type")]
    kind: &'a str,
}

/// The key of the run's plan (`turn/plan/updated`); one per run.
const PLAN_KEY: &str = "plan";

pub(crate) use crate::rpc::Response;

#[derive(Default)]
pub(crate) struct Decoder {
    ownership: crate::retained::Ownership,
    reasoning_summary: crate::reasoning::Summary,
    rpc: crate::rpc::State,
    citations: super::citations::Citations,
    root: Option<String>,
    /// Followed child threads → the `Agent` item their items belong to.
    children: HashMap<String, String>,
    /// Spawn calls awaiting an explicit receiver thread relation.
    spawning: Vec<String>,
    deferred: HashMap<String, Vec<(String, Value)>>,
    deferred_bytes: usize,
    /// Child thread → text of its last agent message, the agent's result.
    child_replies: HashMap<String, artisan_api::TextContent>,
    items: HashMap<String, Tracked>,
    /// Item key → the Codex request waiting for the user.
    pending: HashMap<String, Pending>,
    approval_targets: HashMap<String, String>,
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

#[derive(serde::Serialize)]
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
        self.rpc.request(method, params)
    }

    pub fn notify(&mut self, method: &str) {
        self.rpc.notify(method, None);
    }

    pub fn take_outbox(&mut self) -> Vec<Value> {
        std::mem::take(&mut self.rpc.outbox)
    }

    pub fn take_responses(&mut self) -> Vec<Response> {
        std::mem::take(&mut self.rpc.responses)
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
    pub fn feed(&mut self, line: &str) -> anyhow::Result<Vec<EngineEvent>> {
        let message: Value = serde_json::from_str(line).context("decoding Codex protocol JSON")?;
        anyhow::ensure!(
            message.is_object(),
            "Codex protocol frame must be an object"
        );
        anyhow::ensure!(
            message["method"].is_string() || message.get("id").is_some(),
            "native protocol frame is missing its required envelope identity"
        );
        // Consumed notice payloads must validate before scope/deferred ownership
        // or canonical Items change. Missing threadId cannot silently bypass this.
        if message.get("id").is_none()
            && let Some(method) = message["method"].as_str()
        {
            NativeNotice::parse(method, &message["params"])?;
            validate_consumed_items(method, &message["params"])?;
        }
        if message["method"].as_str() == Some("mcpServer/elicitation/request") {
            validate_elicitation(&message["params"])?;
        }
        if message["method"].as_str() == Some("item/tool/requestUserInput") {
            validate_user_input_request(&message["params"])?;
        }
        let pending_frame = (message.get("id").is_some()
            && matches!(
                message["method"].as_str(),
                Some(
                    "item/commandExecution/requestApproval"
                        | "item/fileChange/requestApproval"
                        | "item/permissions/requestApproval"
                        | "item/tool/requestUserInput"
                )
            ))
            || message["params"]["item"]["questions"].is_array();
        if pending_frame {
            crate::retained::admit_pending(self.pending.len(), self.ownership.bytes(), &message)?;
        }
        let pending_before = self.pending.len();
        let events = self.feed_raw(&message);
        let events = self.reasoning_summary.normalize(events, false);
        let events = self.citations.normalize(events);
        // Output order guarantees that Forge persists the Inline event before a
        // later callback can reuse its immutable source descriptor.
        for key in crate::retained::changed_keys(&events) {
            if let Some(tracked) = self.items.get_mut(key) {
                if tracked.status.is_terminal() {
                    crate::retained::compact_terminal(&mut tracked.kind)?;
                }
                self.ownership
                    .replace(key, crate::retained::owned_charge(&tracked.kind)?);
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

    fn feed_raw(&mut self, message: &Value) -> Vec<EngineEvent> {
        let mut out = Vec::new();
        match (message["method"].as_str(), message.get("id")) {
            (Some(method), Some(id)) => {
                self.server_request(id.clone(), method, &message["params"], &mut out)
            }
            (Some(method), None) => self.notification(method, &message["params"], &mut out),
            (None, Some(_)) => self.response(message, &mut out),
            (None, None) => {}
        }
        out
    }

    fn response(&mut self, message: &Value, out: &mut Vec<EngineEvent>) {
        let Some(Response { id, method, result }) = self.rpc.response(message) else {
            return;
        };
        let value = result.as_ref().ok();
        let empty = Value::Null;
        let value = value.unwrap_or(&empty);
        // `thread/start` and `thread/resume` answer with the thread;
        // `turn/start` with the turn.
        if let Some(thread) = value["thread"]["id"].as_str() {
            self.root = Some(thread.to_owned());
            out.push(EngineEvent::Session {
                id: thread.to_owned(),
            });
            if let Some(model) = str_at(value, "model") {
                out.push(EngineEvent::Model {
                    id: model.to_owned(),
                });
            }
        }
        if let Some(turn) = value["turn"]["id"].as_str() {
            self.turn = Some(turn.to_owned());
        }
        self.rpc.responses.push(Response { id, method, result });
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
        None
    }

    fn defer(&mut self, thread: &str, method: &str, params: &Value) {
        let bytes = method.len() + params.to_string().len();
        if self.deferred_bytes + bytes > 4 * 1024 * 1024
            || self.deferred.values().map(Vec::len).sum::<usize>() >= 256
        {
            let message =
                "Codex omitted child spawn relations and exceeded the pending-event budget";
            tracing::warn!(thread, method, message);
            self.turn_end = Some(Outcome::failed(message));
            self.deferred.clear();
            self.deferred_bytes = 0;
            return;
        }
        self.deferred_bytes += bytes;
        self.deferred
            .entry(thread.to_owned())
            .or_default()
            .push((method.to_owned(), params.clone()));
    }

    fn bind_child(&mut self, thread: &str, agent: &str) {
        self.spawning.retain(|key| key != agent);
        self.children.insert(thread.to_owned(), agent.to_owned());
    }

    fn notification(&mut self, method: &str, params: &Value, out: &mut Vec<EngineEvent>) {
        if let Err(error) = validate_consumed_items(method, params) {
            tracing::warn!(method, %error, "invalid Codex item payload");
            return;
        }
        // Deferred child notices were validated at ingress. Parse the same
        // borrowed fields when replaying them, without retaining another model.
        let notice = match NativeNotice::parse(method, params) {
            Ok(notice) => notice,
            Err(error) => {
                tracing::warn!(method, %error, "invalid Codex notice");
                return;
            }
        };
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
        let Some(thread) = params["threadId"].as_str() else {
            return;
        };
        let Some(scope) = self.scope(params) else {
            if !self.spawning.is_empty()
                && let Some(thread) = params["threadId"].as_str()
            {
                self.defer(thread, method, params);
            }
            return;
        };
        let item_key = || {
            params["itemId"]
                .as_str()
                .filter(|key| !key.is_empty())
                .map(str::to_owned)
        };
        match method {
            "turn/started" if scope.is_none() => {
                let Ok(started) = <StartedTurn as serde::Deserialize>::deserialize(params) else {
                    return;
                };
                self.turn = Some(started.turn.id.to_owned());
                self.unanswered = false;
            }
            "turn/completed" => self.turn_completed(scope, params, out),
            "item/started" => self.item(thread, scope, &params["item"], false, out),
            "item/completed" => self.item(thread, scope, &params["item"], true, out),
            "item/agentMessage/delta" | "item/plan/delta" => {
                if let Some(delta) = str_at(params, "delta") {
                    out.push(live(
                        LiveKind::Text {
                            key: {
                                let Some(key) = item_key() else {
                                    tracing::warn!(method, "Codex delta omitted itemId");
                                    return;
                                };
                                key
                            },
                        },
                        delta,
                    ));
                }
            }
            "item/reasoning/summaryTextDelta" if scope.is_none() => {
                if let Some(delta) = str_at(params, "delta") {
                    out.push(live(LiveKind::Reasoning, delta));
                }
            }
            "item/reasoning/summaryPartAdded" if scope.is_none() => {
                match params["summaryIndex"].as_u64() {
                    Some(0) => out.push(EngineEvent::Live {
                        target: LiveKind::Reasoning,
                        text: String::new(),
                        append: false,
                    }),
                    Some(_) => out.push(live(LiveKind::Reasoning, "\n\n")),
                    None => tracing::warn!("Codex reasoning part omitted summaryIndex"),
                }
            }
            "item/commandExecution/outputDelta" => {
                if let Some(delta) = str_at(params, "delta") {
                    out.push(live(
                        LiveKind::Shell {
                            key: {
                                let Some(key) = item_key() else {
                                    tracing::warn!(method, "Codex delta omitted itemId");
                                    return;
                                };
                                key
                            },
                        },
                        delta,
                    ));
                }
            }
            "item/mcpToolCall/progress" => {
                if let Some(message) = str_at(params, "message") {
                    out.push(EngineEvent::Live {
                        target: LiveKind::Progress {
                            key: {
                                let Some(key) = item_key() else {
                                    tracing::warn!(method, "Codex delta omitted itemId");
                                    return;
                                };
                                key
                            },
                        },
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
                let Some(NativeNotice::Rerouted(notice)) = notice else {
                    unreachable!("validated reroute")
                };
                let text = format!(
                    "Codex moved this turn from {} to {} ({}).",
                    notice.from_model,
                    notice.to_model,
                    notice.reason.as_str()
                );
                self.notice(
                    format!("reroute:{}", notice.turn_id),
                    scope,
                    NoticeKind::Reroute {
                        model: notice.to_model.to_owned(),
                    },
                    text,
                    out,
                );
            }
            "model/safetyBuffering/updated" => {
                let Some(NativeNotice::SafetyBuffering(notice)) = notice else {
                    unreachable!("validated safety notification")
                };
                let key = format!("safety:{}", notice.turn_id);
                if notice.show_buffering_ui && !self.items.contains_key(&key) {
                    self.notice(
                        key,
                        scope,
                        NoticeKind::Safety,
                        "Codex is holding this reply for a safety check.".to_owned(),
                        out,
                    );
                }
            }
            "thread/goal/updated" => {
                let Some(NativeNotice::GoalUpdated(notice)) = notice else {
                    unreachable!("validated goal update")
                };
                self.notice(
                    goal_key(scope.as_ref()),
                    scope,
                    NoticeKind::Goal {
                        state: notice.goal.status.as_str().to_owned(),
                    },
                    notice.goal.objective.to_owned(),
                    out,
                );
            }
            "thread/goal/cleared" => {
                let key = goal_key(scope.as_ref());
                // Codex clears at every turn, goal or not: only a goal this
                // session showed is reported cleared.
                let text = match self.items.get(&key).map(|item| &item.kind) {
                    Some(ItemKind::Notice { text, .. }) if text.byte_len() > 0 => text.clone(),
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
            _ => tracing::debug!(method, "ignored unsupported Codex notification"),
        }
    }

    fn thread_started(&mut self, thread: &Value, out: &mut Vec<EngineEvent>) {
        let Ok(started) = <ThreadIdentity as serde::Deserialize>::deserialize(thread) else {
            return;
        };
        let id = started.id;
        let Some(parent) = started.parent_thread_id else {
            return;
        };
        if self.root.as_deref() == Some(id) {
            return;
        }
        if let Some(agent) = self.children.get(id).cloned() {
            if let Some(tracked) = self.items.get_mut(&agent)
                && let ItemKind::Agent { name, agent_id, .. } = &mut tracked.kind
            {
                let nickname = started.agent_nickname.map(str::to_owned);
                let changed =
                    agent_id.as_deref() != Some(id) || name.is_none() && nickname.is_some();
                *agent_id = Some(id.to_owned());
                if name.is_none() {
                    *name = nickname;
                }
                if changed {
                    out.push(self.event(&agent));
                }
            }
            return;
        }
        let parent_scope = if self.root.as_deref() == Some(parent) {
            None
        } else if let Some(agent) = self.children.get(parent) {
            Some(agent.clone())
        } else {
            return;
        };
        let name = started.agent_nickname.map(str::to_owned);
        if !self.spawning.is_empty() {
            self.defer(id, "thread/started", &json!({ "thread": thread }));
            return;
        }
        let agent = id.to_owned();
        let kind = ItemKind::Agent {
            name: name.clone(),
            description: (String::new()).into(),
            prompt: None,
            agent_id: Some(id.to_owned()),
            context_used: None,
            model: None,
            background: false,
            result: None,
            label: None,
        };
        self.track(agent.clone(), parent_scope, Status::Running, kind);
        out.push(self.event(&agent));
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
        let Ok(completed) = <CompletedTurn as serde::Deserialize>::deserialize(params) else {
            return; // Ingress validates before any decoder state changes.
        };
        let turn = &params["turn"];
        let error = &turn["error"];
        let message = str_at(error, "message").map(str::to_owned);
        match scope {
            None => {
                self.turn = None;
                let outcome = match completed.turn.status {
                    TerminalStatus::Interrupted => Outcome::cancelled(),
                    TerminalStatus::Failed => {
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
                    TerminalStatus::Completed => {
                        self.stranded = std::mem::take(&mut self.unanswered);
                        Outcome::done()
                    }
                };
                self.unanswered = false;
                self.last_error = None;
                self.turn_end = Some(outcome);
            }
            Some(agent) => {
                let status = match completed.turn.status {
                    TerminalStatus::Interrupted => Status::Cancelled,
                    TerminalStatus::Failed => Status::Failed,
                    TerminalStatus::Completed => Status::Done,
                };
                let thread = completed.thread_id;
                let reply = self
                    .child_replies
                    .remove(thread)
                    .or_else(|| message.map(Into::into));
                self.ownership.remove_buffer(thread);
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

    fn item(
        &mut self,
        thread: &str,
        scope: Scope,
        item: &Value,
        completed: bool,
        out: &mut Vec<EngineEvent>,
    ) {
        let identity = <NativeItemIdentity as serde::Deserialize>::deserialize(item);
        let Ok(NativeItemIdentity {
            id,
            kind: item_type,
        }) = identity
        else {
            tracing::warn!("Codex item omitted its required native identity/type");
            return;
        };
        if id.is_empty() || item_type.is_empty() {
            tracing::warn!("Codex item supplied an empty native identity/type");
            return;
        }
        let key = id.to_owned();
        if item_type == "dynamicToolCall"
            && self
                .items
                .get(&key)
                .is_some_and(|tracked| matches!(&tracked.kind, ItemKind::Question { .. }))
        {
            if completed {
                let Some(status) = native_tool_status(item, completed) else {
                    return;
                };
                self.pending.remove(&key);
                if let Some(tracked) = self.items.get_mut(&key) {
                    let next = tracked.status.advance(status);
                    if next != tracked.status {
                        tracked.status = next;
                        out.push(EngineEvent::Status { key, status: next });
                    }
                }
            }
            return;
        }
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
                let Ok(message) = <AssistantText as serde::Deserialize>::deserialize(item) else {
                    return;
                };
                let text = message.text.to_owned();
                let phase = match message.phase {
                    Some(MessagePhase::Commentary) => Phase::Commentary,
                    Some(MessagePhase::PartialAnswer | MessagePhase::FinalAnswer) | None => {
                        Phase::Final
                    }
                };
                if completed {
                    if scope.is_some() {
                        self.remember_reply(thread, &text);
                    }
                    self.async_questions(&key, scope.clone(), item, out);
                }
                (
                    settled(true),
                    ItemKind::AssistantMessage {
                        text: text.into(),
                        phase,
                    },
                )
            }
            "plan" => {
                let Some(text) = str_at(item, "text") else {
                    tracing::warn!(id, item_type, "Codex text item omitted text");
                    return;
                };
                let text = text.to_owned();
                (
                    settled(true),
                    ItemKind::AssistantMessage {
                        text: text.into(),
                        phase: Phase::Final,
                    },
                )
            }
            "userMessage" => {
                // Ours are stored by Forge. A child's first message is its
                // task; keep it when the spawn call didn't carry one.
                if let Some(agent) = &scope
                    && let Some(tracked) = self.items.get_mut(agent)
                    && let ItemKind::Agent { prompt, .. } = &mut tracked.kind
                    && prompt.is_none()
                {
                    if let Some(text) = user_text(item) {
                        *prompt = Some(text.into());
                        out.push(self.event(agent));
                    }
                }
                return;
            }
            "commandExecution" => {
                let Some(status) = native_tool_status(item, completed) else {
                    return;
                };
                let tool = ToolKind::Shell {
                    command: ({
                        let Some(command) =
                            str_at(item, "command").filter(|value| !value.trim().is_empty())
                        else {
                            if !completed {
                                return;
                            }
                            self.error_item(
                                scope,
                                ErrorKind::Execution,
                                "Codex commandExecution omitted its command",
                                out,
                            );
                            return;
                        };
                        display_command(command)
                    })
                    .into(),
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
                let Some(status) = native_tool_status(item, completed) else {
                    return;
                };
                let Some(native_changes) = item["changes"].as_array() else {
                    if !completed {
                        return;
                    }
                    self.error_item(
                        scope,
                        ErrorKind::Execution,
                        "Codex fileChange omitted its changes",
                        out,
                    );
                    return;
                };
                let Some(changes) = native_changes
                    .iter()
                    .map(file_change)
                    .collect::<Option<Vec<_>>>()
                else {
                    if !completed {
                        return;
                    }
                    self.error_item(
                        scope,
                        ErrorKind::Execution,
                        "Codex fileChange supplied an invalid file change",
                        out,
                    );
                    return;
                };
                (
                    status,
                    self.tool_kind(
                        &key,
                        ToolKind::Edit {
                            changes: changes.into(),
                        },
                        None,
                        None,
                    ),
                )
            }
            "mcpToolCall" => {
                let Some(status) = native_tool_status(item, completed) else {
                    return;
                };
                let (Some(server), Some(tool_name)) = (
                    str_at(item, "server").filter(|value| !value.is_empty()),
                    str_at(item, "tool").filter(|value| !value.is_empty()),
                ) else {
                    tracing::warn!(id, "Codex MCP call omitted server/tool identity");
                    return;
                };
                let tool = ToolKind::Mcp {
                    server: server.to_owned(),
                    tool: tool_name.to_owned(),
                    input: (arguments(&item["arguments"])).map(Into::into),
                };
                let output = str_at(&item["error"], "message")
                    .map(str::to_owned)
                    .or_else(|| content_text(&item["result"]["content"]))
                    .map(|text| Output::from_text(&text));
                (status, self.tool_kind(&key, tool, output, None))
            }
            "dynamicToolCall" => {
                let Some(status) = native_tool_status(item, completed) else {
                    return;
                };
                let tool = ToolKind::Other {
                    name: {
                        let Some(name) =
                            str_at(item, "tool").filter(|value| !value.trim().is_empty())
                        else {
                            if !completed {
                                return;
                            }
                            self.error_item(
                                scope,
                                ErrorKind::Execution,
                                "Codex dynamicToolCall omitted its tool name",
                                out,
                            );
                            return;
                        };
                        name.to_owned()
                    },
                    input: (arguments(&item["arguments"])).map(Into::into),
                };
                let output =
                    content_text(&item["contentItems"]).map(|text| Output::from_text(&text));
                (status, self.tool_kind(&key, tool, output, None))
            }
            "webSearch" => {
                let action = &item["action"];
                let query = [
                    str_at(item, "query"),
                    str_at(action, "query"),
                    str_at(action, "url"),
                ]
                .into_iter()
                .flatten()
                .find(|value| !value.trim().is_empty())
                .map(str::to_owned)
                .or_else(|| {
                    let queries = action["queries"].as_array()?;
                    let queries: Vec<_> = queries
                        .iter()
                        .filter_map(Value::as_str)
                        .filter(|query| !query.trim().is_empty())
                        .collect();
                    (!queries.is_empty()).then(|| queries.join("; "))
                })
                .or_else(|| {
                    match self.items.get(&key).map(|tracked| &tracked.kind) {
                        Some(ItemKind::Tool {
                            tool: ToolKind::WebSearch { query },
                            ..
                        }) => Some(query.clone()),
                        _ => None,
                    }
                });
                let Some(query) = query else {
                    // Codex starts searches without metadata and can complete
                    // an opaque action as query:"", action:{type:"other"}.
                    // Neither is an execution failure. Wait for useful identity
                    // or omit this optional card; native error events stay errors.
                    tracing::debug!(id, completed, "Codex web action has no display metadata");
                    return;
                };
                (
                    settled(true),
                    self.tool_kind(&key, ToolKind::WebSearch { query }, None, None),
                )
            }
            "imageView" => {
                let Some(path) = str_at(item, "path").filter(|value| !value.trim().is_empty())
                else {
                    if !completed {
                        return;
                    }
                    self.error_item(
                        scope,
                        ErrorKind::Execution,
                        "Codex imageView omitted its path",
                        out,
                    );
                    return;
                };
                let path = path.to_owned();
                (
                    settled(true),
                    self.tool_kind(&key, ToolKind::ViewImage { path }, None, None),
                )
            }
            "imageGeneration" => {
                if !completed {
                    return;
                }
                if let Some(path) = str_at(item, "savedPath").filter(|path| !path.is_empty()) {
                    out.push(EngineEvent::Image {
                        key,
                        parent: scope,
                        path: path.into(),
                        caption: str_at(item, "revisedPrompt").map(str::to_owned),
                    });
                } else {
                    tracing::warn!(item = %key, "Codex completed image generation without an image path");
                }
                return;
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
        if completed {
            self.pending.remove(&key);
        }
        let status = if !completed && self.pending.contains_key(&key) {
            Status::AwaitingUser
        } else {
            status
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
        let Ok(call) = <CollabCall as serde::Deserialize>::deserialize(item) else {
            return; // Ingress validates before tracking or binding agents.
        };
        let receivers = call.receiver_thread_ids;
        if !matches!(call.tool, CollabTool::SpawnAgent) {
            // `wait` reports what the agents said; fill results we missed.
            for (thread, state) in call.agents_states {
                let Some(agent) = self.children.get(thread).cloned() else {
                    continue;
                };
                let Some(message) = state.message else {
                    continue;
                };
                if let Some(tracked) = self.items.get_mut(&agent)
                    && let ItemKind::Agent {
                        result: result @ None,
                        ..
                    } = &mut tracked.kind
                {
                    *result = Some(message.into());
                    out.push(self.event(&agent));
                }
            }
            return;
        }
        if !self.items.contains_key(&key) {
            let kind = ItemKind::Agent {
                name: None,
                description: call.description.unwrap_or_default().to_owned().into(),
                prompt: call.prompt.map(Into::into),
                agent_id: None,
                context_used: None,
                model: None,
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
                if let Some(frames) = self.deferred.remove(thread) {
                    for (method, params) in frames {
                        self.deferred_bytes = self
                            .deferred_bytes
                            .saturating_sub(method.len() + params.to_string().len());
                        self.notification(&method, &params, out);
                    }
                }
            }
        }
        let terminal = match call.status {
            CollabStatus::Failed => Some(Status::Failed),
            CollabStatus::Interrupted => Some(Status::Cancelled),
            CollabStatus::InProgress | CollabStatus::Completed => None,
        };
        if let Some(status) = terminal.filter(|_| completed) {
            self.spawning.retain(|spawn| *spawn != key);
            if let Some(tracked) = self.items.get_mut(&key) {
                tracked.status = tracked.status.advance(status);
            }
            out.push(self.event(&key));
        }
    }

    fn remember_reply(&mut self, thread: &str, text: &str) {
        if !text.is_empty() {
            let mut retained: artisan_api::TextContent = text.into();
            crate::retained::compact_text(&mut retained);
            self.ownership
                .replace_buffer(thread, retained.resident_bytes().saturating_mul(4));
            self.child_replies.insert(thread.to_owned(), retained);
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
        // Codex agentMessage uses null when its optional questions are absent.
        let Some(native) = item.get("questions").filter(|value| !value.is_null()) else {
            return;
        };
        #[derive(serde::Deserialize)]
        struct NativeQuestion {
            title: String,
            #[serde(default)]
            options: Vec<String>,
        }
        let native: Vec<NativeQuestion> = match serde_json::from_value(native.clone()) {
            Ok(native) => native,
            Err(error) => {
                self.error_item(
                    scope,
                    ErrorKind::Execution,
                    &format!("invalid Codex asynchronous questions: {error}"),
                    out,
                );
                return;
            }
        };
        if native.iter().any(|question| {
            question.title.trim().is_empty()
                || question.options.iter().any(|label| label.trim().is_empty())
        }) {
            self.error_item(
                scope,
                ErrorKind::Execution,
                "Codex asynchronous questions require nonempty prompts and option labels",
                out,
            );
            return;
        }
        if native.is_empty() {
            return;
        }
        let questions: Vec<Question> = native
            .into_iter()
            .enumerate()
            .map(|(index, question)| Question {
                id: index.to_string(),
                header: None,
                text: (question.title).into(),
                multi_select: false,
                options: question
                    .options
                    .into_iter()
                    .enumerate()
                    .map(|(index, label)| QuestionOption {
                        id: format!("option-{index}"),
                        label,
                        description: None,
                    })
                    .collect::<Vec<_>>()
                    .into(),
            })
            .collect();
        let key = format!("{key}:questions");
        let texts = questions
            .iter()
            .map(|question| {
                question
                    .text
                    .inline()
                    .expect("adapter question is inline before storage")
                    .to_owned()
            })
            .collect();
        self.pending
            .insert(key.clone(), Pending::AsyncQuestion { texts });
        let kind = ItemKind::Question {
            blocking: false,
            questions: questions.into(),
            answers: None,
            answer_text: None,
        };
        self.track(key.clone(), scope, Status::AwaitingUser, kind);
        out.push(self.event(&key));
    }

    fn plan(&mut self, scope: Scope, params: &Value, out: &mut Vec<EngineEvent>) {
        #[derive(serde::Deserialize)]
        struct Entry {
            step: String,
            status: PlanStatus,
        }
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "camelCase")]
        enum PlanStatus {
            Pending,
            InProgress,
            Completed,
        }
        let entries = match <Vec<Entry> as serde::Deserialize>::deserialize(&params["plan"]) {
            Ok(entries) if entries.iter().all(|entry| !entry.step.trim().is_empty()) => entries,
            _ => {
                self.error_item(
                    scope,
                    ErrorKind::Execution,
                    "Codex supplied malformed plan steps",
                    out,
                );
                return;
            }
        };
        let steps = entries
            .into_iter()
            .map(|entry| PlanStep {
                text: (entry.step).into(),
                status: match entry.status {
                    PlanStatus::Pending => StepStatus::Pending,
                    PlanStatus::InProgress => StepStatus::InProgress,
                    PlanStatus::Completed => StepStatus::Done,
                },
            })
            .collect::<Vec<_>>();
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
                steps: steps.into(),
                proposal: None,
                approval: None,
            },
        );
        out.push(self.event(&key));
    }

    fn token_usage(&mut self, root: bool, usage: &Value, out: &mut Vec<EngineEvent>) {
        // `last` is one model call; `total` is the whole thread, history
        // included on resume, so the run sums `last`.
        let Ok(usage) = <TokenUsage as serde::Deserialize>::deserialize(usage) else {
            return;
        };
        let last = &usage.last;
        let cached = last.cached_input_tokens;
        self.usage.input += last.input_tokens.saturating_sub(cached);
        self.usage.cache_read += cached;
        self.usage.output += last.output_tokens;
        self.usage.reasoning += last.reasoning_output_tokens;
        self.usage.cache_write =
            Some(self.usage.cache_write.unwrap_or_default() + last.cache_write_input_tokens);
        if root {
            self.usage.context_used = Some(last.total_tokens);
            if let Some(window) = usage.model_context_window {
                self.usage.context_window = Some(window);
            }
        }
        out.push(EngineEvent::Usage(self.usage.clone()));
    }

    fn error(&mut self, scope: Scope, params: &Value, out: &mut Vec<EngineEvent>) {
        let Ok(notification) = <ErrorNotification as serde::Deserialize>::deserialize(params)
        else {
            return;
        };
        let error = &params["error"];
        let message = notification.error.message.to_owned();
        if notification.will_retry {
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
                message: (message.to_owned()).into(),
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
        text: impl Into<artisan_api::TextContent>,
        out: &mut Vec<EngineEvent>,
    ) {
        self.items.remove(&key);
        self.track(
            key.clone(),
            scope,
            Status::Done,
            ItemKind::Notice {
                kind,
                text: text.into(),
            },
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
        let item_key = str_at(params, "itemId").filter(|key| !key.trim().is_empty());
        let key = if method.ends_with("/requestApproval") {
            // RPC identity is unique even when separate callbacks share an itemId.
            format!("approval-request:{rpc}")
        } else {
            item_key.map_or_else(|| format!("request-{rpc}"), str::to_owned)
        };
        if item_key.is_none()
            && matches!(
                method,
                "item/commandExecution/requestApproval"
                    | "item/fileChange/requestApproval"
                    | "item/tool/requestUserInput"
            )
        {
            let message = "Codex request omitted its required item ID";
            self.rpc
                .outbox
                .push(json!({"jsonrpc":"2.0","id":rpc,"error":{"code":-32602,"message":message}}));
            if let Some(scope) = scope {
                self.error_item(scope, ErrorKind::Execution, message, out);
            }
            return;
        }
        match (method, scope) {
            ("item/commandExecution/requestApproval", Some(scope)) => {
                let existing = item_key.and_then(|key| self.items.get(key));
                let command = str_at(params, "command")
                    .filter(|command| !command.trim().is_empty())
                    .map(display_command)
                    .map(artisan_api::TextContent::from)
                    .or_else(|| match existing.map(|item| &item.kind) {
                        Some(ItemKind::Tool {
                            tool: ToolKind::Shell { command, .. },
                            ..
                        }) if command.byte_len() > 0 => Some(command.clone()),
                        _ => None,
                    });
                let cwd = str_at(params, "cwd").map(str::to_owned).or_else(|| {
                    match existing.map(|item| &item.kind) {
                        Some(ItemKind::Tool {
                            tool: ToolKind::Shell { cwd, .. },
                            ..
                        }) => cwd.clone(),
                        _ => None,
                    }
                });
                let stdin = str_at(params, "kind") == Some("writeStdin");
                let request = if let Some(command) = command {
                    if stdin {
                        ApprovalRequest::Action {
                            description: command
                                .inline()
                                .map(|command| format!("Send input to `{command}`"))
                                .unwrap_or_else(|| "Send input to the running command".to_owned())
                                .into(),
                        }
                    } else {
                        ApprovalRequest::Command {
                            command: command.into(),
                            cwd,
                        }
                    }
                } else {
                    ApprovalRequest::Action {
                        description: (str_at(params, "reason")
                            .filter(|reason| !reason.trim().is_empty())
                            .unwrap_or(if stdin {
                                "Send input to a running command"
                            } else {
                                "Approve command execution"
                            })
                            .to_owned())
                        .into(),
                    }
                };
                self.ask(&key, scope, item_key, request, params, out);
                self.pending.insert(key, Pending::Command { rpc });
            }
            ("item/fileChange/requestApproval", Some(scope)) => {
                self.ask(
                    &key,
                    scope,
                    item_key,
                    ApprovalRequest::FileChange,
                    params,
                    out,
                );
                self.pending.insert(key, Pending::FileChange { rpc });
            }
            ("item/permissions/requestApproval", Some(scope)) => {
                let description = str_at(params, "reason").map_or_else(
                    || format!("Grant more permissions: {}", params["permissions"]),
                    str::to_owned,
                );
                self.ask(
                    &key,
                    scope,
                    item_key,
                    ApprovalRequest::Action {
                        description: description.into(),
                    },
                    params,
                    out,
                );
                let requested = strip_nulls(&params["permissions"]);
                self.pending
                    .insert(key, Pending::Permissions { rpc, requested });
            }
            ("item/tool/requestUserInput", Some(scope)) => {
                let questions = params["questions"]
                    .as_array()
                    .and_then(|questions| {
                        questions.iter().map(question).collect::<Option<Vec<_>>>()
                    })
                    .filter(|questions| !questions.is_empty());
                let Some(questions) = questions else {
                    let message = "Codex supplied malformed user-input prompts or options";
                    self.rpc.outbox.push(
                        json!({"jsonrpc":"2.0","id":rpc,"error":{"code":-32602,"message":message}}),
                    );
                    self.error_item(scope, ErrorKind::Execution, message, out);
                    return;
                };
                let mut ids = HashSet::new();
                if questions
                    .iter()
                    .any(|question| !ids.insert(question.id.as_str()))
                {
                    let message = "Codex supplied duplicate user-input question IDs";
                    self.rpc.outbox.push(
                        json!({"jsonrpc":"2.0","id":rpc,"error":{"code":-32602,"message":message}}),
                    );
                    self.error_item(scope, ErrorKind::Execution, message, out);
                    return;
                }
                let ids = questions
                    .iter()
                    .map(|question| question.id.clone())
                    .collect();
                let Ok(request) = <UserInputRequest as serde::Deserialize>::deserialize(params)
                else {
                    return;
                };
                let blocking = request.is_blocking;
                self.items.remove(&key);
                self.track(
                    key.clone(),
                    scope,
                    Status::AwaitingUser,
                    ItemKind::Question {
                        blocking,
                        questions: questions.into(),
                        answers: None,
                        answer_text: None,
                    },
                );
                out.push(self.event(&key));
                self.pending.insert(key, Pending::Question { rpc, ids });
            }
            ("mcpServer/elicitation/request", scope) => {
                // Artisan has no form for MCP elicitation yet; say so and
                // let the server go on without the input.
                let Ok(elicitation) = <Elicitation as serde::Deserialize>::deserialize(params)
                else {
                    return;
                };
                let server = elicitation.server_name;
                let text = format!(
                    "{server} asked for input Artisan can't show yet: {}",
                    elicitation.message
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
                self.rpc.outbox.push(json!({ "id": rpc, "error": { "code": -32601, "message": format!("Artisan does not handle {method}") } }));
            }
        }
    }

    /// Each native request is distinct from the execution it gates.
    fn ask(
        &mut self,
        key: &str,
        scope: Scope,
        target: Option<&str>,
        request: ApprovalRequest,
        params: &Value,
        out: &mut Vec<EngineEvent>,
    ) {
        let approval = Approval {
            request,
            reason: (str_at(params, "reason").map(str::to_owned)).map(Into::into),
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
        if let Some(target) = target {
            self.approval_targets
                .insert(key.to_owned(), target.to_owned());
        }
        self.track(
            key.to_owned(),
            scope,
            Status::AwaitingUser,
            ItemKind::Approval {
                target: None,
                approval,
            },
        );
        out.push(self.event(key));
    }

    /// The user's answer to the approval or question on `key`.
    pub fn answer(&mut self, key: &str, answer: &Answer) -> Vec<EngineEvent> {
        let mut events = self.answer_raw(key, answer);
        if let Some(item) = self.items.get_mut(key)
            && (item.status.is_terminal()
                || matches!(
                    &item.kind,
                    ItemKind::Question {
                        answers: Some(_),
                        ..
                    }
                ))
            && let Err(error) = crate::retained::compact_terminal(&mut item.kind)
        {
            events.push(crate::retained::error(key, error));
        }
        let refreshed = (|| -> anyhow::Result<()> {
            if let Some(item) = self.items.get(key) {
                self.ownership
                    .replace(key, crate::retained::owned_charge(&item.kind)?);
            }
            self.ownership
                .set_pending(crate::retained::owned_charge(&self.pending)?);
            self.ownership.check()
        })();
        if let Err(error) = refreshed {
            events.push(crate::retained::ownership_error(key, error));
        }

        events
    }

    fn answer_raw(&mut self, key: &str, answer: &Answer) -> Vec<EngineEvent> {
        let native = if let Answer::Question { answers } = answer {
            let Some(tracked) = self.items.get(key) else {
                return Vec::new();
            };
            let ItemKind::Question { questions, .. } = &tracked.kind else {
                return Vec::new();
            };
            match crate::question::native_collection_answers(questions, answers) {
                Ok(native) => native,
                Err(message) => return vec![crate::question::invalid_answer(key, message)],
            }
        } else {
            Vec::new()
        };
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
                for (id, answer) in ids.iter().zip(&native) {
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
                    .zip(&native)
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
        if let ItemKind::Approval { approval, .. } = &mut tracked.kind {
            approval.decision = Some(decision);
        }
        tracked.status = tracked.status.advance(match decision {
            Decision::AllowOnce | Decision::AllowAlways => Status::Done,
            Decision::DenyOnce | Decision::DenyAlways => Status::Declined,
            Decision::Cancelled => Status::Cancelled,
        });
        vec![self.event(key)]
    }

    fn answered(
        &mut self,
        key: &str,
        given: &[Option<artisan_api::QuestionAnswer>],
    ) -> Vec<EngineEvent> {
        let Some(tracked) = self.items.get_mut(key) else {
            return Vec::new();
        };
        if let ItemKind::Question { answers, .. } = &mut tracked.kind {
            *answers = Some(given.to_vec().into());
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
            tracked.status = if matches!(tracked.kind, ItemKind::Approval { .. }) {
                Status::Done
            } else {
                Status::Running
            };
            out.push(self.event(&key));
        }
    }

    fn reply(&mut self, rpc: Value, result: Value) {
        self.rpc.outbox.push(json!({ "id": rpc, "result": result }));
    }

    /// Settles what is still open when the run ends: plans keep their
    /// steps, everything else stopped with the run.
    pub fn finish(&mut self) -> Vec<EngineEvent> {
        self.pending.clear();
        let mut keys: Vec<String> = self
            .items
            .iter()
            .filter(|(_, tracked)| {
                !tracked.status.is_terminal()
                    && !matches!(
                        tracked.kind,
                        ItemKind::Question {
                            blocking: false,
                            ..
                        }
                    )
            })
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
        if let ItemKind::Approval { approval, .. } = &tracked.kind {
            return EngineEvent::Approval {
                key: key.to_owned(),
                parent: tracked.parent.clone(),
                target: self.approval_targets.get(key).cloned(),
                status: tracked.status,
                approval: approval.clone(),
            };
        }
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
    let change: NativeFileChange = serde::Deserialize::deserialize(change).ok()?;
    Some(change.canonical())
}

fn question(question: &Value) -> Option<Question> {
    let options = match question.get("options") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(options)) => options
            .iter()
            .enumerate()
            .map(|(index, option)| {
                Some(QuestionOption {
                    id: format!("option-{index}"),
                    label: str_at(option, "label")
                        .filter(|label| !label.trim().is_empty())?
                        .to_owned(),
                    description: (str_at(option, "description")
                        .filter(|text| !text.is_empty())
                        .map(str::to_owned))
                    .map(Into::into),
                })
            })
            .collect::<Option<Vec<_>>>()?,
        _ => return None,
    };
    Some(Question {
        id: str_at(question, "id")
            .filter(|id| !id.trim().is_empty())?
            .to_owned(),
        text: (str_at(question, "question")
            .filter(|text| !text.trim().is_empty())?
            .to_owned())
        .into(),
        header: str_at(question, "header")
            .filter(|header| !header.is_empty())
            .map(str::to_owned),
        multi_select: false,
        options: options.into(),
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
    let Some(words) = shlex::split(command) else {
        return command.to_owned();
    };
    match words.as_slice() {
        [shell, flag, script]
            if matches!(
                std::path::Path::new(shell)
                    .file_name()
                    .and_then(|name| name.to_str()),
                Some("sh" | "bash" | "zsh" | "dash" | "fish")
            ) && matches!(flag.as_str(), "-c" | "-lc") =>
        {
            script.clone()
        }
        _ => command.to_owned(),
    }
}

#[cfg(test)]
mod required_fields_tests {
    use super::*;
    #[test]
    fn malformed_native_tools_never_become_empty_canonical_cards() {
        for kind in [
            "commandExecution",
            "dynamicToolCall",
            "imageView",
            "fileChange",
        ] {
            let mut decoder = Decoder::default();
            let mut events = Vec::new();
            decoder.item(
                "root",
                None,
                &json!({"id":"tool","type":kind,"status":"completed"}),
                true,
                &mut events,
            );
            assert!(
                events.iter().any(|event| matches!(
                    event,
                    EngineEvent::Item {
                        kind: ItemKind::Error { .. },
                        ..
                    }
                )),
                "{kind}: expected explicit malformed-input error"
            );
            assert!(
                !events.iter().any(|event| matches!(
                    event,
                    EngineEvent::Item {
                        kind: ItemKind::Tool { .. },
                        ..
                    }
                )),
                "{kind}: no empty card"
            );
        }
    }
    #[test]
    fn sparse_web_completion_keeps_known_identity_and_settles_the_tool() {
        let mut decoder = Decoder::default();
        let mut events = Vec::new();
        decoder.item(
            "root",
            None,
            &json!({
                "id":"web", "type":"webSearch", "query":"initial query", "action":null
            }),
            false,
            &mut events,
        );
        decoder.item(
            "root",
            None,
            &json!({
                "id":"web", "type":"webSearch", "query":"", "action":{"type":"other"}
            }),
            true,
            &mut events,
        );
        assert_eq!(decoder.items["web"].status, Status::Done);
        assert!(matches!(&decoder.items["web"].kind,
            ItemKind::Tool { tool: ToolKind::WebSearch { query }, .. } if query == "initial query"));
        assert!(!events.iter().any(|event| matches!(
            event,
            EngineEvent::Item {
                kind: ItemKind::Error { .. },
                ..
            }
        )));
    }

    #[test]
    fn web_action_metadata_accepts_nested_queries_and_blank_outer_labels() {
        for (action, expected) in [
            (
                json!({"type":"search", "query":null, "queries":["first", "", "second"]}),
                "first; second",
            ),
            (
                json!({"type":"openPage", "url":"https://example.test/page"}),
                "https://example.test/page",
            ),
            (
                json!({"type":"findInPage", "url":null, "pattern":"needle"}),
                "",
            ),
        ] {
            let mut decoder = Decoder::default();
            let mut events = Vec::new();
            decoder.item(
                "root",
                None,
                &json!({
                    "id":"web", "type":"webSearch", "query":"   ", "action":action
                }),
                true,
                &mut events,
            );
            if expected.is_empty() {
                assert!(events.is_empty() && decoder.items.is_empty());
            } else {
                assert!(matches!(&decoder.items["web"].kind,
                    ItemKind::Tool { tool: ToolKind::WebSearch { query }, .. } if query == expected));
            }
        }
    }

    #[test]
    fn opaque_web_completion_does_not_suppress_a_real_provider_error() {
        let mut decoder = Decoder {
            root: Some("root".into()),
            ..Decoder::default()
        };
        decoder.feed(&json!({"method":"item/completed", "params":{
            "threadId":"root", "item":{"id":"web", "type":"webSearch", "query":"", "action":{"type":"other"}, "results":null}
        }}).to_string()).unwrap();
        let events = decoder.feed(&json!({"method":"error", "params":{
            "threadId":"root", "error":{"message":"Provider rejected the request"}, "willRetry":false
        }}).to_string()).unwrap();
        assert_eq!(
            decoder.last_error.as_deref(),
            Some("Provider rejected the request")
        );
        assert!(events.iter().any(|event| matches!(event,
            EngineEvent::Item { kind: ItemKind::Error { message, .. }, .. }
                if message.inline() == Some("Provider rejected the request"))));
    }

    #[test]
    fn native_shell_wrapper_preserves_the_exact_script_whitespace() {
        assert_eq!(
            display_command("bash -lc 'printf \"a  b\"'"),
            "printf \"a  b\""
        );
    }

    #[test]
    fn missing_native_terminal_status_rejects_ingress_without_ending_the_active_turn() {
        let mut decoder = Decoder {
            root: Some("root".into()),
            turn: Some("active".into()),
            unanswered: true,
            ..Decoder::default()
        };
        assert!(
            decoder
                .feed(
                    &json!({
                        "method":"turn/completed",
                        "params":{"threadId":"root","turn":{"id":"turn"}}
                    })
                    .to_string()
                )
                .is_err()
        );
        assert_eq!(decoder.active_turn(), Some("active"));
        assert!(decoder.take_turn_end().is_none());
        assert!(decoder.unanswered);
        assert!(decoder.items.is_empty());
    }
}

#[cfg(test)]
mod native_request_contract_tests {
    use super::*;
    fn decoder() -> Decoder {
        Decoder {
            root: Some("root".into()),
            ..Default::default()
        }
    }
    fn request(
        decoder: &mut Decoder,
        rpc: u64,
        item: &str,
        approval: Option<&str>,
        command: Option<&str>,
    ) -> Vec<EngineEvent> {
        decoder
            .feed(
                &json!({"id":rpc,"method":"item/commandExecution/requestApproval","params":{
                    "threadId":"root","itemId":item,"approvalId":approval,"command":command
                }})
                .to_string(),
            )
            .unwrap()
    }
    #[test]
    fn concurrent_requests_share_one_native_execution_without_overwriting_callbacks() {
        let mut decoder = decoder();
        let mut events = Vec::new();
        decoder.item("root", None, &json!({"id":"known","type":"commandExecution","command":"printf \"a  b\"","status":"inProgress"}), false, &mut events);
        let first = request(&mut decoder, 7, "known", Some("first"), None);
        let second = request(&mut decoder, 8, "known", Some("second"), None);
        assert!(
            matches!(&first[0], EngineEvent::Approval { target:Some(target), approval:Approval { request:ApprovalRequest::Command {command,..},..},.. } if target=="known" && command.inline()==Some("printf \"a  b\""))
        );
        assert!(
            matches!(&second[0], EngineEvent::Approval {target:Some(target),..} if target=="known")
        );
        assert_eq!(
            decoder
                .items
                .values()
                .filter(|item| matches!(item.kind, ItemKind::Tool { .. }))
                .count(),
            1
        );
        assert_eq!(decoder.pending.len(), 2);
        decoder.answer(
            "approval-request:7",
            &Answer::Approval {
                decision: Decision::AllowOnce,
            },
        );
        assert_eq!(decoder.take_outbox()[0]["id"], 7);
        assert_eq!(decoder.items["approval-request:7"].status, Status::Done);
        assert_eq!(decoder.items["known"].status, Status::Running);
        assert_eq!(decoder.pending.len(), 1);
        decoder.answer(
            "approval-request:8",
            &Answer::Approval {
                decision: Decision::DenyOnce,
            },
        );
        assert_eq!(decoder.take_outbox()[0]["id"], 8);
        assert_eq!(decoder.items["approval-request:8"].status, Status::Declined);
    }
    #[test]
    fn approval_before_tool_announcement_is_a_link_not_a_fake_execution() {
        let mut decoder = decoder();
        let events = request(&mut decoder, 9, "later", None, None);
        assert!(
            matches!(&events[0],EngineEvent::Approval {target:Some(target),approval:Approval {request:ApprovalRequest::Action {..},..},..} if target=="later")
        );
        assert!(!decoder.items.contains_key("later"));
        let mut events = Vec::new();
        decoder.item("root",None,&json!({"id":"later","type":"commandExecution","command":"echo later","status":"inProgress"}),false,&mut events);
        assert_eq!(decoder.items["later"].status, Status::Running);
        decoder.resolved(&json!(9), &mut events);
        assert!(matches!(
            &decoder.items["approval-request:9"].kind,
            ItemKind::Approval {
                approval: Approval { decision: None, .. },
                ..
            }
        ));
        assert_eq!(decoder.items["approval-request:9"].status, Status::Done);
    }
    #[test]
    fn malformed_native_question_replies_with_an_error_instead_of_empty_waiting_item() {
        let mut decoder = decoder();
        let events=decoder.feed(&json!({"id":9,"method":"item/tool/requestUserInput","params":{"threadId":"root","itemId":"question","isBlocking":true,"questions":[{"id":"q","header":"","question":" ","options":[]}]}}).to_string()).unwrap();
        assert!(decoder.pending.is_empty());
        assert!(events.iter().any(|event| matches!(
            event,
            EngineEvent::Item {
                kind: ItemKind::Error { .. },
                ..
            }
        )));
        assert_eq!(decoder.take_outbox()[0]["error"]["code"], -32602);
    }
    #[test]
    fn native_tool_completion_preserves_the_question_and_opaque_answer() {
        let mut decoder = decoder();
        decoder.feed(&json!({"id":9,"method":"item/tool/requestUserInput","params":{"threadId":"root","itemId":"question","isBlocking":true,"questions":[{"id":"q","header":"","question":"Choose","options":[{"label":"Same","description":""},{"label":"Same","description":""}]}]}}).to_string()).unwrap();
        let given = vec![Some(artisan_api::QuestionAnswer::Choices {
            ids: (vec!["option-1".into()]).into(),
        })];
        decoder.answer(
            "question",
            &Answer::Question {
                answers: given.clone(),
            },
        );
        decoder.feed(&json!({"method":"item/completed","params":{"threadId":"root","item":{"id":"question","type":"dynamicToolCall","tool":"request_user_input","arguments":null,"status":"completed","success":true}}}).to_string()).unwrap();
        assert!(
            matches!(&decoder.items["question"].kind,ItemKind::Question {answers:Some(answers),..} if answers.inline()==Some(given.as_slice()))
        );
        assert_eq!(decoder.items["question"].status, Status::Done);
    }
}

#[cfg(test)]
mod malformed_frame_tests {
    use super::*;
    #[test]
    fn malformed_json_and_nonobject_frames_are_protocol_errors() {
        let mut decoder = Decoder::default();
        assert!(decoder.feed("{broken-json").is_err());
        assert!(decoder.feed("null").is_err());
        assert!(decoder.feed("[]").is_err());
        assert!(decoder.feed("{}").is_err());
    }
}

#[cfg(test)]
mod native_plan_tests {
    use super::*;
    #[test]
    fn malformed_plan_batch_preserves_previous_steps_instead_of_partial_done() {
        let mut decoder = Decoder::default();
        let mut events = Vec::new();
        decoder.plan(
            None,
            &json!({"plan":[{"step":"Original","status":"pending"}]}),
            &mut events,
        );
        let old = decoder.items[PLAN_KEY].kind.clone();
        for malformed in [
            json!({}),
            json!({"plan":[{"step":"Valid","status":"completed"},{"status":"pending"}]}),
            json!({"plan":[{"step":"Original","status":"future"}]}),
        ] {
            events.clear();
            decoder.plan(None, &malformed, &mut events);
            assert_eq!(decoder.items[PLAN_KEY].kind, old);
            assert!(events.iter().any(|event| matches!(
                event,
                EngineEvent::Item {
                    kind: ItemKind::Error { .. },
                    ..
                }
            )));
        }
        events.clear();
        decoder.plan(None, &json!({"plan":[]}), &mut events);
        assert!(
            matches!(&decoder.items[PLAN_KEY].kind,ItemKind::Plan {steps,..} if steps.is_empty())
        );
        assert_eq!(decoder.items[PLAN_KEY].status, Status::Done);
    }
}

#[cfg(test)]
mod async_question_fields_tests {
    use super::*;
    #[test]
    fn absent_nullable_async_questions_do_not_create_errors_or_pending_prompts() {
        for item in [
            json!({}),
            json!({"questions":null}),
            json!({"questions":[]}),
        ] {
            let mut decoder = Decoder::default();
            let mut out = Vec::new();
            decoder.async_questions("message", None, &item, &mut out);
            assert!(out.is_empty());
            assert!(decoder.pending.is_empty());
        }
    }

    #[test]
    fn malformed_async_questions_never_drop_part_of_the_native_prompt() {
        for questions in [
            json!(false),
            json!([{"title":"Valid","options":["First"]},{}]),
            json!([{"title":"Choose","options":["First",4]}]),
            json!([{"title":"","options":[]}]),
            json!([{"title":"Choose","options":[""]}]),
        ] {
            let mut decoder = Decoder::default();
            let mut out = Vec::new();
            decoder.async_questions("message", None, &json!({"questions":questions}), &mut out);
            assert!(decoder.pending.is_empty());
            assert!(matches!(
                &out[..],
                [EngineEvent::Item {
                    kind: ItemKind::Error { .. },
                    ..
                }]
            ));
        }
    }
    #[test]
    fn async_question_native_order_and_duplicate_choices_are_preserved() {
        let mut decoder = Decoder::default();
        let mut out = Vec::new();
        decoder.async_questions("message",None,&json!({"questions":[{"title":"Choose","options":["Same","Same"]},{"title":"Explain"}]}),&mut out);
        let [
            EngineEvent::Item {
                kind: ItemKind::Question { questions, .. },
                ..
            },
        ] = &out[..]
        else {
            panic!("canonical questions expected")
        };
        let questions = questions.inline().expect("native collection is inline");
        assert_eq!(questions.len(), 2);
        assert_eq!(questions[1].text.inline(), Some("Explain"));
        assert_eq!(questions[0].options.inline().unwrap()[1].id, "option-1");
        assert_eq!(
            questions[0].options.inline().unwrap()[0].label,
            questions[0].options.inline().unwrap()[1].label
        );
        assert!(decoder.pending.contains_key("message:questions"));
    }
}

#[cfg(test)]
mod retained_source_tests {
    use super::*;
    #[test]
    fn terminal_command_emits_inline_once_and_late_permission_keeps_the_source() {
        let mut decoder = Decoder::default();
        decoder.root = Some("root".into());
        let command = "printf '😀' ".repeat(4096);
        let output = "large result\n".repeat(4096);
        let events = decoder.feed(&json!({"method":"item/completed", "params":{"threadId":"root", "item":{"id":"shell", "type":"commandExecution", "status":"completed", "command":command, "aggregatedOutput":output, "exitCode":0}}}).to_string()).unwrap();
        let first = events
            .iter()
            .find_map(|event| match event {
                EngineEvent::Item { key, kind, .. } if key == "shell" => Some(kind),
                _ => None,
            })
            .unwrap();
        let ItemKind::Tool {
            tool: ToolKind::Shell {
                command: emitted, ..
            },
            output: Some(emitted_output),
            ..
        } = first
        else {
            panic!("expected shell");
        };
        assert_eq!(emitted.inline(), Some(command.as_str()));
        assert_eq!(emitted_output.text.inline(), Some(output.as_str()));
        let ItemKind::Tool {
            tool: ToolKind::Shell {
                command: retained, ..
            },
            output: Some(retained_output),
            ..
        } = &decoder.items["shell"].kind
        else {
            panic!("expected retained shell");
        };
        assert_eq!(
            retained,
            &artisan_api::TextContent::Stored(artisan_api::ContentRef::from_text(&command))
        );
        assert!(retained.resident_bytes() < 256);
        assert!(retained_output.text.resident_bytes() < 256);
        let events = decoder.feed(&json!({"id":12, "method":"item/commandExecution/requestApproval", "params":{"threadId":"root", "itemId":"shell"}}).to_string()).unwrap();
        assert!(events.iter().any(|event| matches!(event, EngineEvent::Approval { target: Some(target), approval: Approval { request: ApprovalRequest::Command { command: preserved, .. }, .. }, .. } if target == "shell" && preserved == &artisan_api::TextContent::Stored(artisan_api::ContentRef::from_text(&command)))));
        let later = decoder.feed(&json!({"method":"item/completed", "params":{"threadId":"root", "item":{"id":"shell", "type":"commandExecution", "status":"completed", "command":command, "aggregatedOutput":"genuine later output", "exitCode":0}}}).to_string()).unwrap();
        assert!(later.iter().any(|event| matches!(event, EngineEvent::Item { kind: ItemKind::Tool { output: Some(value), .. }, .. } if value.text.inline() == Some("genuine later output"))));
    }
}

#[cfg(test)]
mod native_collection_tests {
    use super::*;
    use artisan_api::{Collection, CollectionRef, QuestionAnswer};

    #[test]
    fn large_native_question_keeps_callback_until_valid_inline_identity_answer() {
        let mut decoder = Decoder {
            root: Some("root".into()),
            ..Decoder::default()
        };
        let options: Vec<Value> = (0..5000)
            .map(|_| json!({"label":"Same","description":""}))
            .collect();
        let events = decoder.feed(&json!({"id":91,"method":"item/tool/requestUserInput","params":{
            "threadId":"root","itemId":"large-question","isBlocking":true,"questions":[{"id":"q","header":"","question":"Choose","options":options}]
        }}).to_string()).unwrap();
        assert!(
            matches!(&events[0], EngineEvent::Item {kind:ItemKind::Question {questions,..},..}
            if questions.inline().unwrap()[0].options.len()==5000)
        );
        for ids in [
            Collection::Inline(vec!["missing".into()]),
            Collection::Inline(vec!["option-4999".into(), "option-4999".into()]),
            Collection::Stored(CollectionRef::from_records(&["option-4999"]).unwrap()),
        ] {
            let errors = decoder.answer(
                "large-question",
                &Answer::Question {
                    answers: vec![Some(QuestionAnswer::Choices { ids })],
                },
            );
            assert!(matches!(
                &errors[..],
                [EngineEvent::Item {
                    kind: ItemKind::Error { .. },
                    ..
                }]
            ));
            assert!(decoder.pending.contains_key("large-question"));
            assert!(decoder.take_outbox().is_empty());
            assert_eq!(decoder.items["large-question"].status, Status::AwaitingUser);
        }
        let answer = vec![Some(QuestionAnswer::Choices {
            ids: vec!["option-4999".into()].into(),
        })];
        let events = decoder.answer(
            "large-question",
            &Answer::Question {
                answers: answer.clone(),
            },
        );
        assert!(!decoder.pending.contains_key("large-question"));
        let reply = decoder.take_outbox();
        assert_eq!(reply.len(), 1);
        assert_eq!(reply[0]["id"], 91);
        assert_eq!(
            reply[0]["result"]["answers"]["q"]["answers"],
            json!(["Same"])
        );
        assert!(
            matches!(&events[0], EngineEvent::Item {kind:ItemKind::Question {answers:Some(saved),..},..}
            if saved.inline()==Some(answer.as_slice()))
        );
        // A later native frame cannot retain or republish the 5000-option Vec.
        decoder
            .feed(&json!({"id":0,"result":null}).to_string())
            .unwrap();
        assert!(
            matches!(&decoder.items["large-question"].kind, ItemKind::Question {questions,..}
            if questions.inline().unwrap()[0].options.stored().is_some())
        );
    }

    #[test]
    fn five_thousand_native_plan_steps_emit_complete_once_then_retain_descriptor() {
        let mut decoder = Decoder {
            root: Some("root".into()),
            ..Decoder::default()
        };
        let plan: Vec<Value> = (0..5000)
            .map(|n| json!({"step":format!("Step {n}"),"status":"completed"}))
            .collect();
        let events = decoder
            .feed(
                &json!({"method":"turn/plan/updated","params":{"threadId":"root","plan":plan}})
                    .to_string(),
            )
            .unwrap();
        let complete = events
            .iter()
            .find_map(|event| match event {
                EngineEvent::Item {
                    kind: ItemKind::Plan { steps, .. },
                    ..
                } => Some(steps),
                _ => None,
            })
            .unwrap();
        assert_eq!(complete.inline().unwrap().len(), 5000);
        let ItemKind::Plan { steps, .. } = &decoder.items[PLAN_KEY].kind else {
            panic!("plan expected")
        };
        assert_eq!(steps.len(), 5000);
        assert_eq!(
            steps.stored(),
            Some(&CollectionRef::from_records(complete.inline().unwrap()).unwrap())
        );
    }
}

#[cfg(test)]
mod history_compaction_cost_tests {
    use super::*;
    #[test]
    fn ordinary_extension_frame_does_not_touch_five_thousand_completed_native_records() {
        let mut decoder = Decoder::default();
        for id in 0..5000 {
            decoder.items.insert(
                format!("old-{id}"),
                Tracked {
                    parent: None,
                    status: Status::Done,
                    kind: ItemKind::Tool {
                        tool: ToolKind::Other {
                            name: "fixture".into(),
                            input: None,
                        },
                        title: None,
                        output: None,
                        exit_code: None,
                        approval: None,
                    },
                },
            );
        }
        let source = "界🙂".repeat(4096);
        if let ItemKind::Tool {
            tool: ToolKind::Other { input, .. },
            ..
        } = &mut decoder.items.get_mut("old-4999").unwrap().kind
        {
            *input = Some(source.clone().into());
        }
        assert!(
            decoder
                .feed(r#"{"method":"extension/noop","params":{}}"#)
                .unwrap()
                .is_empty()
        );
        assert!(
            matches!(&decoder.items["old-4999"].kind,ItemKind::Tool {tool:ToolKind::Other {input:Some(input),..},..} if input.inline()==Some(source.as_str()))
        );
    }
}

#[cfg(test)]
mod child_reply_owner_tests {
    use super::*;
    #[test]
    fn complete_child_reply_has_one_ordered_inline_event_and_a_compact_retained_copy() {
        let mut decoder = Decoder {
            root: Some("root".into()),
            ..Decoder::default()
        };
        decoder.children.insert("child".into(), "agent".into());
        let text = "child report 界🙂 ".repeat(4096);
        let events = decoder.feed(&json!({"method":"item/completed", "params":{"threadId":"child", "item":{"id":"reply", "type":"agentMessage", "text":text}}}).to_string()).unwrap();
        assert!(events.iter().any(|event| matches!(event, EngineEvent::Item { kind: ItemKind::AssistantMessage { text: complete, .. }, .. } if complete.inline() == Some(text.as_str()))));
        assert_eq!(
            decoder.child_replies["child"],
            artisan_api::TextContent::Stored(artisan_api::ContentRef::from_text(&text))
        );
        assert!(decoder.child_replies["child"].resident_bytes() < 256);
        decoder.feed(&json!({"method":"turn/completed", "params":{"threadId":"child", "turn":{"id":"turn", "status":"completed"}}}).to_string()).unwrap();
        assert!(!decoder.child_replies.contains_key("child"));
    }
}

#[cfg(test)]
mod required_notice_tests {
    use super::*;
    fn frame(method: &str, params: Value) -> String {
        json!({"method":method,"params":params}).to_string()
    }
    fn goal(objective: &str, status: &str) -> Value {
        json!({"threadId":"root","turnId":null,"goal":{"objective":objective,"status":status}})
    }
    fn reroute() -> Value {
        json!({"threadId":"root","turnId":"turn-1","fromModel":"chosen","toModel":"safer","reason":"highRiskCyberActivity"})
    }
    fn safety() -> Value {
        json!({"threadId":"root","turnId":"turn-1","showBufferingUi":true})
    }
    fn malformed(method: &str, params: Value) {
        let mut decoder = Decoder {
            root: Some("root".to_owned()),
            ..Decoder::default()
        };
        decoder
            .feed(&frame(
                "thread/goal/updated",
                goal("Keep this goal", "active"),
            ))
            .unwrap();
        decoder.spawning.push("unbound-child".to_owned());
        let prior_goal = decoder.items["goal"].kind.clone();
        let count = decoder.items.len();
        assert!(
            decoder.feed(&frame(method, params)).is_err(),
            "{method}: malformed payload must report a protocol error"
        );
        assert_eq!(
            decoder.items.len(),
            count,
            "invalid notices must not manufacture Items"
        );
        assert_eq!(
            decoder.items["goal"].kind, prior_goal,
            "invalid notices must not replace an existing goal"
        );
        assert!(
            decoder.deferred.is_empty(),
            "invalid identity must not enter the child relation queue"
        );
    }
    #[test]
    fn consumed_notices_reject_missing_empty_and_wrongly_typed_required_fields_before_mutation() {
        for (method, valid, fields) in [
            (
                "model/rerouted",
                reroute(),
                vec!["threadId", "turnId", "fromModel", "toModel", "reason"],
            ),
            (
                "model/safetyBuffering/updated",
                safety(),
                vec!["threadId", "turnId", "showBufferingUi"],
            ),
            (
                "thread/goal/updated",
                goal("Valid goal", "active"),
                vec!["threadId", "goal"],
            ),
            (
                "thread/goal/cleared",
                json!({"threadId":"root"}),
                vec!["threadId"],
            ),
        ] {
            for field in fields {
                let mut missing = valid.clone();
                missing.as_object_mut().unwrap().remove(field);
                malformed(method, missing);
                let mut wrong = valid.clone();
                wrong[field] = json!(17);
                malformed(method, wrong);
                let mut empty = valid.clone();
                empty[field] = json!(" ");
                malformed(method, empty);
            }
        }
        for field in ["objective", "status"] {
            let mut missing = goal("Valid goal", "active");
            missing["goal"].as_object_mut().unwrap().remove(field);
            malformed("thread/goal/updated", missing);
            let mut wrong = goal("Valid goal", "active");
            wrong["goal"][field] = json!(false);
            malformed("thread/goal/updated", wrong);
            let mut empty = goal("Valid goal", "active");
            empty["goal"][field] = json!("");
            malformed("thread/goal/updated", empty);
        }
        malformed(
            "thread/goal/updated",
            goal("Valid goal", "new-unknown-status"),
        );
        let mut unknown_reason = reroute();
        unknown_reason["reason"] = json!("new-unknown-reason");
        malformed("model/rerouted", unknown_reason);
    }
    #[test]
    fn clearing_a_compacted_goal_reuses_its_canonical_source_without_inline_assumptions() {
        let mut decoder = Decoder {
            root: Some("root".to_owned()),
            ..Decoder::default()
        };
        let objective = "An authored goal. ".repeat(2000);
        decoder
            .feed(&frame("thread/goal/updated", goal(&objective, "active")))
            .unwrap();
        let ItemKind::Notice { text, .. } = &decoder.items["goal"].kind else {
            panic!("expected goal")
        };
        assert!(matches!(text, artisan_api::TextContent::Stored(_)));
        let expected = text.clone();
        let events = decoder
            .feed(&frame("thread/goal/cleared", json!({"threadId":"root"})))
            .unwrap();
        assert!(events.iter().any(|event| matches!(event, EngineEvent::Item { kind: ItemKind::Notice { kind: NoticeKind::Goal { state }, text }, .. } if state == "cleared" && text == &expected)));
    }
    #[test]
    fn valid_notices_keep_child_scope_nullable_goal_turn_and_intentional_noops() {
        let mut decoder = Decoder {
            root: Some("root".to_owned()),
            ..Decoder::default()
        };
        decoder
            .children
            .insert("child".to_owned(), "agent-item".to_owned());
        assert!(
            decoder
                .feed(&frame("thread/goal/cleared", json!({"threadId":"root"})))
                .unwrap()
                .is_empty()
        );
        let mut hidden = safety();
        hidden["showBufferingUi"] = json!(false);
        assert!(
            decoder
                .feed(&frame("model/safetyBuffering/updated", hidden))
                .unwrap()
                .is_empty()
        );
        let mut child_reroute = reroute();
        child_reroute["threadId"] = json!("child");
        let events = decoder
            .feed(&frame("model/rerouted", child_reroute))
            .unwrap();
        assert!(events.iter().any(|event| matches!(event, EngineEvent::Item { key, parent: Some(parent), kind: ItemKind::Notice { kind: NoticeKind::Reroute { model }, .. }, .. } if key == "reroute:turn-1" && parent == "agent-item" && model == "safer")));
        let events = decoder
            .feed(&frame("model/safetyBuffering/updated", safety()))
            .unwrap();
        assert!(events.iter().any(|event| matches!(event, EngineEvent::Item { key, kind: ItemKind::Notice { kind: NoticeKind::Safety, .. }, .. } if key == "safety:turn-1")));
        assert!(
            decoder
                .feed(&frame("model/safetyBuffering/updated", safety()))
                .unwrap()
                .is_empty(),
            "repeated true updates do not duplicate the safety notice"
        );
        for status in [
            "active",
            "paused",
            "blocked",
            "usageLimited",
            "budgetLimited",
            "complete",
        ] {
            let events = decoder
                .feed(&frame(
                    "thread/goal/updated",
                    goal("Keep authored objective", status),
                ))
                .unwrap();
            assert!(events.iter().any(|event| matches!(event, EngineEvent::Item { key, kind: ItemKind::Notice { kind: NoticeKind::Goal { state }, text }, .. } if key == "goal" && state == status && text.inline() == Some("Keep authored objective"))));
        }
        let events = decoder
            .feed(&frame("thread/goal/cleared", json!({"threadId":"root"})))
            .unwrap();
        assert!(events.iter().any(|event| matches!(event, EngineEvent::Item { kind: ItemKind::Notice { kind: NoticeKind::Goal { state }, text }, .. } if state == "cleared" && text.inline() == Some("Keep authored objective"))));
    }
}

#[cfg(test)]
mod required_item_payload_tests {
    use super::*;
    fn frame(method: &str, params: Value) -> String {
        json!({"method":method,"params":params}).to_string()
    }
    fn decoder() -> Decoder {
        let mut decoder = Decoder {
            root: Some("root".into()),
            turn: Some("active".into()),
            unanswered: true,
            ..Decoder::default()
        };
        decoder.feed(&frame("item/started",json!({"threadId":"root","item":{"id":"spawn","type":"collabAgentToolCall","tool":"spawnAgent","status":"inProgress","agentsStates":{},"receiverThreadIds":["child"]}}))).unwrap();
        decoder
            .child_replies
            .insert("child".into(), "Keep reply".into());
        // Valid setup Item delivery clears this flag. Establish the state
        // being protected after that setup, before testing malformed ingress.
        decoder.unanswered = true;
        decoder
    }
    fn reject(method: &str, params: Value) {
        let mut decoder = decoder();
        decoder.spawning.push("waiting".into());
        let prior = decoder.items["spawn"].kind.clone();
        let bytes = decoder.ownership.bytes();
        assert!(decoder.feed(&frame(method, params)).is_err());
        assert_eq!(decoder.turn.as_deref(), Some("active"));
        assert!(decoder.turn_end.is_none());
        assert!(decoder.unanswered);
        assert_eq!(decoder.items.len(), 1);
        assert_eq!(decoder.items["spawn"].kind, prior);
        assert_eq!(decoder.items["spawn"].status, Status::Running);
        assert_eq!(
            decoder.children.get("child").map(String::as_str),
            Some("spawn")
        );
        assert_eq!(decoder.child_replies["child"].inline(), Some("Keep reply"));
        assert_eq!(decoder.spawning, ["waiting"]);
        assert!(decoder.deferred.is_empty());
        assert_eq!(decoder.deferred_bytes, 0);
        assert_eq!(decoder.ownership.bytes(), bytes);
    }
    #[test]
    fn malformed_terminal_receivers_and_content_fail_before_any_mutation() {
        let terminal = json!({"threadId":"child","turn":{"id":"turn","status":"completed"}});
        for thread in [Value::Null, json!(7), json!("")] {
            let mut params = terminal.clone();
            params["threadId"] = thread;
            reject("turn/completed", params);
        }
        for field in ["id", "status"] {
            let mut params = terminal.clone();
            params["turn"].as_object_mut().unwrap().remove(field);
            reject("turn/completed", params);
            for invalid in [
                Value::Null,
                json!(false),
                json!(""),
                json!("inProgress"),
                json!("unknown"),
            ] {
                if field == "id" && matches!(invalid.as_str(), Some("inProgress" | "unknown")) {
                    continue;
                }
                let mut params = terminal.clone();
                params["turn"][field] = invalid;
                reject("turn/completed", params);
            }
        }
        for receivers in [Value::Null, json!({}), json!(["child", 17]), json!([""])] {
            reject(
                "item/completed",
                json!({"threadId":"root","item":{"id":"spawn","type":"collabAgentToolCall","tool":"spawnAgent","status":"inProgress","agentsStates":{},"receiverThreadIds":receivers}}),
            );
        }
        reject(
            "item/started",
            json!({"threadId":"unknown","item":{"id":"new","type":"collabAgentToolCall","tool":"spawnAgent"}}),
        );
        for content in [
            Value::Null,
            json!({}),
            json!([{"type":"text"}]),
            json!([{"type":"text","text":17}]),
            json!([{"type":"unknown"}]),
        ] {
            reject(
                "item/completed",
                json!({"threadId":"child","item":{"id":"user","type":"userMessage","content":content}}),
            );
        }
        reject(
            "item/started",
            json!({"threadId":"unknown","item":{"id":"user","type":"userMessage"}}),
        );
    }
    #[test]
    fn child_completion_uses_only_explicit_terminal_status_and_consumes_its_reply() {
        for (status, expected) in [
            ("completed", Status::Done),
            ("interrupted", Status::Cancelled),
            ("failed", Status::Failed),
        ] {
            let mut decoder = decoder();
            let events = decoder
                .feed(&frame(
                    "turn/completed",
                    json!({"threadId":"child","turn":{"id":"child-turn","status":status}}),
                ))
                .unwrap();
            assert_eq!(decoder.items["spawn"].status, expected);
            assert!(
                matches!(&decoder.items["spawn"].kind,ItemKind::Agent {result:Some(result),..} if result.inline()==Some("Keep reply"))
            );
            assert!(!decoder.child_replies.contains_key("child"));
            assert_eq!(decoder.turn.as_deref(), Some("active"));
            assert!(
                events
                    .iter()
                    .any(|event| matches!(event,EngineEvent::Item {key,..} if key=="spawn"))
            );
        }
    }
    #[test]
    fn valid_empty_receivers_and_nontext_user_inputs_do_not_invent_prompt_text() {
        let mut decoder = decoder();
        decoder.feed(&frame("item/started",json!({"threadId":"root","item":{"id":"waiting","type":"collabAgentToolCall","tool":"spawnAgent","status":"inProgress","agentsStates":{},"receiverThreadIds":[]}}))).unwrap();
        assert_eq!(decoder.spawning, ["waiting"]);
        for content in [
            json!([]),
            json!([{"type":"image"},{"type":"audio"},{"type":"skill"}]),
            json!([
                {"type":"image","url":"https://example/image"},{"type":"image","fileId":"file"},
                {"type":"localImage","path":"/image"},{"type":"audio","url":"https://example/audio"},
                {"type":"localAudio","path":"/audio"},{"type":"skill","name":"Skill","path":"/skill"},
                {"type":"mention","name":"Name","path":"/mention"}
            ]),
        ] {
            assert!(decoder.feed(&frame("item/started",json!({"threadId":"child","item":{"id":"user","type":"userMessage","content":content}}))).unwrap().is_empty());
            assert!(matches!(
                &decoder.items["spawn"].kind,
                ItemKind::Agent { prompt: None, .. }
            ));
        }
        decoder.feed(&frame("item/completed",json!({"threadId":"child","item":{"id":"user","type":"userMessage","content":[{"type":"text","text":"First"},{"type":"image","fileId":"file"},{"type":"text","text":"Second"}]}}))).unwrap();
        assert!(
            matches!(&decoder.items["spawn"].kind,ItemKind::Agent {prompt:Some(prompt),description,..} if prompt.inline()==Some("First\nSecond") && description.inline()==Some(""))
        );
    }
}

#[cfg(test)]
mod required_progress_payload_tests {
    use super::*;
    fn frame(method: &str, params: Value) -> String {
        json!({"method":method,"params":params}).to_string()
    }
    fn usage() -> Value {
        json!({"threadId":"root","tokenUsage":{"last":{
            "totalTokens":120,"inputTokens":100,"cachedInputTokens":20,
            "cacheWriteInputTokens":5,"outputTokens":15,"reasoningOutputTokens":3
        },"modelContextWindow":1000}})
    }
    fn rejects(method: &str, params: Value, rpc: bool) {
        let mut decoder = Decoder {
            root: Some("root".into()),
            turn: Some("active".into()),
            ..Decoder::default()
        };
        decoder
            .feed(&frame("thread/tokenUsage/updated", usage()))
            .unwrap();
        decoder.unanswered = true;
        decoder.last_error = Some("Existing error".into());
        decoder.spawning.push("unbound".into());
        let prior = serde_json::to_value(&decoder.usage).unwrap();
        let bytes = decoder.ownership.bytes();
        let mut message = json!({"method":method,"params":params});
        if rpc {
            message["id"] = json!(17);
        }
        assert!(
            decoder.feed(&message.to_string()).is_err(),
            "{method} must reject malformed consumed fields"
        );
        assert_eq!(serde_json::to_value(&decoder.usage).unwrap(), prior);
        assert_eq!(decoder.turn.as_deref(), Some("active"));
        assert!(decoder.unanswered);
        assert_eq!(decoder.last_error.as_deref(), Some("Existing error"));
        assert_eq!(decoder.notices, 0);
        assert!(decoder.items.is_empty() && decoder.children.is_empty());
        assert!(decoder.deferred.is_empty() && decoder.deferred_bytes == 0);
        assert_eq!(decoder.spawning, ["unbound"]);
        assert!(decoder.rpc.outbox.is_empty());
        assert_eq!(decoder.ownership.bytes(), bytes);
    }
    #[test]
    fn required_progress_fields_reject_before_usage_turn_or_scope_mutation() {
        for field in [
            "totalTokens",
            "inputTokens",
            "cachedInputTokens",
            "cacheWriteInputTokens",
            "outputTokens",
            "reasoningOutputTokens",
        ] {
            let mut missing = usage();
            missing["tokenUsage"]["last"]
                .as_object_mut()
                .unwrap()
                .remove(field);
            rejects("thread/tokenUsage/updated", missing, false);
            for wrong in [Value::Null, json!(-1), json!("0"), json!(false), json!(1.5)] {
                let mut params = usage();
                params["tokenUsage"]["last"][field] = wrong;
                rejects("thread/tokenUsage/updated", params, false);
            }
        }
        for field in ["threadId", "turn"] {
            let mut params = json!({"threadId":"root","turn":{"id":"new"}});
            params.as_object_mut().unwrap().remove(field);
            rejects("turn/started", params, false);
        }
        for id in [Value::Null, json!(7), json!("")] {
            rejects(
                "turn/started",
                json!({"threadId":"root","turn":{"id":id}}),
                false,
            );
        }
        rejects("turn/started", json!({"threadId":"root","turn":{}}), false);
        for id in [Value::Null, json!(false), json!("")] {
            rejects(
                "thread/started",
                json!({"thread":{"id":id,"parentThreadId":"root"}}),
                false,
            );
        }
        rejects(
            "thread/started",
            json!({"thread":{"parentThreadId":"root"}}),
            false,
        );
        for field in ["error", "willRetry", "threadId"] {
            let mut params =
                json!({"threadId":"root","error":{"message":"Authored error"},"willRetry":false});
            params.as_object_mut().unwrap().remove(field);
            rejects("error", params, false);
        }
        rejects(
            "error",
            json!({"threadId":"root","error":{},"willRetry":false}),
            false,
        );
        rejects(
            "error",
            json!({"threadId":"root","error":{"message":17},"willRetry":false}),
            false,
        );
        rejects(
            "error",
            json!({"threadId":"root","error":{"message":"Authored"},"willRetry":"false"}),
            false,
        );
        for field in ["threadId", "serverName", "message"] {
            let mut params = json!({"threadId":"root","serverName":"MCP","message":"Authored request","turnId":null});
            params.as_object_mut().unwrap().remove(field);
            rejects("mcpServer/elicitation/request", params, true);
            let mut params = json!({"threadId":"root","serverName":"MCP","message":"Authored request","turnId":null});
            params[field] = json!(false);
            rejects("mcpServer/elicitation/request", params, true);
        }
    }
    #[test]
    fn explicit_zero_counts_nullable_context_and_valid_child_progress_keep_their_meaning() {
        let mut decoder = Decoder {
            root: Some("root".into()),
            ..Decoder::default()
        };
        decoder
            .feed(&frame("thread/tokenUsage/updated", usage()))
            .unwrap();
        assert_eq!(
            (
                decoder.usage.input,
                decoder.usage.cache_read,
                decoder.usage.cache_write,
                decoder.usage.output,
                decoder.usage.reasoning
            ),
            (80, 20, Some(5), 15, 3)
        );
        assert_eq!(
            (decoder.usage.context_used, decoder.usage.context_window),
            (Some(120), Some(1000))
        );
        decoder.children.insert("child".into(), "agent".into());
        let mut child = usage();
        child["threadId"] = json!("child");
        child["tokenUsage"]["modelContextWindow"] = json!(2000);
        decoder
            .feed(&frame("thread/tokenUsage/updated", child))
            .unwrap();
        assert_eq!(
            (
                decoder.usage.input,
                decoder.usage.cache_read,
                decoder.usage.output
            ),
            (160, 40, 30)
        );
        assert_eq!(
            (decoder.usage.context_used, decoder.usage.context_window),
            (Some(120), Some(1000)),
            "child context must not replace root context"
        );
        let mut zero = usage();
        for value in zero["tokenUsage"]["last"]
            .as_object_mut()
            .unwrap()
            .values_mut()
        {
            *value = json!(0);
        }
        zero["tokenUsage"]["modelContextWindow"] = Value::Null;
        decoder
            .feed(&frame("thread/tokenUsage/updated", zero))
            .unwrap();
        assert_eq!(
            (
                decoder.usage.input,
                decoder.usage.context_used,
                decoder.usage.context_window
            ),
            (160, Some(0), Some(1000))
        );
        decoder
            .feed(&frame(
                "thread/started",
                json!({"thread":{"id":"root","parentThreadId":null,"agentNickname":null}}),
            ))
            .unwrap();
        assert!(decoder.children.len() == 1 && decoder.items.is_empty());
        decoder
            .feed(&frame(
                "turn/started",
                json!({"threadId":"root","turn":{"id":"new"}}),
            ))
            .unwrap();
        assert_eq!(decoder.active_turn(), Some("new"));
        decoder.feed(&frame("thread/started",json!({"thread":{"id":"new-child","parentThreadId":"root","agentNickname":"Named child"}}))).unwrap();
        assert_eq!(
            decoder.children.get("new-child").map(String::as_str),
            Some("new-child")
        );
        assert!(
            matches!(&decoder.items["new-child"].kind,ItemKind::Agent {name:Some(name),..} if name=="Named child")
        );
    }
    #[test]
    fn explicit_retry_error_and_unsupported_elicitation_preserve_authored_text() {
        let mut decoder = Decoder {
            root: Some("root".into()),
            ..Decoder::default()
        };
        let events = decoder
            .feed(&frame(
                "error",
                json!({"threadId":"root","error":{"message":"Try later"},"willRetry":true}),
            ))
            .unwrap();
        assert!(events.iter().any(|event|matches!(event,EngineEvent::Live {target:LiveKind::Status,text,..} if text=="Try later Retrying…")));
        assert!(decoder.items.is_empty() && decoder.last_error.is_none());
        decoder
            .feed(&frame(
                "error",
                json!({"threadId":"root","error":{"message":"Authored failure"},"willRetry":false}),
            ))
            .unwrap();
        assert_eq!(decoder.last_error.as_deref(), Some("Authored failure"));
        let events=decoder.feed(&json!({"id":21,"method":"mcpServer/elicitation/request","params":{"threadId":"root","turnId":null,"serverName":"Named MCP","message":"Authored prompt"}}).to_string()).unwrap();
        assert!(events.iter().any(|event|matches!(event,EngineEvent::Item {kind:ItemKind::Notice {text,..},..} if text.inline()==Some("Named MCP asked for input Artisan can't show yet: Authored prompt"))));
        let out = decoder.take_outbox();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["id"], 21);
        assert_eq!(out[0]["result"]["action"], "decline");
    }
}

#[cfg(test)]
mod required_tool_status_tests {
    use super::*;
    fn item(kind: &str, status: &str) -> Value {
        let mut item = json!({"id":"tool","type":kind,"status":status});
        match kind {
            "commandExecution" => item["command"] = json!("echo fixture"),
            "fileChange" => item["changes"] = json!([]),
            "mcpToolCall" => {
                item["server"] = json!("fixture-server");
                item["arguments"] = Value::Null;
                item["tool"] = json!("fixture-tool");
            }
            "dynamicToolCall" => {
                item["tool"] = json!("fixture-tool");
                item["success"] = Value::Null;
                item["arguments"] = Value::Null;
            }
            _ => unreachable!(),
        }
        item
    }
    fn feed(
        decoder: &mut Decoder,
        method: &str,
        thread: &str,
        item: Value,
    ) -> anyhow::Result<Vec<EngineEvent>> {
        decoder.feed(&json!({"method":method,"params":{"threadId":thread,"item":item}}).to_string())
    }
    fn reject(kind: &str, invalid: Value, thread: &str) {
        let mut decoder = Decoder {
            root: Some("root".into()),
            turn: Some("active".into()),
            ..Decoder::default()
        };
        feed(
            &mut decoder,
            "item/started",
            "root",
            item(kind, "inProgress"),
        )
        .unwrap();
        // Establish the state after valid startup: starting a real tool clears
        // unanswered by design; the malformed update must leave it untouched.
        decoder.unanswered = true;
        decoder.spawning.push("unbound".into());
        let before = decoder.items["tool"].kind.clone();
        let bytes = decoder.ownership.bytes();
        assert!(feed(&mut decoder, "item/completed", thread, invalid).is_err());
        assert_eq!(decoder.items.len(), 1);
        assert_eq!(decoder.items["tool"].kind, before);
        assert_eq!(decoder.items["tool"].status, Status::Running);
        assert_eq!(decoder.turn.as_deref(), Some("active"));
        assert!(decoder.unanswered);
        assert!(decoder.turn_end.is_none());
        assert!(decoder.deferred.is_empty() && decoder.deferred_bytes == 0);
        assert_eq!(decoder.spawning, ["unbound"]);
        assert!(decoder.pending.is_empty() && decoder.rpc.outbox.is_empty());
        assert_eq!(decoder.ownership.bytes(), bytes);
    }
    #[test]
    fn malformed_native_tool_status_never_settles_or_defers_an_item() {
        for kind in [
            "commandExecution",
            "fileChange",
            "mcpToolCall",
            "dynamicToolCall",
        ] {
            let mut missing = item(kind, "completed");
            missing.as_object_mut().unwrap().remove("status");
            reject(kind, missing.clone(), "root");
            reject(kind, missing, "unknown-child");
            for status in [
                Value::Null,
                json!(7),
                json!(false),
                json!(""),
                json!("unknown"),
            ] {
                let mut wrong = item(kind, "completed");
                wrong["status"] = status;
                reject(kind, wrong, "root");
            }
        }
        for kind in ["mcpToolCall", "dynamicToolCall"] {
            reject(kind, item(kind, "declined"), "root");
        }
        let mut invalid = item("dynamicToolCall", "completed");
        invalid["success"] = json!("false");
        reject("dynamicToolCall", invalid, "root");
    }
    #[test]
    fn every_valid_status_is_explicit_and_dynamic_failure_overrides_successful_completion() {
        for kind in [
            "commandExecution",
            "fileChange",
            "mcpToolCall",
            "dynamicToolCall",
        ] {
            for (status, expected) in [
                ("inProgress", Status::Running),
                ("completed", Status::Done),
                ("failed", Status::Failed),
                ("declined", Status::Declined),
            ] {
                if status == "declined" && matches!(kind, "mcpToolCall" | "dynamicToolCall") {
                    continue;
                }
                let mut decoder = Decoder {
                    root: Some("root".into()),
                    ..Decoder::default()
                };
                feed(
                    &mut decoder,
                    "item/started",
                    "root",
                    item(kind, "inProgress"),
                )
                .unwrap();
                feed(&mut decoder, "item/completed", "root", item(kind, status)).unwrap();
                assert_eq!(
                    decoder.items["tool"].status, expected,
                    "{kind} native {status}"
                );
            }
        }
        for success in [Value::Null, json!(true), json!(false)] {
            let mut decoder = Decoder {
                root: Some("root".into()),
                ..Decoder::default()
            };
            let mut completed = item("dynamicToolCall", "completed");
            completed["success"] = success.clone();
            feed(&mut decoder, "item/completed", "root", completed).unwrap();
            assert_eq!(
                decoder.items["tool"].status,
                if success == json!(false) {
                    Status::Failed
                } else {
                    Status::Done
                }
            );
        }
    }
}

#[cfg(test)]
mod remaining_consumed_fields_tests {
    use super::*;
    fn frame(item: Value) -> String {
        json!({"method":"item/completed","params":{"threadId":"root","item":item}}).to_string()
    }
    fn spawn() -> Value {
        json!({"id":"agent","type":"collabAgentToolCall","tool":"spawnAgent","status":"inProgress","receiverThreadIds":["child"],"agentsStates":{},"prompt":null})
    }
    fn rejects(message: Value) {
        let mut decoder = Decoder {
            root: Some("root".into()),
            turn: Some("active".into()),
            ..Decoder::default()
        };
        decoder.feed(&frame(spawn())).unwrap();
        decoder.unanswered = true;
        let prior = decoder.items["agent"].kind.clone();
        let bytes = decoder.ownership.bytes();
        assert!(decoder.feed(&message.to_string()).is_err());
        assert_eq!(decoder.items.len(), 1);
        assert_eq!(decoder.items["agent"].kind, prior);
        assert_eq!(decoder.items["agent"].status, Status::Running);
        assert!(decoder.unanswered);
        assert_eq!(decoder.turn.as_deref(), Some("active"));
        assert_eq!(
            decoder.children.get("child").map(String::as_str),
            Some("agent")
        );
        assert!(decoder.deferred.is_empty() && decoder.deferred_bytes == 0);
        assert!(decoder.pending.is_empty() && decoder.rpc.outbox.is_empty());
        assert_eq!(decoder.ownership.bytes(), bytes);
    }
    fn rejects_item(item: Value) {
        rejects(serde_json::from_str(&frame(item)).unwrap());
    }
    #[test]
    fn malformed_collab_identity_tool_status_and_mcp_names_fail_before_mutation() {
        for field in ["tool", "status", "agentsStates"] {
            let mut value = spawn();
            value.as_object_mut().unwrap().remove(field);
            rejects_item(value);
            for bad in [Value::Null, json!(false), json!("unknown")] {
                let mut value = spawn();
                value[field] = bad;
                rejects_item(value);
            }
        }
        let mut bad = spawn();
        bad["agentsStates"] = json!({"child":{"message":42}});
        rejects_item(bad);
        for kind in ["mcpToolCall", "dynamicToolCall"] {
            let valid = json!({"id":"tool","type":kind,"status":"completed","server":"MCP","tool":"native","arguments":null});
            for field in if kind == "mcpToolCall" {
                vec!["server", "tool", "arguments"]
            } else {
                vec!["tool", "arguments"]
            } {
                let mut missing = valid.clone();
                missing.as_object_mut().unwrap().remove(field);
                rejects_item(missing);
                if field != "arguments" {
                    for bad in [Value::Null, json!(false), json!("")] {
                        let mut value = valid.clone();
                        value[field] = bad;
                        rejects_item(value);
                    }
                }
            }
        }
        for item in [
            json!({"id":"text","type":"agentMessage"}),
            json!({"id":"text","type":"agentMessage","text":false}),
            json!({"id":"text","type":"agentMessage","text":"Preserve text","phase":"unknown"}),
            json!({"id":"text","type":"plan","text":7}),
        ] {
            rejects_item(item);
        }
        for change in [
            json!({"path":"file","kind":{"type":"unknown"},"diff":""}),
            json!({"path":"file","kind":{"type":"update"}}),
            json!({"path":"file","kind":{"type":"update"},"diff":false}),
        ] {
            rejects_item(
                json!({"id":"edit","type":"fileChange","status":"completed","changes":[change]}),
            );
        }
        for result in [json!({}), json!({"content":false}), json!(false)] {
            rejects_item(
                json!({"id":"tool","type":"mcpToolCall","status":"completed","server":"MCP","tool":"native","arguments":null,"result":result}),
            );
        }
        rejects_item(
            json!({"id":"tool","type":"dynamicToolCall","status":"completed","tool":"native","arguments":null,"contentItems":[{"type":"inputText"}]}),
        );
        rejects(
            json!({"id":10,"method":"item/tool/requestUserInput","params":{"threadId":"root","itemId":"question","isBlocking":true,"questions":[{"id":"q","question":"Choose","options":null}]}}),
        );
        rejects(
            json!({"id":10,"method":"item/tool/requestUserInput","params":{"threadId":"root","itemId":"question","isBlocking":true,"questions":[{"id":"q","header":"","question":"Choose","options":[{"label":"Choice"}]}]}}),
        );
        for blocking in [Value::Null, json!("true"), json!(1)] {
            rejects(
                json!({"id":10,"method":"item/tool/requestUserInput","params":{"threadId":"root","itemId":"question","isBlocking":blocking,"questions":[]}}),
            );
        }
        rejects(
            json!({"id":10,"method":"item/tool/requestUserInput","params":{"threadId":"root","itemId":"question","questions":[]}}),
        );
    }
    #[test]
    fn nullable_and_optional_native_data_keep_intentional_compatibility() {
        let mut decoder = Decoder {
            root: Some("root".into()),
            ..Decoder::default()
        };
        decoder.feed(&frame(spawn())).unwrap();
        assert!(
            matches!(&decoder.items["agent"].kind,ItemKind::Agent {prompt:None,description,..} if description.inline()==Some(""))
        );
        for tool in [
            "sendInput",
            "resumeAgent",
            "wait",
            "closeAgent",
            "sendMessage",
            "followupTask",
            "interruptAgent",
            "listAgents",
        ] {
            let value = json!({"id":"call","type":"collabAgentToolCall","tool":tool,"status":"completed","receiverThreadIds":[],"agentsStates":{"child":{"message":null}}});
            assert!(decoder.feed(&frame(value)).unwrap().is_empty());
        }
        decoder.feed(&frame(json!({"id":"wait","type":"collabAgentToolCall","tool":"wait","status":"completed","receiverThreadIds":[],"agentsStates":{"child":{"message":"Authored child reply"}}}))).unwrap();
        assert!(
            matches!(&decoder.items["agent"].kind,ItemKind::Agent {result:Some(result),..} if result.inline()==Some("Authored child reply"))
        );
        for (native, expected) in [
            ("failed", Status::Failed),
            ("interrupted", Status::Cancelled),
        ] {
            let mut separate = Decoder {
                root: Some("root".into()),
                ..Decoder::default()
            };
            separate.feed(&frame(spawn())).unwrap();
            let mut terminal = spawn();
            terminal["status"] = json!(native);
            separate.feed(&frame(terminal)).unwrap();
            assert_eq!(separate.items["agent"].status, expected);
        }
        for (phase, expected) in [
            (Value::Null, Phase::Final),
            (json!("commentary"), Phase::Commentary),
            (json!("partial_answer"), Phase::Final),
            (json!("final_answer"), Phase::Final),
        ] {
            let events = decoder
                .feed(&frame(
                    json!({"id":"text","type":"agentMessage","text":"Authored text","phase":phase}),
                ))
                .unwrap();
            assert!(events.iter().any(|event|matches!(event,EngineEvent::Item {kind:ItemKind::AssistantMessage {phase,..},..} if *phase==expected)));
        }
        decoder.feed(&frame(json!({"id":"edit","type":"fileChange","status":"completed","changes":[{"path":"old","kind":{"type":"update","move_path":null},"diff":""},{"path":"old","kind":{"type":"update","move_path":"new"},"diff":"patch"}]}))).unwrap();
        assert!(
            matches!(&decoder.items["edit"].kind,ItemKind::Tool {tool:ToolKind::Edit {changes},..} if matches!(&changes.inline().unwrap()[0].op,artisan_api::FileOp::Modify) && matches!(&changes.inline().unwrap()[1].op,artisan_api::FileOp::Move {from} if from=="old"))
        );
        let events=decoder.feed(&json!({"id":10,"method":"item/tool/requestUserInput","params":{"threadId":"root","itemId":"q","isBlocking":false,"questions":[{"id":"choice","header":"","question":"Choose","options":null}]}}).to_string()).unwrap();
        assert!(events.iter().any(|event| matches!(
            event,
            EngineEvent::Item {
                kind: ItemKind::Question {
                    blocking: false,
                    ..
                },
                ..
            }
        )));
    }
}
