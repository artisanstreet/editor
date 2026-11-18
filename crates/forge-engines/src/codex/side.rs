use super::goal::CallError;
use super::protocol::{Frame, Item, Notification, TurnStatus};
use super::server::{self, AttachError, Ended};
use super::turn_overrides;
use crate::{SideReply, SideUsage, Turn};
use serde_json::{Value, json};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

const LIMIT: Duration = Duration::from_secs(120);

#[derive(Debug, thiserror::Error)]
pub enum SideQueryError {
    #[error("attach to the Codex app-server")]
    Attach(#[source] AttachError),
    #[error("the side query was cancelled")]
    Cancelled,
    #[error("the Codex fork gave no answer in {}s", after.as_secs())]
    TimedOut { after: Duration },
    #[error("ask Codex {method}")]
    Call {
        method: &'static str,
        #[source]
        source: CallError,
    },
    #[error("Codex {method} answered without a thread")]
    NoThread { method: &'static str },
    #[error("refuse a request in fork {fork}")]
    Refuse {
        fork: String,
        #[source]
        source: Ended,
    },
    #[error("read the end of fork {fork}'s turn")]
    TurnEnd {
        fork: String,
        #[source]
        source: serde_json::Error,
    },
    #[error("Codex fork {fork} failed: {message}")]
    Failed { fork: String, message: String },
    #[error("Codex fork {fork} was interrupted")]
    Interrupted { fork: String },
    #[error("Codex exited during the side query: {ended}")]
    Exited { ended: String },
    #[error("decode Codex output during the side query")]
    Decode(#[source] serde_json::Error),
}

pub(super) async fn side_query(
    turn: Turn,
    cancel: CancellationToken,
) -> Result<SideReply, SideQueryError> {
    query(turn, cancel, LIMIT).await
}

pub(super) async fn compact(
    turn: Turn,
    cancel: CancellationToken,
) -> Result<SideReply, SideQueryError> {
    query(turn, cancel, Duration::from_secs(300)).await
}

async fn query(
    turn: Turn,
    cancel: CancellationToken,
    deadline: Duration,
) -> Result<SideReply, SideQueryError> {
    let thread = turn.session_id.clone();
    let mut conn = server::attach(&turn)
        .await
        .map_err(SideQueryError::Attach)?;
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
        () = cancel.cancelled() => Err(SideQueryError::Cancelled),
        () = tokio::time::sleep(deadline) => Err(SideQueryError::TimedOut { after: deadline }),
    };
    if let Some(forked) = fork.thread.take() {
        if let Some(turn) = fork.turn.take() {
            let interrupted = tokio::time::timeout(
                Duration::from_secs(5),
                fork.call(
                    "turn/interrupt",
                    json!({ "threadId": forked, "turnId": turn }),
                ),
            )
            .await;
            match interrupted {
                Ok(Ok(_)) => {}
                Ok(Err(failure)) => {
                    let error: &dyn std::error::Error = &failure;
                    tracing::debug!(fork = forked, error, "couldn't interrupt a Codex fork");
                }
                Err(_elapsed) => tracing::debug!(fork = forked, "a Codex fork didn't confirm its interrupt"),
            }
        }
        let id = fork.next_id;
        if let Err(failure) = fork.conn.send(json!({
            "id": id,
            "method": "thread/unsubscribe",
            "params": { "threadId": forked },
        })) {
            let error: &dyn std::error::Error = &failure;
            tracing::debug!(fork = forked, error, "the Codex server ended with the fork");
        }
    }
    answer
}

struct Fork<'a> {
    conn: &'a mut server::Attachment,
    next_id: u64,
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
    async fn ask(&mut self, turn: &Turn, thread: Option<&str>) -> Result<SideReply, SideQueryError> {
        let (method, forked) = if let Some(thread) = thread {
            let method = "thread/fork";
            let forked = self
                .call(
                    method,
                    json!({
                        "threadId": thread, "ephemeral": true, "excludeTurns": true,
                    }),
                )
                .await?;
            (method, forked)
        } else {
            let (_, mut params) = super::thread_params(turn);
            params["ephemeral"] = json!(true);
            params["approvalPolicy"] = json!("never");
            params["sandbox"] = json!("read-only");
            if let Some(params) = params.as_object_mut() {
                params.remove("approvalsReviewer");
            }
            let method = "thread/start";
            (method, self.call(method, params).await?)
        };
        let forked = forked["thread"]["id"]
            .as_str()
            .ok_or(SideQueryError::NoThread { method })?
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
                self.conn
                    .send(json!({
                        "id": id,
                        "error": { "code": crate::rpc::METHOD_NOT_FOUND, "message": "a side query takes no requests" },
                    }))
                    .map_err(|source| SideQueryError::Refuse {
                        fork: forked.clone(),
                        source,
                    })?;
                continue;
            }
            let notification = match Notification::parse(method, &frame.params) {
                Ok(notification) => notification,
                Err(source)
                    if method == "turn/completed"
                        && frame.params["threadId"].as_str() == Some(forked.as_str()) =>
                {
                    return Err(SideQueryError::TurnEnd {
                        fork: forked,
                        source,
                    });
                }
                Err(_unread) => continue,
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
                    return match notice.turn.status {
                        TurnStatus::Failed => Err(SideQueryError::Failed {
                            fork: forked,
                            message: notice
                                .turn
                                .error
                                .and_then(|error| error.message)
                                .unwrap_or_else(|| "no reason given".to_owned()),
                        }),
                        TurnStatus::Interrupted => Err(SideQueryError::Interrupted { fork: forked }),
                        TurnStatus::Completed => Ok(SideReply {
                            text: self.text.take().unwrap_or_default().trim().to_owned(),
                            usage: self.usage,
                        }),
                    };
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

    async fn call(&mut self, method: &'static str, params: Value) -> Result<Value, SideQueryError> {
        let id = self.next_id;
        self.next_id += 1;
        super::goal::call(self.conn, id, method, params)
            .await
            .map_err(|source| SideQueryError::Call { method, source })
    }

    async fn next(&mut self) -> Result<Frame, SideQueryError> {
        let Some(line) = self.conn.next_line().await else {
            return Err(SideQueryError::Exited {
                ended: self.conn.ended(),
            });
        };
        serde_json::from_str::<Frame>(&line).map_err(SideQueryError::Decode)
    }
}
