//! Clearing a thread's goal when no run has the thread loaded.
//!
//! `thread/goal/clear` needs the thread loaded on the shared app-server, so
//! this resumes it (identity only, no history) and clears; the thread then
//! unloads with the other idle ones.

use super::protocol::Frame;
use super::{server, thread_params};
use crate::Turn;
use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};
use std::time::Duration;

/// How long the clear may take before it is given up on.
const LIMIT: Duration = Duration::from_secs(30);

pub(super) async fn clear_goal(turn: Turn) -> Result<()> {
    let thread = turn
        .session_id
        .clone()
        .context("clearing a goal needs the thread's Codex session")?;
    let mut conn = server::attach(&turn)
        .await
        .context("attaching to the Codex app-server")?;
    let clear = async {
        let (method, params) = thread_params(&turn);
        call(&mut conn, 1, method, params).await?;
        call(
            &mut conn,
            2,
            "thread/goal/clear",
            json!({ "threadId": thread }),
        )
        .await
    };
    tokio::time::timeout(LIMIT, clear)
        .await
        .map_err(|_elapsed| anyhow!("Codex didn't clear the goal in {}s", LIMIT.as_secs()))?
        .map(drop)
}

/// Sends a request and waits for its result, skipping what Codex sends
/// meanwhile.
pub(super) async fn call(
    conn: &mut server::Attachment,
    id: u64,
    method: &str,
    params: Value,
) -> Result<Value> {
    conn.send(json!({ "id": id, "method": method, "params": params }))?;
    loop {
        let line = conn
            .next_line()
            .await
            .ok_or_else(|| anyhow!("Codex exited while waiting for {method}: {}", conn.ended()))?;
        let Ok(frame) = serde_json::from_str::<Frame>(&line) else {
            continue;
        };
        if let (Some(_), Some(request)) = (&frame.method, &frame.id) {
            // Resuming asks nothing; anything that does is refused.
            conn.send(json!({
                "id": request,
                "error": { "code": crate::rpc::METHOD_NOT_FOUND, "message": "this operation takes no server requests" },
            }))?;
            continue;
        }
        if frame.method.is_none() && frame.id == Some(json!(id)) {
            if let Some(error) = frame.error {
                bail!("Codex {method}: {error}");
            }
            return Ok(frame.result.unwrap_or(Value::Null));
        }
    }
}
