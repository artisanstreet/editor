use super::{SIDE_QUERY_LIMIT, apply_env, config_dir, launch_args};
use crate::process::{self, ReadError, SendError, SpawnError};
use crate::{SideReply, SideUsage, Turn};
use artisan_api::EngineConfig;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

#[derive(Debug, thiserror::Error)]
pub enum SideQueryError {
    #[error("Claude Code can't resume claude.ai session {session}")]
    ClaudeAiSession { session: String },
    #[error("start the Claude fork")]
    Spawn(#[source] SpawnError),
    #[error("{step} in the Claude fork")]
    Send {
        step: &'static str,
        #[source]
        source: SendError,
    },
    #[error("read the Claude fork")]
    Read(#[source] ReadError),
    #[error("the side query was cancelled")]
    Cancelled,
    #[error("the Claude fork gave no answer in {}s", after.as_secs())]
    TimedOut { after: Duration },
    #[error("the Claude fork exited ({}) without an answer: {stderr}", code.map_or_else(|| "signal".to_owned(), |code| format!("code {code}")))]
    Exited { code: Option<i32>, stderr: String },
    #[error("the Claude fork failed: {reason}")]
    Failed { reason: String },
}

pub(super) async fn side_query(
    turn: Turn,
    cancel: CancellationToken,
) -> Result<SideReply, SideQueryError> {
    query(turn, cancel, SIDE_QUERY_LIMIT).await
}

pub(super) async fn compact(
    turn: Turn,
    cancel: CancellationToken,
) -> Result<SideReply, SideQueryError> {
    query(turn, cancel, Duration::from_secs(300)).await
}

#[expect(
    clippy::too_many_lines,
    reason = "one disposable fork protocol, from handshake to cleanup"
)]
async fn query(
    mut turn: Turn,
    cancel: CancellationToken,
    deadline: Duration,
) -> Result<SideReply, SideQueryError> {
    let session = turn.session_id.clone();
    if let Some(session) = session
        .as_deref()
        .filter(|id| id.starts_with(artisan_api::CLAUDE_AI_SESSION_PREFIX))
    {
        return Err(SideQueryError::ClaudeAiSession {
            session: session.to_owned(),
        });
    }
    apply_env(&mut turn);
    let mut engine = process::spawn(
        &turn,
        &side_args(&turn.config, session.as_deref(), &turn.launch.readable),
    )
    .map_err(SideQueryError::Spawn)?;
    let send_error = |step: &'static str| move |source| SideQueryError::Send { step, source };
    engine
        .send(&json!({
            "type": "control_request",
            "request_id": "artisan-side-init",
            "request": {
                "subtype": "initialize",
                "hooks": { "PreToolUse": [{ "matcher": "*", "hookCallbackIds": [REFUSE_TOOLS] }] },
            },
        }))
        .await
        .map_err(send_error("register the tool hook"))?;
    engine
        .send(&json!({
            "type": "user",
            "message": { "role": "user", "content": [{ "type": "text", "text": turn.input.text }] },
            "parent_tool_use_id": null,
            "session_id": "",
        }))
        .await
        .map_err(send_error("send the side query"))?;
    let limit = tokio::time::sleep(deadline);
    tokio::pin!(limit);
    let mut fork = None;
    let reply = loop {
        let line = tokio::select! {
            line = engine.next_line() => line.map_err(SideQueryError::Read)?,
            () = cancel.cancelled() => {
                engine.kill().await;
                discard(&turn, fork.as_deref());
                return Err(SideQueryError::Cancelled);
            }
            () = &mut limit => {
                engine.kill().await;
                discard(&turn, fork.as_deref());
                return Err(SideQueryError::TimedOut { after: deadline });
            }
        };
        let Some(line) = line else {
            let code = engine.wait().await;
            discard(&turn, fork.as_deref());
            return Err(SideQueryError::Exited {
                code,
                stderr: engine.stderr_tail(),
            });
        };
        let Ok(frame) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if let Some(id) = frame["session_id"]
            .as_str()
            .filter(|id| Some(*id) != session.as_deref())
        {
            fork.get_or_insert_with(|| id.to_owned());
        }
        match frame["type"].as_str() {
            Some("control_request") => {
                let refusal = match frame["request"]["subtype"].as_str() {
                    Some("hook_callback") => json!({
                        "decision": "block",
                        "reason": NO_TOOLS,
                        "hookSpecificOutput": {
                            "hookEventName": "PreToolUse",
                            "permissionDecision": "deny",
                            "permissionDecisionReason": NO_TOOLS,
                        },
                    }),
                    Some("can_use_tool") => {
                        json!({ "behavior": "deny", "message": NO_TOOLS, "interrupt": false })
                    }
                    _ => json!({}),
                };
                engine
                    .send(&json!({
                        "type": "control_response",
                        "response": {
                            "subtype": "success",
                            "request_id": frame["request_id"],
                            "response": refusal,
                        },
                    }))
                    .await
                    .map_err(send_error("refuse a tool"))?;
            }
            Some("result") => break reply_of(&frame),
            _ => {}
        }
    };
    engine.close_stdin("the side query has its answer");
    engine.wait().await;
    discard(&turn, fork.as_deref());
    reply
}

const REFUSE_TOOLS: &str = "artisan-side-refuse-tools";
const NO_TOOLS: &str = "A side question cannot use tools; answer from the conversation.";

fn side_args(config: &EngineConfig, session: Option<&str>, readable: &[PathBuf]) -> Vec<String> {
    let mut args = launch_args(config, session, readable);
    if session.is_some() {
        args.push("--fork-session".to_owned());
    }
    args.extend(["--max-turns", "1"].map(str::to_owned));
    args
}

fn reply_of(frame: &Value) -> Result<SideReply, SideQueryError> {
    let reached_for_a_tool = frame["subtype"] == "error_max_turns";
    if frame["is_error"].as_bool() == Some(true) && !reached_for_a_tool {
        return Err(SideQueryError::Failed {
            reason: frame["result"]
                .as_str()
                .or_else(|| frame["subtype"].as_str())
                .unwrap_or("no reason given")
                .to_owned(),
        });
    }
    let usage = &frame["usage"];
    let tokens = |key: &str| usage[key].as_u64().unwrap_or(0);
    let cached = tokens("cache_read_input_tokens");
    Ok(SideReply {
        text: if reached_for_a_tool {
            String::new()
        } else {
            frame["result"]
                .as_str()
                .unwrap_or_default()
                .trim()
                .to_owned()
        },
        usage: SideUsage {
            input: tokens("input_tokens") + tokens("cache_creation_input_tokens") + cached,
            cached_input: cached,
            output: tokens("output_tokens"),
        },
    })
}

fn discard(turn: &Turn, fork: Option<&str>) {
    let Some(fork) = fork else {
        return;
    };
    let directory = config_dir(turn).join("projects").join(project_dir(turn));
    let transcript = directory.join(format!("{fork}.jsonl"));
    if let Err(error) = std::fs::remove_file(&transcript)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        tracing::debug!(path = %transcript.display(), "removing a Claude fork's transcript: {error}");
    }
    let attachments = directory.join(fork);
    if let Err(error) = std::fs::remove_dir_all(&attachments)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        tracing::debug!(path = %attachments.display(), "removing a Claude fork's directory: {error}");
    }
}

pub(super) fn project_dir(turn: &Turn) -> String {
    turn.cwd
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

#[cfg(test)]
#[path = "../../tests/claude/side.rs"]
mod tests;
