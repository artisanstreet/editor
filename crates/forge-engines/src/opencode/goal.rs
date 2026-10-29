use super::decoder::{Decoder, Goal};
use super::{Api, READY_TIMEOUT, emit_all, password, ready, spawn, write_agents};
use crate::{RunIo, Turn};
use anyhow::{Context as _, Result};
use serde_json::{Value, json};

pub(super) fn argument(text: &str) -> Option<&str> {
    let text = text.trim_start();
    let (name, rest) = text.split_once(char::is_whitespace).unwrap_or((text, ""));
    (name == "/goal").then(|| rest.trim())
}

pub(super) async fn command(api: &Api, session: &str, text: &str, files: &[Value]) -> Result<()> {
    api.post(
        &format!("/api/session/{session}/command"),
        Some(&json!({ "name": "goal", "text": text, "files": files, "delivery": "steer" })),
    )
    .await
    .map(drop)
}

pub(super) async fn refresh(
    api: &Api,
    session: &str,
    decoder: &mut Decoder,
    io: &RunIo,
    await_change: bool,
) -> Result<()> {
    let value = api
        .post(
            "/api/rpc/opencode.goal/get",
            Some(&json!({ "input": { "sessionID": session } })),
        )
        .await?;
    let goal = serde_json::from_value::<Option<Goal>>(value["output"].clone())
        .context("decoding OpenCode goal")?;
    if await_change {
        decoder.await_goal_change(goal.clone());
    }
    emit_all(io, decoder.goal_changed(session, goal)).await;
    Ok(())
}

pub(super) async fn clear_goal(turn: Turn) -> Result<()> {
    let session = turn
        .session_id
        .as_deref()
        .context("clearing a goal needs an OpenCode session")?;
    let password = password()?;
    let config_dir = write_agents(&turn.home)?;
    let mut server = spawn(&turn, &password, &config_dir)?;
    let result = async {
        let base = tokio::time::timeout(READY_TIMEOUT, ready(&mut server)).await??;
        let api = Api::new(base, password)?;
        command(&api, session, "clear", &[]).await
    }
    .await;
    server.kill().await;
    server.wait().await;
    result
}
