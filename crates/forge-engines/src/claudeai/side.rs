//! Summarize from the source's saved message UUID, keeping its model and tool
//! prefix. The summary is a sibling branch: Forge never advances the source's
//! continuation to it. Only missing sessions need serialized canonical history
//! in a fresh temporary conversation.
use super::*;
use crate::{SideReply, SideUsage};
use tokio_util::sync::CancellationToken;

pub(super) async fn compact(turn: Turn, cancel: CancellationToken) -> anyhow::Result<SideReply> {
    let backend = Transport::open(&turn.home)?;
    compact_with(&backend, &turn, cancel).await
}

struct SummaryContext {
    conversation: String,
    parent: Option<String>,
    tools: Vec<Value>,
    /// A fresh fallback conversation is ours to delete. Never delete the source.
    owned: bool,
}

async fn context(backend: &dyn Backend, turn: &Turn) -> anyhow::Result<SummaryContext> {
    if let Some(session) = &turn.session_id {
        let (conversation, parent) = session
            .strip_prefix(artisan_api::CLAUDE_AI_SESSION_PREFIX)
            .and_then(|session| session.split_once('/'))
            .filter(|(conversation, parent)| !conversation.is_empty() && !parent.is_empty())
            .context("the native source session has no saved message UUID; canonical history is required")?;
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
        conversation: backend.create_conversation().await?,
        parent: None,
        tools: Vec::new(),
        owned: true,
    })
}

pub(super) async fn compact_with(
    backend: &dyn Backend,
    turn: &Turn,
    cancel: CancellationToken,
) -> anyhow::Result<SideReply> {
    let context = tokio::select! {
        () = cancel.cancelled() => anyhow::bail!("compaction cancelled"),
        result = tokio::time::timeout(Duration::from_secs(30), context(backend, turn)) => result.context("preparing native compaction timed out")??,
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
        () = cancel.cancelled() => Err(anyhow::anyhow!("compaction cancelled")),
        () = tokio::time::sleep(Duration::from_secs(180)) => Err(anyhow::anyhow!("claude.ai compaction timed out")),
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
            Ok(Err(error)) => tracing::warn!("deleting native compaction fallback: {error:#}"),
            Err(_) => tracing::warn!("deleting native compaction fallback timed out"),
        }
    }
    stopped?;
    answer
}

async fn read_summary(
    backend: &dyn Backend,
    conversation: &str,
    request: &Request,
) -> anyhow::Result<SideReply> {
    let mut stream = backend.complete(conversation, request).await?;
    let mut decoder = ClaudeAiDecoder::default();
    let mut reply = SideReply::default();
    while let Some(event) = stream.next().await {
        let mut out = Vec::new();
        decoder.feed(&event?, &mut out);
        anyhow::ensure!(
            !decoder.waiting_on_caller() && !decoder.connector_started(),
            "the summary attempted to call a tool"
        );
        if let Some(error) = decoder.take_failure() {
            return Err(anyhow::Error::new(error));
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
                    anyhow::bail!("the summary attempted to call a tool");
                }
                EngineEvent::Usage(usage) => {
                    reply.usage = SideUsage {
                        input: usage.input + usage.cache_read + usage.cache_write.unwrap_or(0),
                        cached_input: usage.cache_read,
                        output: usage.output + usage.reasoning,
                    }
                }
                EngineEvent::Session { .. }
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
        if decoder.ended() {
            return Ok(reply);
        }
    }
    anyhow::bail!("claude.ai compaction stream ended without a summary")
}
