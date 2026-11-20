use super::protocol::Frame;
use super::server::{self, AttachError, Ended};
use super::thread_params;
use crate::Turn;
use artisan_api::ThreadId;
use serde_json::{Value, json};
use std::time::Duration;

const LIMIT: Duration = Duration::from_secs(30);

#[derive(Debug, thiserror::Error)]
pub enum CallError {
    #[error("send {method}")]
    Send {
        method: String,
        #[source]
        source: Ended,
    },
    #[error("Codex exited while waiting for {method}: {ended}")]
    Exited { method: String, ended: String },
    #[error("decode Codex output while waiting for {method}")]
    Decode {
        method: String,
        #[source]
        source: serde_json::Error,
    },
    #[error("Codex refused {method}")]
    Refused {
        method: String,
        #[source]
        refusal: crate::rpc::RpcError,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum ClearGoalError {
    #[error("thread {thread} has no Codex session")]
    NoSession { thread: ThreadId },
    #[error("attach to the Codex app-server")]
    Attach(#[source] AttachError),
    #[error("Codex didn't clear the goal of session {session} in {}s", LIMIT.as_secs())]
    TimedOut { session: String },
    #[error("clear the goal of session {session}")]
    Call {
        session: String,
        #[source]
        source: CallError,
    },
}

pub(super) async fn clear_goal(turn: Turn) -> Result<(), ClearGoalError> {
    let session = turn
        .session_id
        .clone()
        .ok_or_else(|| ClearGoalError::NoSession {
            thread: turn.thread_id.clone(),
        })?;
    let mut conn = server::attach(&turn)
        .await
        .map_err(ClearGoalError::Attach)?;
    let clear = async {
        let (method, params) = thread_params(&turn, super::ProjectGuard::of(&turn));
        call(&mut conn, 1, method, params).await?;
        call(
            &mut conn,
            2,
            "thread/goal/clear",
            json!({ "threadId": session }),
        )
        .await
    };
    match tokio::time::timeout(LIMIT, clear).await {
        Err(_elapsed) => Err(ClearGoalError::TimedOut { session }),
        Ok(Err(source)) => Err(ClearGoalError::Call { session, source }),
        Ok(Ok(_)) => Ok(()),
    }
}

pub(super) async fn call(
    conn: &mut server::Attachment,
    id: u64,
    method: &str,
    params: Value,
) -> Result<Value, CallError> {
    let send = |conn: &server::Attachment, message: Value| {
        conn.send(message).map_err(|source| CallError::Send {
            method: method.to_owned(),
            source,
        })
    };
    send(
        conn,
        json!({ "id": id, "method": method, "params": params }),
    )?;
    loop {
        let Some(line) = conn.next_line().await else {
            return Err(CallError::Exited {
                method: method.to_owned(),
                ended: conn.ended(),
            });
        };
        let frame = serde_json::from_str::<Frame>(&line).map_err(|source| CallError::Decode {
            method: method.to_owned(),
            source,
        })?;
        if let (Some(_), Some(request)) = (&frame.method, &frame.id) {
            send(
                conn,
                json!({
                    "id": request,
                    "error": { "code": crate::rpc::METHOD_NOT_FOUND, "message": "this operation takes no server requests" },
                }),
            )?;
            continue;
        }
        if frame.method.is_none() && frame.id == Some(json!(id)) {
            if let Some(error) = &frame.error {
                return Err(CallError::Refused {
                    method: method.to_owned(),
                    refusal: crate::rpc::RpcError::refusal(error),
                });
            }
            return Ok(frame.result.unwrap_or(Value::Null));
        }
    }
}
