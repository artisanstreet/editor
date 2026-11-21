use super::decoder::{Decoder, Goal};
use super::{Api, ApiError, ServerError, connect, emit_all, prepare};
use crate::{RunIo, Turn};
use artisan_api::ThreadId;
use serde_json::{Value, json};

#[derive(Debug, thiserror::Error)]
pub enum GoalError {
    #[error("send the goal command")]
    Command(#[source] ApiError),
    #[error("read the goal")]
    Read(#[source] ApiError),
    #[error("decode the goal")]
    Decode(#[source] serde_json::Error),
}

#[derive(Debug, thiserror::Error)]
pub enum ClearGoalError {
    #[error("thread {thread} has no OpenCode session")]
    NoSession { thread: ThreadId },
    #[error("start the OpenCode server")]
    Server(#[source] ServerError),
    #[error("clear the goal of OpenCode session {session}")]
    Clear {
        session: String,
        #[source]
        source: ApiError,
    },
}

pub(super) fn argument(text: &str) -> Option<&str> {
    let text = text.trim_start();
    let (name, rest) = text.split_once(char::is_whitespace).unwrap_or((text, ""));
    (name == "/goal").then(|| rest.trim())
}

pub(super) async fn command(
    api: &Api,
    session: &str,
    text: &str,
    files: &[Value],
) -> Result<(), ApiError> {
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
) -> Result<(), GoalError> {
    let mut value = api
        .post(
            "/api/rpc/opencode.goal/get",
            Some(&json!({ "input": { "sessionID": session } })),
        )
        .await
        .map_err(GoalError::Read)?;
    let goal = serde_json::from_value::<Option<Goal>>(value["output"].take())
        .map_err(GoalError::Decode)?;
    if await_change {
        decoder.await_goal_change(goal.clone());
    }
    emit_all(io, decoder.goal_changed(session, goal)).await;
    Ok(())
}

pub(super) async fn clear_goal(turn: Turn) -> Result<(), ClearGoalError> {
    let session = turn
        .session_id
        .clone()
        .ok_or_else(|| ClearGoalError::NoSession {
            thread: turn.thread_id.clone(),
        })?;
    let (mut server, password, _agents) = prepare(&turn).await.map_err(ClearGoalError::Server)?;
    let result = async {
        let api = connect(&mut server, password)
            .await
            .map_err(ClearGoalError::Server)?;
        command(&api, &session, "clear", &[])
            .await
            .map_err(|source| ClearGoalError::Clear {
                session: session.clone(),
                source,
            })
    }
    .await;
    server.kill().await;
    server.finish().await;
    result
}
