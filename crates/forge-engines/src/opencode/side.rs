use super::decoder::{TurnFailure, exceeds_context};
use super::*;
use crate::{SideReply, SideUsage};
use artisan_api::{ItemKind, Phase, Status};
use tokio_util::sync::CancellationToken;

const LIMIT: Duration = Duration::from_secs(180);

#[derive(Debug, thiserror::Error)]
pub enum SideQueryError {
    #[error("the side query was cancelled")]
    Cancelled,
    #[error("OpenCode gave no answer in {}s", LIMIT.as_secs())]
    TimedOut,
    #[error("start the OpenCode server")]
    Server(#[source] ServerError),
    #[error("open OpenCode's event stream")]
    Subscribe(#[source] SubscribeError),
    #[error("{step}")]
    Api {
        step: &'static str,
        #[source]
        source: ApiError,
    },
    #[error("OpenCode's fork has no id")]
    NoForkId,
    #[error("send the question to fork {fork}")]
    Prompt {
        fork: String,
        #[source]
        source: InputError,
    },
    #[error("OpenCode's event stream closed during the side query")]
    StreamClosed,
    #[error("decode an OpenCode event")]
    Decode(#[source] DecodeError),
    #[error("decode an OpenCode compaction event")]
    CompactionEvent(#[source] serde_json::Error),
    #[error("OpenCode compaction failed: {error}")]
    Failed { error: String },
    #[error("OpenCode fork {fork} is over the model's context limit: {error}")]
    ContextLimit { fork: String, error: String },
    #[error("OpenCode compaction was interrupted")]
    Interrupted,
}

impl SideQueryError {
    #[must_use]
    pub const fn exceeds_context(&self) -> bool {
        match self {
            Self::ContextLimit { .. } => true,
            Self::Cancelled
            | Self::TimedOut
            | Self::Server(_)
            | Self::Subscribe(_)
            | Self::Api { .. }
            | Self::NoForkId
            | Self::Prompt { .. }
            | Self::StreamClosed
            | Self::Decode(_)
            | Self::CompactionEvent(_)
            | Self::Failed { .. }
            | Self::Interrupted => false,
        }
    }
}

pub(super) async fn compact(
    turn: Turn,
    cancel: CancellationToken,
) -> Result<SideReply, SideQueryError> {
    query(turn, cancel, true).await
}

pub(super) async fn side_query(
    turn: Turn,
    cancel: CancellationToken,
) -> Result<SideReply, SideQueryError> {
    query(turn, cancel, false).await
}

async fn query(
    turn: Turn,
    cancel: CancellationToken,
    compact: bool,
) -> Result<SideReply, SideQueryError> {
    let (mut server, password) = prepare(&turn).map_err(SideQueryError::Server)?;
    let mut api = None;
    let mut fork = None;
    let call = |step: &'static str| move |source| SideQueryError::Api { step, source };
    let answer = tokio::select! {
        biased;
        () = cancel.cancelled() => Err(SideQueryError::Cancelled),
        () = tokio::time::sleep(LIMIT) => Err(SideQueryError::TimedOut),
        answer = async {
            let connected = connect(&mut server, password).await.map_err(SideQueryError::Server)?;
            let api = api.insert(connected);
            let mut events = subscribe(api).await.map_err(SideQueryError::Subscribe)?;
            let native = compact && turn.session_id.is_some();
            let created = if let Some(session) = &turn.session_id {
                api.post(&format!("/api/session/{session}/fork"), Some(&json!({}))).await.map_err(call("fork the session"))?
            } else {
                api.post("/api/session", Some(&json!({
                    "agent": "artisan-summary",
                    "location": { "directory": turn.cwd },
                }))).await.map_err(call("create a session"))?
            };
            let session = created["data"]["id"].as_str().ok_or(SideQueryError::NoForkId)?.to_owned();
            fork = Some(session.clone());
            if !native && turn.session_id.is_some() {
                api.post(&format!("/api/session/{session}/agent"), Some(&json!({ "agent": "artisan-summary" }))).await.map_err(call("switch the fork's agent"))?;
            }
            if let Some(model) = model_ref(turn.config.model.as_deref(), turn.config.variant.as_deref()) {
                api.post(&format!("/api/session/{session}/model"), Some(&json!({ "model": model }))).await.map_err(call("switch the fork's model"))?;
            }
            let mut decoder = Decoder::new(Some(session.clone()));
            if native {
                api.post(&format!("/api/session/{session}/compact"), Some(&json!({}))).await.map_err(call("compact the fork"))?;
            } else {
                let message = send_prompt(api, &session, &turn.input).await.map_err(|source| SideQueryError::Prompt { fork: session.clone(), source })?;
                decoder.prompt_sent(&message, None);
            }
            let mut reply = SideReply::default();
            loop {
                let data = events.recv().await.ok_or(SideQueryError::StreamClosed)?;
                if native {
                    if let Some(result) = compact_event(&data, &session)? { return Ok(result); }
                } else {
                    for event in decoder.feed(&data).map_err(SideQueryError::Decode)? {
                        match event {
                            EngineEvent::Item { status: Status::Done, kind: ItemKind::AssistantMessage { text, phase: Phase::Final }, .. } => {
                                text.inline().unwrap_or_default().clone_into(&mut reply.text);
                            }
                            EngineEvent::Usage(usage) => {
                                reply.usage = SideUsage { input: usage.input + usage.cache_read + usage.cache_write.unwrap_or(0), cached_input: usage.cache_read, output: usage.output + usage.reasoning };
                            }
                            EngineEvent::Session { .. }
                        | EngineEvent::SessionGone
                        | EngineEvent::Model { .. }
                        | EngineEvent::Title { .. }
                        | EngineEvent::Item { .. }
                        | EngineEvent::Image { .. }
                        | EngineEvent::Status { .. }
                        | EngineEvent::Live { .. }
                        | EngineEvent::Commands(_)
                        | EngineEvent::InputAccepted { .. } => {}
                        }
                    }
                    if decoder.idle() {
                        match decoder.error.take() {
                            Some(TurnFailure::ContextLimit(error)) => return Err(SideQueryError::ContextLimit { fork: session, error }),
                            Some(TurnFailure::Failed(error)) => return Err(SideQueryError::Failed { error }),
                            None => {}
                        }
                        return Ok(reply);
                    }
                }
            }
        } => answer,
    };
    if let (Some(api), Some(session)) = (api, fork) {
        let cleaned = tokio::time::timeout(Duration::from_secs(5), async {
            if let Err(failure) = api
                .post(&format!("/api/session/{session}/interrupt"), None)
                .await
            {
                let error: &dyn std::error::Error = &failure;
                tracing::debug!(fork = session, error, "couldn't interrupt an OpenCode fork");
            }
            api.delete(&format!("/api/session/{session}")).await
        })
        .await;
        match cleaned {
            Ok(Ok(_)) => {}
            Ok(Err(failure)) => {
                let error: &dyn std::error::Error = &failure;
                tracing::debug!(fork = session, error, "couldn't delete an OpenCode fork");
            }
            Err(_elapsed) => tracing::debug!(fork = session, "deleting an OpenCode fork timed out"),
        }
    }
    server.kill().await;
    server.wait().await;
    answer
}

fn compact_event(data: &str, session: &str) -> Result<Option<SideReply>, SideQueryError> {
    let event: Value = serde_json::from_str(data).map_err(SideQueryError::CompactionEvent)?;
    let body = &event["data"];
    if body["sessionID"] != session {
        return Ok(None);
    }
    match event["type"].as_str() {
        Some("session.compaction.ended") => {
            let tokens = &body["tokens"];
            let count = |name: &str| tokens[name].as_u64().unwrap_or(0);
            let cached = tokens["cache"]["read"].as_u64().unwrap_or(0);
            let written = tokens["cache"]["write"].as_u64().unwrap_or(0);
            Ok(Some(SideReply {
                text: body["text"].as_str().unwrap_or_default().to_owned(),
                usage: SideUsage {
                    input: count("input") + cached + written,
                    cached_input: cached,
                    output: count("output") + count("reasoning"),
                },
            }))
        }
        Some("session.compaction.failed" | "session.execution.failed") => {
            let error = &body["error"];
            Err(if exceeds_context(error) {
                SideQueryError::ContextLimit {
                    fork: session.to_owned(),
                    error: error.to_string(),
                }
            } else {
                SideQueryError::Failed {
                    error: error.to_string(),
                }
            })
        }
        Some("session.execution.interrupted") => Err(SideQueryError::Interrupted),
        _ => Ok(None),
    }
}

#[cfg(test)]
#[path = "../../tests/opencode/side.rs"]
mod tests;

#[cfg(test)]
#[path = "../../tests/opencode/side/integration.rs"]
mod integration_tests;
