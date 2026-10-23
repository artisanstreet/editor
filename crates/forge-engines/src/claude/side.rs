//! Side queries: one question to a fork of a Claude session.
//!
//! `--resume <session> --fork-session` copies the session into a new one,
//! so the thread's own transcript is never touched. The fork launches
//! exactly as the thread does, so its request starts with the same prefix
//! and reads the thread's prompt cache: against Claude Code 2.1.296 a fork
//! in the thread's permission mode read all of it, while one in another
//! mode, or with a hook added through `--settings`, rewrote about 11k
//! tokens on every fork.
//!
//! So tools stay as they are, and none of them runs: the fork registers a
//! `PreToolUse` hook over the stream (`initialize`), which the CLI calls
//! before every tool in every permission mode and which always refuses.
//! A permission prompt (Auto mode) is refused too. `--max-turns 1` ends the
//! fork after one reply; a fork that reached for a tool answers nothing.

use super::{SIDE_QUERY_LIMIT, apply_env, config_dir, launch_args};
use crate::process;
use crate::{SideReply, SideUsage, Turn};
use anyhow::{Context, Result, bail};
use artisan_api::EngineConfig;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

pub(super) async fn side_query(turn: Turn, cancel: CancellationToken) -> Result<SideReply> {
    query(turn, cancel, SIDE_QUERY_LIMIT).await
}

pub(super) async fn compact(turn: Turn, cancel: CancellationToken) -> Result<SideReply> {
    query(turn, cancel, std::time::Duration::from_secs(300)).await
}

#[expect(
    clippy::too_many_lines,
    reason = "one disposable fork protocol, from handshake to cleanup"
)]
async fn query(
    mut turn: Turn,
    cancel: CancellationToken,
    deadline: std::time::Duration,
) -> Result<SideReply> {
    let session = turn.session_id.clone();
    anyhow::ensure!(
        !session
            .as_deref()
            .is_some_and(|id| id.starts_with(artisan_api::CLAUDE_AI_SESSION_PREFIX)),
        "a claude.ai session cannot be resumed by Claude Code"
    );
    apply_env(&mut turn);
    let mut engine = process::spawn(&turn, &side_args(&turn.config, session.as_deref()))
        .context("starting the Claude fork")?;
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
        .context("registering the Claude fork's tool hook")?;
    engine
        .send(&json!({
            "type": "user",
            "message": { "role": "user", "content": [{ "type": "text", "text": turn.input.text }] },
            "parent_tool_use_id": null,
            "session_id": "",
        }))
        .await
        .context("sending the side query to Claude")?;
    let limit = tokio::time::sleep(deadline);
    tokio::pin!(limit);
    let mut fork = None;
    let reply = loop {
        let line = tokio::select! {
            line = engine.next_line() => line.context("reading the Claude fork")?,
            () = cancel.cancelled() => {
                engine.kill().await;
                discard(&turn, fork.as_deref());
                bail!("the side query was cancelled");
            }
            () = &mut limit => {
                engine.kill().await;
                discard(&turn, fork.as_deref());
                bail!("the Claude fork gave no answer in {}s", deadline.as_secs());
            }
        };
        let Some(line) = line else {
            let code = engine.wait().await;
            discard(&turn, fork.as_deref());
            bail!(
                "the Claude fork exited ({}) without an answer: {}",
                code.map_or_else(|| "signal".to_owned(), |code| format!("code {code}")),
                engine.stderr_tail()
            );
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
                    .context("refusing a tool in the Claude fork")?;
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

/// The id of the hook that refuses every tool.
const REFUSE_TOOLS: &str = "artisan-side-refuse-tools";
/// What the fork is told when it reaches for a tool.
const NO_TOOLS: &str = "A side question cannot use tools; answer from the conversation.";

/// The thread's launch, unchanged, forked for one reply.
fn side_args(config: &EngineConfig, session: Option<&str>) -> Vec<String> {
    let mut args = launch_args(config, session);
    if session.is_some() {
        args.push("--fork-session".to_owned());
    }
    args.extend(["--max-turns", "1"].map(str::to_owned));
    args
}

fn reply_of(frame: &Value) -> Result<SideReply> {
    // Reaching for a tool used up the one turn: there is no answer.
    let reached_for_a_tool = frame["subtype"] == "error_max_turns";
    if frame["is_error"].as_bool() == Some(true) && !reached_for_a_tool {
        bail!(
            "the Claude fork failed: {}",
            frame["result"]
                .as_str()
                .or_else(|| frame["subtype"].as_str())
                .unwrap_or("no reason given")
        );
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

/// Deletes the fork's transcript: it is never resumed.
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

/// Claude's directory name for the project: the path with every character
/// that isn't a letter or digit replaced by `-`.
pub(super) fn project_dir(turn: &Turn) -> String {
    turn.cwd
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use artisan_api::Permissions;

    /// The fork reads the thread's cache only if it launches as the thread
    /// does, permission mode included.
    #[test]
    fn a_fork_launches_as_its_thread_does() {
        for permissions in [Permissions::Auto, Permissions::Full] {
            let mut config = EngineConfig::new(artisan_api::EngineId::Claude);
            config.permissions = permissions;
            config.model = Some("claude-opus-5-5".to_owned());
            let thread = launch_args(&config, Some("s1"));
            let fork = side_args(&config, Some("s1"));
            assert_eq!(fork[..thread.len()], thread[..]);
            assert_eq!(
                fork[thread.len()..],
                ["--fork-session", "--max-turns", "1"].map(str::to_owned)
            );
        }
    }
}
