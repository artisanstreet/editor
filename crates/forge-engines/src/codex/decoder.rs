//! Pure decoding of `codex app-server` JSON-RPC (one JSON object per line).
//!
//! The decoder follows the run's root thread plus every child thread its
//! agents spawn. Each child thread becomes an `Agent` item (keyed by the
//! `spawnAgent` call) and everything the child does is decoded the same way
//! with `parent` set to that item. It also keeps the JSON-RPC bookkeeping:
//! request ids, responses for the driver, and replies to Codex's own
//! requests (`requests`). Shapes were captured from Codex 0.162.1 (see
//! `transcripts/codex`) and its generated app-server schema (`protocol`).
//!
//! Each line is parsed once into `protocol` types. What the decoder can't
//! read is logged with its method and thread and skipped, except a turn's
//! end, which the run waits for: an unreadable `turn/completed` fails it.

mod requests;

use super::protocol::{
    CallStatus, CodexError, CollabCall, CollabStatus, CollabTool, CompletedTurn, ExecutionStatus,
    Frame, Item, MessagePhase, Notification, PatchKind, PlanEntry, PlanStatus, ThreadInfo,
    ThreadStarted, TokenUsage, TurnStatus, UserInput,
};
use crate::{EngineEvent, LiveKind, Outcome};
use anyhow::Context as _;
use artisan_api::{
    AgentAction, ErrorKind, FileChange, FileOp, ItemKind, MessageDirection, NoticeKind, Output,
    Phase, PlanStep, RunStatus, Status, StepStatus, TextContent, ToolKind, Usage,
};
use requests::{Approvals, Questions};
use serde_json::Value;
use std::collections::HashMap;

/// The key of the run's plan (`turn/plan/updated`); one per run.
const PLAN_KEY: &str = "plan";

/// Child frames held until Codex says which spawn they belong to.
const DEFERRED_FRAMES: usize = 256;
const DEFERRED_BYTES: usize = 4 * 1024 * 1024;

pub(crate) use crate::rpc::Response;

