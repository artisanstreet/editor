//! Summarize from the source's saved message UUID, keeping its model and tool
//! prefix. The summary is a sibling branch: Forge never advances the source's
//! continuation to it. Only missing sessions need serialized canonical history
//! in a fresh temporary conversation.
use super::*;
use crate::{SideReply, SideUsage};
use tokio_util::sync::CancellationToken;

#[derive(Debug, thiserror::Error)]
pub enum SideQueryError {
    #[error("opening claude.ai")]
    Open(#[source] OpenTransportError),
    #[error(
        "the claude.ai session {session} has no saved message to branch from; canonical history is required"
    )]
    NoSavedMessage { session: String },
    #[error("starting a claude.ai conversation for the side query")]
    Create(#[source] CreateConversationError),
    #[error("preparing the side query took over {seconds} s")]
    PrepareTimedOut { seconds: u64 },
    #[error("the side query was cancelled")]
    Cancelled,
    #[error("claude.ai did not answer the side query within {seconds} s")]
    TimedOut { seconds: u64 },
    #[error("asking claude.ai the side query")]
    Complete(#[source] CompleteError),
    #[error("reading claude.ai's answer to the side query")]
    Stream(#[source] StreamError),
    #[error("claude.ai failed the side query")]
    Refused(#[source] Refusal),
    #[error("claude.ai's answer to the side query could not be read")]
    Protocol(#[source] ProtocolError),
    #[error("the side query tried to call a tool")]
    CalledTool,
    #[error("claude.ai cut the side query's answer off at the model's output limit")]
    Truncated,
    #[error("Claude declined the side query")]
    Declined,
    #[error("claude.ai ended the side query's answer for an unknown reason ({reason})")]
    UnknownStop { reason: String },
    #[error("claude.ai's stream ended without an answer to the side query")]
    NoAnswer,
    #[error("releasing claude.ai's side query completion")]
    Stop(#[source] StopCompletionError),
}

const PREPARE_LIMIT: Duration = Duration::from_secs(30);
const ANSWER_LIMIT: Duration = Duration::from_secs(180);

pub(super) async fn compact(
    turn: Turn,
    cancel: CancellationToken,
) -> Result<SideReply, SideQueryError> {
    let backend = Transport::open(&turn.home).map_err(SideQueryError::Open)?;
    compact_with(&backend, &turn, cancel).await
}

struct SummaryContext {
    conversation: String,
    parent: Option<String>,
    tools: Vec<Value>,
    /// A fresh fallback conversation is ours to delete. Never delete the source.
    owned: bool,
}

async fn context(backend: &dyn Backend, turn: &Turn) -> Result<SummaryContext, SideQueryError> {
    if let Some(session) = &turn.session_id {
        let (conversation, parent) = session
            .strip_prefix(artisan_api::CLAUDE_AI_SESSION_PREFIX)
            .and_then(|session| session.split_once('/'))
            .filter(|(conversation, parent)| !conversation.is_empty() && !parent.is_empty())
            .ok_or_else(|| SideQueryError::NoSavedMessage {
                session: session.clone(),
            })?;
        let runtime = mcp::for_thread(
            turn.thread_id.as_str(),
            tools::Runtime::build(&turn.home, &turn.cwd, &turn.launch.mcp_servers),
        )
        .await;
        return Ok(SummaryContext {
            conversation: conversation.to_owned(),
            parent: Some(parent.to_owned()),
            tools: tools::definitions(&runtime),
            owned: false,
        });
    }
    Ok(SummaryContext {
        conversation: backend
            .create_conversation()
            .await
            .map_err(SideQueryError::Create)?,
        parent: None,
        tools: Vec::new(),
        owned: true,
    })
}

pub(super) async fn compact_with(
    backend: &dyn Backend,
    turn: &Turn,
    cancel: CancellationToken,
) -> Result<SideReply, SideQueryError> {
    let context = tokio::select! {
        () = cancel.cancelled() => return Err(SideQueryError::Cancelled),
        result = tokio::time::timeout(PREPARE_LIMIT, context(backend, turn)) => result.map_err(|_elapsed| SideQueryError::PrepareTimedOut { seconds: PREPARE_LIMIT.as_secs() })??,
    };
    let request = Request {
        prompt: turn.input.text.clone(),
        model: turn
            .config
            .model
            .clone()
            .filter(|model| !model.is_empty())
            .unwrap_or_else(|| "claude-opus-5-5".to_owned()),
        thinking_mode: "auto".to_owned(),
        // Match drive_turn's effective effort as well as its tool definitions.
        effort: turn
            .config
            .effort
            .clone()
            .filter(|effort| !effort.is_empty())
            .or_else(|| Some("high".to_owned())),
        tools: context.tools,
        tool_states: Vec::new(),
        files: Vec::new(),
        parent: context.parent,
    };
    let answer = tokio::select! {
        biased;
        () = cancel.cancelled() => Err(SideQueryError::Cancelled),
        () = tokio::time::sleep(ANSWER_LIMIT) => Err(SideQueryError::TimedOut { seconds: ANSWER_LIMIT.as_secs() }),
        answer = read_summary(backend, &context.conversation, &request) => answer,
    };
    // end_turn may precede message_stop; release the server completion before
    // the caller can resume its original branch, including on cancellation.
    let stopped = stop(backend, &context.conversation).await;
    if context.owned {
        match tokio::time::timeout(
            Duration::from_secs(5),
            backend.delete_conversation(&context.conversation),
        )
        .await
        {
            Ok(Ok(())) => {}
            Ok(Err(error)) => tracing::warn!(
                "deleting native compaction fallback: {}",
                diagnostics::chain(&error)
            ),
            Err(_) => tracing::warn!("deleting native compaction fallback timed out"),
        }
    }
    stopped.map_err(SideQueryError::Stop)?;
    answer
}

async fn read_summary(
    backend: &dyn Backend,
    conversation: &str,
    request: &Request,
) -> Result<SideReply, SideQueryError> {
    let mut stream = backend
        .complete(conversation, request)
        .await
        .map_err(SideQueryError::Complete)?;
    let mut decoder = ClaudeAiDecoder::default();
    let mut reply = SideReply::default();
    while let Some(event) = stream.next().await {
        let mut out = Vec::new();
        decoder.feed(&event.map_err(SideQueryError::Stream)?, &mut out);
        if decoder.waiting_on_caller() || decoder.connector_started() {
            return Err(SideQueryError::CalledTool);
        }
        if let Some(error) = decoder.take_failure() {
            return Err(SideQueryError::Refused(error));
        }
        if let Some(error) = decoder.take_protocol_error() {
            return Err(SideQueryError::Protocol(error));
        }
        for event in out {
            match event {
                EngineEvent::Item {
                    status: Status::Done,
                    kind:
                        ItemKind::AssistantMessage {
                            text,
                            phase: artisan_api::Phase::Final,
                        },
                    ..
                } => {
                    reply.text.push_str(text.inline().unwrap_or_default());
                }
                EngineEvent::Item {
                    kind: ItemKind::Tool { .. } | ItemKind::Agent { .. } | ItemKind::Question { .. },
                    ..
                } => {
                    return Err(SideQueryError::CalledTool);
                }
                EngineEvent::Usage(usage) => {
                    reply.usage = SideUsage {
                        input: usage.input + usage.cache_read + usage.cache_write.unwrap_or(0),
                        cached_input: usage.cache_read,
                        output: usage.output + usage.reasoning,
                    }
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
        match decoder.stop() {
            Some(Stop::Completed) => return Ok(reply),
            Some(Stop::Truncated) => return Err(SideQueryError::Truncated),
            Some(Stop::Refused) => return Err(SideQueryError::Declined),
            Some(Stop::Unsupported(reason)) => {
                return Err(SideQueryError::UnknownStop {
                    reason: reason.clone(),
                });
            }
            Some(Stop::ToolUse) | None => {}
        }
    }
    Err(SideQueryError::NoAnswer)
}
