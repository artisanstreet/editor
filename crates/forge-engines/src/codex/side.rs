//! Side queries: one question to an ephemeral fork of a Codex thread.
//!
//! `thread/fork` with `ephemeral` copies the thread into one Codex never
//! saves, on the same shared app-server, so the thread itself is never
//! touched. The fork's turn keeps the thread's model and effort, so its
//! request starts with the thread's prefix and reads its prompt cache; it
//! runs read-only and never asks, so nothing can change. The fork is
//! unsubscribed once it answered.
//!
//! Measured against Codex 0.163.0: a fork reads the thread's whole cached
//! prefix and adds about 5k tokens of its own after it, which the first
//! fork of a point in the thread pays for. The read-only override costs
//! nothing, and `experimentalPredictionMode` read no more than a plain fork.

use super::protocol::{Frame, Item, Notification, TurnStatus};
use super::{server, turn_overrides};
use crate::{SideReply, SideUsage, Turn};
use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// How long a side query may take before it is given up on.
const LIMIT: Duration = Duration::from_secs(120);

pub(super) async fn side_query(turn: Turn, cancel: CancellationToken) -> Result<SideReply> {
    query(turn, cancel, LIMIT).await
}

pub(super) async fn compact(turn: Turn, cancel: CancellationToken) -> Result<SideReply> {
    query(turn, cancel, std::time::Duration::from_secs(300)).await
}

async fn query(
    turn: Turn,
    cancel: CancellationToken,
    deadline: std::time::Duration,
) -> Result<SideReply> {
    let thread = turn.session_id.clone();
    let mut conn = server::attach(&turn)
        .await
        .context("attaching to the Codex app-server")?;
    let mut fork = Fork {
        conn: &mut conn,
        next_id: 1,
        thread: None,
        turn: None,
        text: None,
        usage: SideUsage::default(),
    };
    let answer = tokio::select! {
        answer = fork.ask(&turn, thread.as_deref()) => answer,
        () = cancel.cancelled() => Err(anyhow!("the side query was cancelled")),
        () = tokio::time::sleep(deadline) => Err(anyhow!("the Codex fork gave no answer in {}s", deadline.as_secs())),
    };
    if let Some(forked) = fork.thread.take() {
        if let Some(turn) = fork.turn.take() {
            tokio::time::timeout(
                Duration::from_secs(5),
                fork.call(
                    "turn/interrupt",
                    json!({ "threadId": forked, "turnId": turn }),
                ),
            )
            .await
            .ok();
        }
        let id = fork.next_id;
        // Best effort: a server that has ended dropped the fork with it.
        fork.conn
            .send(json!({
                "id": id,
                "method": "thread/unsubscribe",
                "params": { "threadId": forked },
            }))
            .ok();
    }
    answer
}

struct Fork<'a> {
    conn: &'a mut server::Attachment,
    next_id: u64,
    /// The fork's thread id, once Codex made it.
    thread: Option<String>,
    turn: Option<String>,
    text: Option<String>,
    usage: SideUsage,
}

impl Fork<'_> {
    #[expect(
        clippy::too_many_lines,
        reason = "one forked turn and its exhaustive notification loop"
    )]
    async fn ask(&mut self, turn: &Turn, thread: Option<&str>) -> Result<SideReply> {
        let forked = if let Some(thread) = thread {
            self.call(
                "thread/fork",
                json!({
                    "threadId": thread, "ephemeral": true, "excludeTurns": true,
                }),
            )
            .await?
        } else {
            let (_, mut params) = super::thread_params(turn);
            params["ephemeral"] = json!(true);
            params["approvalPolicy"] = json!("never");
            params["sandbox"] = json!("read-only");
            if let Some(params) = params.as_object_mut() {
                params.remove("approvalsReviewer");
            }
            self.call("thread/start", params).await?
        };
        let forked = forked["thread"]["id"]
            .as_str()
            .context("Codex thread/fork answered without a thread")?
            .to_owned();
        self.thread = Some(forked.clone());
        let mut params = json!({
            "threadId": forked,
            "input": [{ "type": "text", "text": turn.input.text, "text_elements": [] }],
        });
        turn_overrides(turn, &mut params);
        params["approvalPolicy"] = json!("never");
        params["sandboxPolicy"] = json!({ "type": "readOnly" });
        if let Some(params) = params.as_object_mut() {
            params.remove("approvalsReviewer");
        }
        let started = self.call("turn/start", params).await?;
        self.turn = started["turn"]["id"].as_str().map(str::to_owned);
        loop {
            let frame = self.next().await?;
            let Some(method) = frame.method.as_deref() else {
                continue;
            };
            if let Some(id) = frame.id {
                // Never asks under `never`; anything that does is refused.
                self.conn.send(json!({
                    "id": id,
                    "error": { "code": crate::rpc::METHOD_NOT_FOUND, "message": "a side query takes no requests" },
                }))?;
                continue;
            }
            let Ok(notification) = Notification::parse(method, &frame.params) else {
                continue;
            };
            if notification.thread() != Some(forked.as_str()) {
                continue;
            }
            match notification {
                Notification::ItemCompleted(notice) => {
                    if let Item::AgentMessage {
                        text: Some(text), ..
                    } = notice.item
                    {
                        self.text = Some(text);
                    }
                }
                Notification::TokenUsageUpdated(notice) => {
                    let last = notice.token_usage.last;
                    self.usage.input += last.input_tokens;
                    self.usage.cached_input += last.cached_input_tokens;
                    self.usage.output += last.output_tokens;
                }
                Notification::TurnCompleted(notice) => {
                    self.turn = None;
                    if matches!(notice.turn.status, TurnStatus::Failed) {
                        let message = notice
                            .turn
                            .error
                            .and_then(|error| error.message)
                            .unwrap_or_else(|| "no reason given".to_owned());
                        bail!("the Codex fork failed: {message}");
                    }
                    return Ok(SideReply {
                        text: self.text.take().unwrap_or_default().trim().to_owned(),
                        usage: self.usage,
                    });
                }
                Notification::ThreadStarted(_)
                | Notification::ThreadNameUpdated(_)
                | Notification::TurnStarted(_)
                | Notification::ItemStarted(_)
                | Notification::TextDelta(_)
                | Notification::ReasoningDelta(_)
                | Notification::ReasoningPart(_)
                | Notification::OutputDelta(_)
                | Notification::McpProgress(_)
                | Notification::PlanUpdated(_)
                | Notification::Rerouted(_)
                | Notification::SafetyBuffering(_)
                | Notification::GoalUpdated(_)
                | Notification::GoalCleared(_)
                | Notification::Error(_)
                | Notification::Warning(_)
                | Notification::RequestResolved(_)
                | Notification::Unknown => {}
            }
        }
    }

    /// Sends a request and waits for its result.
    async fn call(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        self.conn
            .send(json!({ "id": id, "method": method, "params": params }))?;
        loop {
            let frame = self.next().await?;
            if frame.method.is_none() && frame.id == Some(json!(id)) {
                if let Some(error) = frame.error {
                    bail!("Codex {method}: {error}");
                }
                return Ok(frame.result.unwrap_or(Value::Null));
            }
        }
    }

    async fn next(&mut self) -> Result<Frame> {
        loop {
            let line = self.conn.next_line().await.ok_or_else(|| {
                anyhow!("Codex exited during a side query: {}", self.conn.ended())
            })?;
            if let Ok(frame) = serde_json::from_str::<Frame>(&line) {
                return Ok(frame);
            }
        }
    }
}