#[derive(Default)]
pub(crate) struct Decoder {
    reasoning_summary: crate::reasoning::Summary,
    rpc: crate::rpc::State,
    citations: super::citations::Citations,
    root: Option<String>,
    /// Followed child threads → the `Agent` item their items belong to.
    children: HashMap<String, String>,
    /// Spawn calls awaiting an explicit receiver thread relation.
    spawning: Vec<String>,
    /// Frames of child threads not bound yet, with their line sizes.
    deferred: HashMap<String, Vec<(Notification, usize)>>,
    deferred_bytes: usize,
    /// Child thread → text of its last agent message, the agent's result.
    child_replies: HashMap<String, TextContent>,
    items: HashMap<String, Tracked>,
    approvals: Approvals,
    questions: Questions,
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

/// Where a frame belongs: the root thread (`None`) or a child thread's
/// agent item (`Some`).
type Scope = Option<String>;

/// What Codex's `thread/start`, `thread/resume` and `turn/start` answer.
#[derive(serde::Deserialize)]
struct Started {
    thread: Option<StartedThread>,
    model: Option<String>,
    turn: Option<super::protocol::TurnId>,
}

#[derive(serde::Deserialize)]
struct StartedThread {
    id: String,
}

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
        let frame: Frame = serde_json::from_str(line).context("decoding Codex protocol JSON")?;
        let mut out = Vec::new();
        match (frame.method, frame.id) {
            (Some(method), Some(rpc)) => self.server_request(rpc, &method, &frame.params, &mut out),
            (Some(method), None) => match Notification::parse(&method, &frame.params) {
                Ok(notice) => self.notification(notice, line.len(), &mut out),
                // The run waits for its turn's end; one it can't read fails it.
                Err(error) if method == "turn/completed" => {
                    return Err(anyhow::Error::new(error).context(format!(
                        "reading Codex turn/completed of thread {:?}",
                        frame.params["threadId"].as_str()
                    )));
                }
                Err(error) => tracing::warn!(
                    method,
                    thread = frame.params["threadId"].as_str(),
                    item = frame.params["item"]["id"].as_str(),
                    %error,
                    "skipping a Codex notification Artisan can't read"
                ),
            },
            (None, Some(rpc)) => self.response(&rpc, frame.result, frame.error.as_ref(), &mut out),
            (None, None) => anyhow::bail!("Codex protocol frame has neither a method nor an id"),
        }
        let out = self.reasoning_summary.normalize(out, false);
        let out = self.citations.normalize(out);
        Ok(crate::retained::settle(self, out))
    }

    /// The user's answer to the approval or question on `key`.
    pub fn answer(&mut self, key: &str, answer: &artisan_api::Answer) -> Vec<EngineEvent> {
        let events = self.answer_raw(key, answer);
        crate::retained::settle(self, events)
    }

    fn response(
        &mut self,
        rpc: &Value,
        result: Option<Value>,
        error: Option<&Value>,
        out: &mut Vec<EngineEvent>,
    ) {
        let Some(response) = self.rpc.respond(rpc, result, error) else {
            return;
        };
        // `thread/start` and `thread/resume` answer with the thread;
        // `turn/start` with the turn.
        if let Ok(value) = &response.result
            && let Ok(started) = <Started as serde::Deserialize>::deserialize(value)
        {
            if let Some(thread) = started.thread {
                out.push(EngineEvent::Session {
                    id: thread.id.clone(),
                });
                self.root = Some(thread.id);
                if let Some(model) = started.model {
                    out.push(EngineEvent::Model { id: model });
                }
            }
            if let Some(turn) = started.turn {
                self.turn = Some(turn.id);
            }
        }
        self.rpc.responses.push(response);
    }

    /// The scope of a thread; `None` for threads we don't follow.
    fn scope(&self, thread: &str) -> Option<Scope> {
        if self.root.as_deref() == Some(thread) {
            return Some(None);
        }
        self.children.get(thread).map(|agent| Some(agent.clone()))
    }

    fn defer(&mut self, thread: String, notice: Notification, size: usize) {
        let frames = self.deferred.values().map(Vec::len).sum::<usize>();
        if self.deferred_bytes + size > DEFERRED_BYTES || frames >= DEFERRED_FRAMES {
            let message =
                "Codex omitted child spawn relations and exceeded the pending-event budget";
            tracing::warn!(thread, frames, bytes = self.deferred_bytes, message);
            self.turn_end = Some(Outcome::failed(message));
            self.deferred.clear();
            self.deferred_bytes = 0;
            return;
        }
        self.deferred_bytes += size;
        self.deferred
            .entry(thread)
            .or_default()
            .push((notice, size));
    }

    fn bind_child(&mut self, thread: &str, agent: &str, out: &mut Vec<EngineEvent>) {
        self.spawning.retain(|key| key != agent);
        self.children.insert(thread.to_owned(), agent.to_owned());
        for (notice, size) in self.deferred.remove(thread).into_iter().flatten() {
            self.deferred_bytes = self.deferred_bytes.saturating_sub(size);
            self.notification(notice, size, out);
        }
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one arm per notification Codex sends, each short"
    )]
    fn notification(&mut self, notice: Notification, size: usize, out: &mut Vec<EngineEvent>) {
        match notice {
            Notification::ThreadStarted(ThreadStarted { thread }) => {
                return self.thread_started(thread, size, out);
            }
            Notification::ThreadNameUpdated(update) => {
                if self.root.as_deref() == Some(update.thread_id.as_str())
                    && let Some(title) = update
                        .thread_name
                        .as_deref()
                        .map(str::trim)
                        .filter(|title| !title.is_empty())
                {
                    out.push(EngineEvent::Title {
                        title: title.to_owned(),
                    });
                }
                return;
            }
            Notification::Unknown => return,
            _ => {}
        }
        let Some(thread) = notice.thread().map(str::to_owned) else {
            return;
        };
        let Some(scope) = self.scope(&thread) else {
            if !self.spawning.is_empty() {
                self.defer(thread, notice, size);
            }
            return;
        };
        match notice {
            Notification::TurnStarted(started) => {
                if scope.is_none() {
                    self.turn = Some(started.turn.id);
                    self.unanswered = false;
                }
            }
            Notification::TurnCompleted(completed) => {
                self.turn_completed(&thread, scope, &completed.turn, out);
            }
            Notification::ItemStarted(notice) => self.item(&thread, scope, notice.item, false, out),
            Notification::ItemCompleted(notice) => {
                self.item(&thread, scope, notice.item, true, out);
            }
            Notification::TextDelta(delta) => {
                if let Some(text) = delta.text {
                    let Some(key) = delta.item_id.filter(|key| !key.is_empty()) else {
                        tracing::warn!(thread, "Codex text delta omitted itemId");
                        return;
                    };
                    out.push(live(LiveKind::Text { key }, text));
                }
            }
            Notification::ReasoningDelta(delta) => {
                if scope.is_none()
                    && let Some(text) = delta.text
                {
                    out.push(live(LiveKind::Reasoning, text));
                }
            }
            Notification::ReasoningPart(part) if scope.is_none() => match part.summary_index {
                Some(0) => out.push(EngineEvent::Live {
                    target: LiveKind::Reasoning,
                    text: String::new(),
                    append: false,
                }),
                Some(_) => out.push(live(LiveKind::Reasoning, "\n\n".to_owned())),
                None => tracing::warn!(thread, "Codex reasoning part omitted summaryIndex"),
            },
            Notification::OutputDelta(delta) => {
                if let Some(text) = delta.text {
                    let Some(key) = delta.item_id.filter(|key| !key.is_empty()) else {
                        tracing::warn!(thread, "Codex output delta omitted itemId");
                        return;
                    };
                    out.push(live(LiveKind::Shell { key }, text));
                }
            }
            Notification::McpProgress(progress) => {
                if let Some(text) = progress.message {
                    let Some(key) = progress.item_id.filter(|key| !key.is_empty()) else {
                        tracing::warn!(thread, "Codex MCP progress omitted itemId");
                        return;
                    };
                    out.push(EngineEvent::Live {
                        target: LiveKind::Progress { key },
                        text,
                        append: false,
                    });
                }
            }
            Notification::PlanUpdated(update) => self.plan(scope, &update.plan, out),
            Notification::TokenUsageUpdated(update) => {
                self.token_usage(scope.is_none(), &update.token_usage, out);
            }
            Notification::Rerouted(reroute) => {
                if [&reroute.turn_id, &reroute.from_model, &reroute.to_model]
                    .iter()
                    .any(|value| value.trim().is_empty())
                {
                    tracing::warn!(thread, "skipping a Codex reroute with blank turn or models");
                    return;
                }
                let text = format!(
                    "Codex moved this turn from {} to {} ({}).",
                    reroute.from_model, reroute.to_model, reroute.reason
                );
                self.notice(
                    &format!("reroute:{}", reroute.turn_id),
                    scope,
                    NoticeKind::Reroute {
                        model: reroute.to_model,
                    },
                    text,
                    out,
                );
            }
            Notification::SafetyBuffering(buffering) => {
                let key = format!("safety:{}", buffering.turn_id);
                if buffering.show_buffering_ui
                    && !buffering.turn_id.trim().is_empty()
                    && !self.items.contains_key(&key)
                {
                    self.notice(
                        &key,
                        scope,
                        NoticeKind::Safety,
                        "Codex is holding this reply for a safety check.",
                        out,
                    );
                }
            }
            Notification::GoalUpdated(update) => {
                if update.goal.objective.trim().is_empty() {
                    tracing::warn!(thread, "skipping a Codex goal with a blank objective");
                    return;
                }
                self.notice(
                    &goal_key(scope.as_ref()),
                    scope,
                    NoticeKind::Goal {
                        state: update.goal.status,
                    },
                    update.goal.objective,
                    out,
                );
            }
            Notification::GoalCleared(_) => {
                let key = goal_key(scope.as_ref());
                // Codex clears at every turn, goal or not: only a goal this
                // session showed is reported cleared.
                let text = match self.items.get(&key).map(|item| &item.kind) {
                    Some(ItemKind::Notice { text, .. }) if text.byte_len() > 0 => text.clone(),
                    _ => return,
                };
                let state = "cleared".to_owned();
                self.notice(&key, scope, NoticeKind::Goal { state }, text, out);
            }
            Notification::Error(notice) => {
                let refused = notice.error.is_refusal();
                let message = notice
                    .error
                    .message
                    .unwrap_or_else(|| "Codex reported an error".to_owned());
                if notice.will_retry {
                    out.push(EngineEvent::Live {
                        target: LiveKind::Status,
                        text: format!("{message} Retrying…"),
                        append: false,
                    });
                    return;
                }
                if scope.is_none() {
                    self.last_error = Some(message.clone());
                }
                let kind = if refused {
                    ErrorKind::Refusal
                } else {
                    ErrorKind::Api
                };
                self.error_item(scope, kind, &message, out);
            }
            Notification::Warning(warning) => {
                if let Some(message) = warning.message {
                    self.notices += 1;
                    let key = format!("warning-{}", self.notices);
                    self.notice(&key, scope, NoticeKind::Info, message, out);
                }
            }
            Notification::RequestResolved(resolved) => self.resolved(&resolved.request_id, out),
            Notification::ReasoningPart(_)
            | Notification::ThreadStarted(_)
            | Notification::ThreadNameUpdated(_)
            | Notification::Unknown => {}
        }
    }

    fn thread_started(&mut self, thread: ThreadInfo, size: usize, out: &mut Vec<EngineEvent>) {
        let Some(parent) = thread.parent_thread_id.clone() else {
            return;
        };
        if self.root.as_deref() == Some(thread.id.as_str()) {
            return;
        }
        if let Some(agent) = self.children.get(&thread.id).cloned() {
            if let Some(tracked) = self.items.get_mut(&agent)
                && let ItemKind::Agent { name, agent_id, .. } = &mut tracked.kind
            {
                let changed = agent_id.as_deref() != Some(thread.id.as_str())
                    || name.is_none() && thread.agent_nickname.is_some();
                *agent_id = Some(thread.id.clone());
                if name.is_none() {
                    name.clone_from(&thread.agent_nickname);
                }
                if changed {
                    out.push(self.event(&agent));
                }
            }
            return;
        }
        let Some(parent_scope) = self.scope(&parent) else {
            return;
        };
        if !self.spawning.is_empty() {
            let id = thread.id.clone();
            self.defer(
                id,
                Notification::ThreadStarted(ThreadStarted { thread }),
                size,
            );
            return;
        }
        // A child with no spawn call we saw is its own agent.
        let agent = thread.id.clone();
        let kind = ItemKind::Agent {
            name: thread.agent_nickname,
            description: TextContent::default(),
            prompt: None,
            agent_id: Some(thread.id.clone()),
            context_used: None,
            model: None,
            background: false,
            result: None,
            label: None,
        };
        self.track(agent.clone(), parent_scope, Status::Running, kind);
        out.push(self.event(&agent));
        self.bind_child(&thread.id, &agent, out);
    }

    fn turn_completed(
        &mut self,
        thread: &str,
        scope: Scope,
        turn: &CompletedTurn,
        out: &mut Vec<EngineEvent>,
    ) {
        let message = turn.error.as_ref().and_then(|error| error.message.clone());
        let Some(agent) = scope else {
            self.turn = None;
            let outcome = match turn.status {
                TurnStatus::Interrupted => Outcome::cancelled(),
                TurnStatus::Failed => {
                    let refused = turn.error.as_ref().is_some_and(CodexError::is_refusal);
                    let message = message.unwrap_or_else(|| "Codex failed this turn".to_owned());
                    if self.last_error.as_deref() != Some(message.as_str()) {
                        let kind = if refused {
                            ErrorKind::Refusal
                        } else {
                            ErrorKind::Api
                        };
                        self.error_item(None, kind, &message, out);
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
                TurnStatus::Completed => {
                    self.stranded = std::mem::take(&mut self.unanswered);
                    Outcome::done()
                }
            };
            self.unanswered = false;
            self.last_error = None;
            self.turn_end = Some(outcome);
            return;
        };
        let status = match turn.status {
            TurnStatus::Interrupted => Status::Cancelled,
            TurnStatus::Failed => Status::Failed,
            TurnStatus::Completed => Status::Done,
        };
        let reply = self
            .child_replies
            .remove(thread)
            .or_else(|| message.map(Into::into));
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

    #[expect(
        clippy::too_many_lines,
        reason = "one arm per item type Codex sends, each short"
    )]
    fn item(
        &mut self,
        thread: &str,
        scope: Scope,
        item: Item,
        completed: bool,
        out: &mut Vec<EngineEvent>,
    ) {
        if scope.is_none() {
            match (&item, completed) {
                (Item::UserMessage { .. }, true) => self.unanswered = true,
                (Item::UserMessage { .. }, false) | (_, true) => {}
                (_, false) => self.unanswered = false,
            }
        }
        let settled = if completed {
            Status::Done
        } else {
            Status::Running
        };
        let (key, status, kind) = match item {
            Item::AgentMessage {
                id,
                text,
                phase,
                questions,
            } => {
                let Some(text) = text else {
                    tracing::warn!(thread, item = id, "Codex agentMessage omitted its text");
                    return;
                };
                let phase = match phase {
                    Some(MessagePhase::Commentary) => Phase::Commentary,
                    _ => Phase::Final,
                };
                if completed {
                    if scope.is_some() && !text.is_empty() {
                        // Kept until the child's turn ends; Forge has the text.
                        let mut reply = TextContent::from(text.as_str());
                        forge_content::compact_text(&mut reply);
                        self.child_replies.insert(thread.to_owned(), reply);
                    }
                    self.async_questions(&id, scope.clone(), questions, out);
                }
                let kind = ItemKind::AssistantMessage {
                    text: text.into(),
                    phase,
                };
                (id, settled, kind)
            }
            Item::Plan { id, text } => {
                let Some(text) = text else {
                    tracing::warn!(thread, item = id, "Codex plan omitted its text");
                    return;
                };
                let kind = ItemKind::AssistantMessage {
                    text: text.into(),
                    phase: Phase::Final,
                };
                (id, settled, kind)
            }
            Item::UserMessage { content, .. } => {
                // Ours are stored by Forge. A child's first message is its
                // task; keep it when the spawn call didn't carry one.
                if let Some(agent) = &scope
                    && let Some(tracked) = self.items.get_mut(agent)
                    && let ItemKind::Agent { prompt: None, .. } = &tracked.kind
                    && let Some(text) = user_text(content)
                {
                    if let ItemKind::Agent { prompt, .. } = &mut tracked.kind {
                        *prompt = Some(text.into());
                    }
                    out.push(self.event(agent));
                }
                return;
            }
            Item::CommandExecution {
                id,
                status,
                command,
                cwd,
                aggregated_output,
                exit_code,
            } => {
                let Some(status) = execution_status(status) else {
                    return;
                };
                let Some(command) = command.filter(|command| !command.trim().is_empty()) else {
                    if completed {
                        self.malformed(
                            scope,
                            &id,
                            "Codex commandExecution omitted its command",
                            out,
                        );
                    }
                    return;
                };
                let tool = ToolKind::Shell {
                    command: display_command(&command).into(),
                    cwd,
                    background: false,
                };
                let output = aggregated_output
                    .filter(|text| !text.is_empty())
                    .map(|text| Output::from_text(&text));
                let exit_code = exit_code.and_then(|code| i32::try_from(code).ok());
                let kind = self.tool_kind(&id, tool, output, exit_code);
                (id, status, kind)
            }
            Item::FileChange {
                id,
                status,
                changes,
            } => {
                let Some(status) = execution_status(status) else {
                    return;
                };
                let Some(changes) = changes else {
                    if completed {
                        self.malformed(scope, &id, "Codex fileChange omitted its changes", out);
                    }
                    return;
                };
                if changes.iter().any(|change| change.path.trim().is_empty()) {
                    if completed {
                        let message = "Codex fileChange supplied an invalid file change";
                        self.malformed(scope, &id, message, out);
                    }
                    return;
                }
                let changes = changes.into_iter().map(file_change).collect::<Vec<_>>();
                let tool = ToolKind::Edit {
                    changes: changes.into(),
                };
                let kind = self.tool_kind(&id, tool, None, None);
                (id, status, kind)
            }
            Item::McpToolCall {
                id,
                status,
                server,
                tool,
                arguments,
                result,
                error,
            } => {
                let Some(status) = call_status(status, None, completed) else {
                    return;
                };
                let (Some(server), Some(tool)) = (
                    server.filter(|value| !value.is_empty()),
                    tool.filter(|value| !value.is_empty()),
                ) else {
                    tracing::warn!(
                        thread,
                        item = id,
                        "Codex MCP call omitted server/tool identity"
                    );
                    return;
                };
                let output = error
                    .and_then(|error| error.message)
                    .or_else(|| {
                        result.and_then(|result| crate::native::content_text(&result.content))
                    })
                    .map(|text| Output::from_text(&text));
                let tool = ToolKind::Mcp {
                    server,
                    tool,
                    input: arguments_text(arguments).map(Into::into),
                };
                let kind = self.tool_kind(&id, tool, output, None);
                (id, status, kind)
            }
            Item::DynamicToolCall {
                id,
                status,
                success,
                tool,
                arguments,
                content_items,
            } => {
                // A question Codex asked through `requestUserInput` settles
                // when the tool call behind it ends.
                if matches!(
                    self.items.get(&id).map(|tracked| &tracked.kind),
                    Some(ItemKind::Question { .. })
                ) {
                    if completed && let Some(status) = call_status(status, success, completed) {
                        self.questions.remove(&id);
                        if let Some(tracked) = self.items.get_mut(&id) {
                            let next = tracked.status.advance(status);
                            if next != tracked.status {
                                tracked.status = next;
                                out.push(EngineEvent::Status {
                                    key: id,
                                    status: next,
                                });
                            }
                        }
                    }
                    return;
                }
                let Some(status) = call_status(status, success, completed) else {
                    return;
                };
                let Some(name) = tool.filter(|name| !name.trim().is_empty()) else {
                    if completed {
                        self.malformed(
                            scope,
                            &id,
                            "Codex dynamicToolCall omitted its tool name",
                            out,
                        );
                    }
                    return;
                };
                let output = crate::native::content_text(&content_items)
                    .map(|text| Output::from_text(&text));
                let tool = ToolKind::Other {
                    name,
                    input: arguments_text(arguments).map(Into::into),
                };
                let kind = self.tool_kind(&id, tool, output, None);
                (id, status, kind)
            }
            Item::WebSearch { id, query, action } => {
                let known = match self.items.get(&id).map(|tracked| &tracked.kind) {
                    Some(ItemKind::Tool {
                        tool: ToolKind::WebSearch { query },
                        ..
                    }) => Some(query.clone()),
                    _ => None,
                };
                let Some(query) = web_query(query, action).or(known) else {
                    // Codex starts searches without metadata and can complete
                    // an opaque action as query:"", action:{type:"other"}.
                    // Neither is an execution failure: wait for a query, or
                    // leave this optional card out.
                    tracing::debug!(
                        thread,
                        item = id,
                        completed,
                        "Codex web action has no display metadata"
                    );
                    return;
                };
                let kind = self.tool_kind(&id, ToolKind::WebSearch { query }, None, None);
                (id, settled, kind)
            }
            Item::ImageView { id, path } => {
                let Some(path) = path.filter(|path| !path.trim().is_empty()) else {
                    if completed {
                        self.malformed(scope, &id, "Codex imageView omitted its path", out);
                    }
                    return;
                };
                let kind = self.tool_kind(&id, ToolKind::ViewImage { path }, None, None);
                (id, settled, kind)
            }
            Item::ImageGeneration {
                id,
                saved_path,
                revised_prompt,
            } => {
                if !completed {
                    return;
                }
                if let Some(path) = saved_path.filter(|path| !path.is_empty()) {
                    out.push(EngineEvent::Image {
                        key: id,
                        parent: scope,
                        path: path.into(),
                        caption: revised_prompt,
                    });
                } else {
                    tracing::warn!(
                        thread,
                        item = id,
                        "Codex completed image generation without an image path"
                    );
                }
                return;
            }
            Item::ContextCompaction { id } => {
                let tokens_before = match self.items.get(&id).map(|tracked| &tracked.kind) {
                    Some(ItemKind::Compaction { tokens_before, .. }) => *tokens_before,
                    _ => self.usage.context_used,
                };
                let kind = ItemKind::Compaction {
                    tokens_before,
                    tokens_after: None,
                };
                (id, settled, kind)
            }
            Item::CollabAgentToolCall(call) => return self.collab(scope, call, completed, out),
            Item::Sleep { id, duration_ms } => {
                let seconds = duration_ms.unwrap_or_default().div_ceil(1000);
                let mut kind = self.tool_kind(
                    &id,
                    ToolKind::Other {
                        name: "sleep".to_owned(),
                        input: None,
                    },
                    None,
                    None,
                );
                if let ItemKind::Tool { title, .. } = &mut kind {
                    *title = Some(format!("{seconds} s"));
                }
                (id, settled, kind)
            }
            Item::Unknown => return,
        };
        if key.is_empty() {
            tracing::warn!(thread, "Codex item supplied an empty id");
            return;
        }
        if completed {
            // Codex ended the item; nothing it asked about it waits anymore.
            self.approvals.remove(&key);
            self.questions.remove(&key);
        }
        let status = if !completed && self.approvals.contains_key(&key) {
            Status::AwaitingUser
        } else {
            status
        };
        self.track(key.clone(), scope, status, kind);
        out.push(self.event(&key));
    }

    /// A tool kind for `key`, keeping the title and approval it has.
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
        scope: Scope,
        call: CollabCall,
        completed: bool,
        out: &mut Vec<EngineEvent>,
    ) {
        let key = call.id;
        if call.tool != CollabTool::SpawnAgent {
            let status = match call.status {
                CollabStatus::Failed => Status::Failed,
                CollabStatus::Interrupted => Status::Cancelled,
                _ if completed => Status::Done,
                _ => Status::Running,
            };
            let mut waited: Vec<String> = call
                .agents_states
                .keys()
                .filter(|thread| !call.receiver_thread_ids.contains(thread))
                .cloned()
                .collect();
            waited.sort();
            let mut addresses = call.receiver_thread_ids.clone();
            addresses.extend(waited);
            let tool = match call.tool {
                CollabTool::Wait => Some(ToolKind::Agents {
                    action: AgentAction::Wait,
                    addresses,
                    targets: Vec::new(),
                }),
                CollabTool::SendInput => addresses.first().map(|to| ToolKind::AgentMessage {
                    recipient: to.clone(),
                    target: None,
                    summary: None,
                    text: call.prompt.clone().unwrap_or_default().into(),
                    direction: MessageDirection::Sent,
                }),
                CollabTool::SpawnAgent | CollabTool::Other => None,
            };
            if let Some(tool) = tool {
                let kind = self.tool_kind(&key, tool, None, None);
                self.track(key.clone(), scope.clone(), status, kind);
                out.push(self.event(&key));
            }
            // `wait` reports what the agents said; fill results we missed.
            for (thread, state) in call.agents_states {
                let (Some(agent), Some(message)) = (self.children.get(&thread), state.message)
                else {
                    continue;
                };
                let agent = agent.clone();
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
                description: call.description.unwrap_or_default().into(),
                prompt: call.prompt.map(Into::into),
                agent_id: None,
                context_used: None,
                model: None,
                background: false,
                result: None,
                label: None,
            };
            self.track(key.clone(), scope, Status::Running, kind);
            if call.receiver_thread_ids.is_empty() {
                self.spawning.push(key.clone());
            }
            out.push(self.event(&key));
        }
        for thread in call.receiver_thread_ids {
            if !thread.trim().is_empty() && !self.children.contains_key(&thread) {
                self.bind_child(&thread, &key, out);
            }
        }
        let terminal = match call.status {
            CollabStatus::Failed => Some(Status::Failed),
            CollabStatus::Interrupted => Some(Status::Cancelled),
            CollabStatus::InProgress | CollabStatus::Completed | CollabStatus::Unknown => None,
        };
        if let Some(status) = terminal.filter(|_| completed) {
            self.spawning.retain(|spawn| *spawn != key);
            if let Some(tracked) = self.items.get_mut(&key) {
                tracked.status = tracked.status.advance(status);
            }
            out.push(self.event(&key));
        }
    }

    fn plan(&mut self, scope: Scope, plan: &Value, out: &mut Vec<EngineEvent>) {
        let entries = match <Vec<PlanEntry> as serde::Deserialize>::deserialize(plan) {
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
                text: entry.step.into(),
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
        let kind = ItemKind::Plan {
            steps: steps.into(),
            proposal: None,
            approval: None,
        };
        self.track(key.clone(), scope, status, kind);
        out.push(self.event(&key));
    }

    fn token_usage(&mut self, root: bool, usage: &TokenUsage, out: &mut Vec<EngineEvent>) {
        // `last` is one model call; `total` is the whole thread, history
        // included on resume, so the run sums `last`.
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

    /// A completed item Codex left a required field out of: an error the
    /// user sees, never an empty card.
    fn malformed(&mut self, scope: Scope, item: &str, message: &str, out: &mut Vec<EngineEvent>) {
        tracing::warn!(item, message, "Codex sent a malformed item");
        self.error_item(scope, ErrorKind::Execution, message, out);
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
        let kind = ItemKind::Error {
            kind,
            message: message.to_owned().into(),
            retryable: false,
        };
        self.track(key.clone(), scope, Status::Done, kind);
        out.push(self.event(&key));
    }

    fn notice(
        &mut self,
        key: &str,
        scope: Scope,
        kind: NoticeKind,
        text: impl Into<TextContent>,
        out: &mut Vec<EngineEvent>,
    ) {
        self.items.remove(key);
        let kind = ItemKind::Notice {
            kind,
            text: text.into(),
        };
        self.track(key.to_owned(), scope, Status::Done, kind);
        out.push(self.event(key));
    }

    /// Settles what is still open when the run ends: plans keep their
    /// steps, everything else stopped with the run.
    pub fn finish(&mut self) -> Vec<EngineEvent> {
        self.approvals.clear();
        self.questions.clear();
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
                let tracked = self.items.get_mut(&key).expect("collected above");
                let status = if matches!(tracked.kind, ItemKind::Plan { .. }) {
                    Status::Done
                } else {
                    Status::Cancelled
                };
                tracked.status = status;
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

impl crate::retained::Retains for Decoder {
    fn retained(&mut self, key: &str) -> Option<&mut ItemKind> {
        self.items.get_mut(key).map(|tracked| &mut tracked.kind)
    }
}

fn live(target: LiveKind, text: String) -> EngineEvent {
    EngineEvent::Live {
        target,
        text,
        append: true,
    }
}

fn goal_key(scope: Option<&String>) -> String {
    scope.map_or_else(|| "goal".to_owned(), |agent| format!("goal:{agent}"))
}

/// A shell or patch item's status; `None` when Codex sent none we know.
fn execution_status(status: Option<ExecutionStatus>) -> Option<Status> {
    Some(match status? {
        ExecutionStatus::InProgress => Status::Running,
        ExecutionStatus::Completed => Status::Done,
        ExecutionStatus::Failed => Status::Failed,
        ExecutionStatus::Declined => Status::Declined,
        ExecutionStatus::Unknown => return None,
    })
}

/// An MCP or dynamic call's status: these have no `declined`, and a
/// dynamic call that completed unsuccessfully failed.
fn call_status(
    status: Option<CallStatus>,
    success: Option<bool>,
    completed: bool,
) -> Option<Status> {
    let status = match status? {
        CallStatus::InProgress => Status::Running,
        CallStatus::Completed => Status::Done,
        CallStatus::Failed => Status::Failed,
        CallStatus::Unknown => return None,
    };
    Some(if completed && success == Some(false) {
        Status::Failed
    } else {
        status
    })
}

/// What a web action looked up: the first query or address it names.
fn web_query(query: Option<String>, action: Option<super::protocol::WebAction>) -> Option<String> {
    let action = action.unwrap_or(super::protocol::WebAction {
        query: None,
        url: None,
        queries: Vec::new(),
    });
    [query, action.query, action.url]
        .into_iter()
        .flatten()
        .find(|value| !value.trim().is_empty())
        .or_else(|| {
            let queries: Vec<&str> = action
                .queries
                .iter()
                .filter_map(Value::as_str)
                .filter(|query| !query.trim().is_empty())
                .collect();
            (!queries.is_empty()).then(|| queries.join("; "))
        })
}

/// A child's first message as text; images and mentions carry none.
fn user_text(content: Vec<UserInput>) -> Option<String> {
    let text: Vec<String> = content
        .into_iter()
        .filter_map(|part| match part {
            UserInput::Text { text } => Some(text),
            UserInput::Other => None,
        })
        .collect();
    (!text.is_empty()).then(|| text.join("\n"))
}

/// Tool arguments as compact JSON; nothing for an empty call.
fn arguments_text(value: Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::Object(map) if map.is_empty() => None,
        Value::String(text) => Some(text),
        other => Some(other.to_string()),
    }
}

fn file_change(change: super::protocol::FileChange) -> FileChange {
    let (path, op) = match change.kind {
        PatchKind::Add => (change.path, FileOp::Add),
        PatchKind::Delete => (change.path, FileOp::Delete),
        PatchKind::Update {
            move_path: Some(to),
        } => (to, FileOp::Move { from: change.path }),
        PatchKind::Update { move_path: None } => (change.path, FileOp::Modify),
    };
    FileChange {
        path,
        op,
        diff: (!change.diff.is_empty()).then(|| change.diff.into()),
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
