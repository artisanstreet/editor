//! Claude Code over `claude -p` stream-json stdio.
//!
//! One process per engine turn. The user message goes in as a stream-json
//! `user` line; steers are more `user` lines (the CLI folds them into the
//! running turn, or runs them as the next turn in the same process). With
//! `--replay-user-messages` the CLI echoes each user line when it takes it,
//! which is when a steer counts as accepted. Permission prompts arrive as
//! `control_request can_use_tool` on stdout and are answered with
//! `control_response` on stdin.
//!
//! Verified against Claude Code 2.1.296: `control_request interrupt` stops
//! the turn (the `result` then has `terminal_reason: aborted_*`);
//! `generate_session_title` names a fresh session (and persists an
//! `ai-title` record — the CLI no longer writes one by itself in `-p` mode);
//! a background `Agent` keeps the process alive after `result` and the CLI
//! runs one more turn when it reports back.

mod decoder;
mod side;
pub mod tools;

pub use decoder::ClaudeDecoder;

use crate::process::{self, EngineProcess};
use crate::{EngineEvent, Harness, Input, Outcome, RunIo, SideReply, Turn, UserInput};
use anyhow::{Context, Result};
use artisan_api::{EngineConfig, Permissions};
use base64::Engine as _;
use futures::future::BoxFuture;
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::io::Write as _;
use std::path::Path;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// How long an interrupted turn may take to wind down before it is killed.
const INTERRUPT_GRACE: Duration = Duration::from_secs(5);
/// How long input stays open after the last background agent ends with no
/// follow-up turn. The ending agent queues one whose frames arrive well
/// inside this; it only bounds the case where none comes.
#[cfg(not(test))]
const SETTLED_INPUT_GRACE: Duration = Duration::from_secs(30);
#[cfg(test)]
const SETTLED_INPUT_GRACE: Duration = Duration::from_millis(300);
/// How long a side query may take before its fork is killed.
const SIDE_QUERY_LIMIT: Duration = Duration::from_secs(120);

pub struct ClaudeHarness;

impl Harness for ClaudeHarness {
    fn set_title(&self, turn: Turn, title: String) -> BoxFuture<'static, Result<()>> {
        Box::pin(async move {
            let session = turn
                .session_id
                .as_deref()
                .context("naming Claude needs a session")?;
            if let Some(native) = session.strip_prefix(artisan_api::CLAUDE_AI_SESSION_PREFIX) {
                let conversation = native.split('/').next().context("no native conversation")?;
                return crate::claudeai::Transport::open(&turn.home)?
                    .rename_conversation(conversation, &title)
                    .await;
            }
            // The CLI's native generated-title record. Appending preserves
            // the active transcript and makes the name survive resume.
            let path = config_dir(&turn)
                .join("projects")
                .join(side::project_dir(&turn))
                .join(format!("{session}.jsonl"));
            let mut record = serde_json::to_vec(&json!({
                "type": "ai-title", "aiTitle": title, "sessionId": session,
            }))?;
            record.push(b'\n');
            let mut file = std::fs::OpenOptions::new().append(true).open(path)?;
            file.write_all(&record)?;
            Ok(())
        })
    }

    fn compact(
        &self,
        turn: Turn,
        cancel: tokio_util::sync::CancellationToken,
    ) -> BoxFuture<'static, Result<SideReply>> {
        // Route by the source session when present: switching transports must
        // summarize the session being left, not try to resume it elsewhere.
        let native = turn.session_id.as_deref().map_or_else(
            || {
                turn.config
                    .options
                    .get("transport")
                    .and_then(serde_json::Value::as_str)
                    == Some(CLAUDE_AI_TRANSPORT)
            },
            |id| id.starts_with(artisan_api::CLAUDE_AI_SESSION_PREFIX),
        );
        if native {
            Box::pin(crate::claudeai::compact(turn, cancel))
        } else {
            Box::pin(side::compact(turn, cancel))
        }
    }

    fn run(&self, turn: Turn, io: RunIo) -> BoxFuture<'static, Outcome> {
        Box::pin(run(turn, io))
    }

    fn side_query(
        &self,
        turn: Turn,
        cancel: CancellationToken,
    ) -> BoxFuture<'static, Result<SideReply>> {
        // A session made on claude.ai is asked there: Claude Code cannot
        // resume it.
        if turn.session_id.as_deref().map_or_else(
            || {
                turn.config.options.get("transport").and_then(Value::as_str)
                    == Some(CLAUDE_AI_TRANSPORT)
            },
            |id| id.starts_with(artisan_api::CLAUDE_AI_SESSION_PREFIX),
        ) {
            Box::pin(crate::claudeai::side_query(turn, cancel))
        } else {
            Box::pin(side::side_query(turn, cancel))
        }
    }
}

/// The run's environment: what the thread's settings ask of the CLI.
fn apply_env(turn: &mut Turn) {
    if let Some((key, value)) = context_window_env(&turn.config) {
        turn.launch.env.retain(|(existing, _)| existing != key);
        turn.launch.env.push((key.to_owned(), value));
    }
}

/// Runs the turn, on whichever transport Forge chose for it.
///
/// The claude.ai transport (claude.ai mode) is the same engine
/// pointed at Anthropic's chat backend, which is the only one that returns
/// the provider's one-line recap of the model's thinking. Forge picks it
/// when the mode is on and a session is kept; when the mode is on but
/// nothing is signed in, the turn runs on Claude Code and says so.
async fn run(turn: Turn, io: RunIo) -> Outcome {
    match turn.config.options.get("transport").and_then(Value::as_str) {
        Some(CLAUDE_AI_TRANSPORT) => return Box::pin(crate::claudeai::run(turn, io)).await,
        Some(CLAUDE_AI_UNAVAILABLE) => {
            io.emit(EngineEvent::Item {
                key: "claude-ai-unavailable".to_owned(),
                parent: None,
                status: artisan_api::Status::Done,
                kind: artisan_api::ItemKind::Notice {
                    kind: artisan_api::NoticeKind::Info,
                    text: "claude.ai mode is on, but claude.ai isn't signed in, so this turn runs on Claude Code. Sign in under Settings → Claude Code → claude.ai mode.".to_owned().into(),
                },
            })
            .await;
        }
        _ => {}
    }
    run_claude_code(turn, io).await
}

/// The `transport` option that runs a turn on claude.ai.
pub const CLAUDE_AI_TRANSPORT: &str = "claude.ai";
/// The `transport` option for a turn that asked for claude.ai without a
/// session: it runs on Claude Code and says why.
pub const CLAUDE_AI_UNAVAILABLE: &str = "claude.ai-unavailable";

/// Runs the turn on Claude Code.
async fn run_claude_code(mut turn: Turn, mut io: RunIo) -> Outcome {
    apply_env(&mut turn);
    let mut decoder = ClaudeDecoder::default();
    let mut session = turn.session_id.clone();
    let fresh = session.is_none();
    // Messages for the next process: the turn's own message, then steers
    // that arrived after the previous process stopped taking input.
    let mut next: VecDeque<(UserInput, bool)> = VecDeque::from([(turn.input.clone(), false)]);
    let mut inbox_open = true;
    let mut outcome = None;

    while !next.is_empty() && !io.cancel.is_cancelled() {
        let args = launch_args(&turn.config, session.as_deref());
        let mut engine = match process::spawn(&turn, &args) {
            Ok(engine) => engine,
            Err(error) => {
                tracing::error!(thread = %turn.thread_id, "starting Claude: {error:#}");
                return Outcome::failed(format!("Claude could not start: {error:#}"));
            }
        };
        let mut delivered = 0;
        while let Some((input, steer)) = next.pop_front() {
            if let Err(error) = deliver(&mut engine, &mut decoder, &input, steer).await {
                tracing::error!(thread = %turn.thread_id, item = %input.id, "sending a message to Claude: {error:#}");
                return Outcome::failed(format!("Claude did not take the message: {error:#}"));
            }
            delivered += 1;
        }
        if fresh && session.is_none() && delivered > 0 {
            request_title(&mut engine, &mut decoder, &turn.input.text).await;
        }

        let finished = drive(
            &turn.thread_id,
            &mut engine,
            &mut decoder,
            &mut io,
            &mut next,
            &mut inbox_open,
        )
        .await;
        let finished = match finished {
            Ok(finished) => finished,
            Err(error) => {
                tracing::warn!(thread = %turn.thread_id, session = ?decoder.session(), "Claude protocol exchange failed: {error:#}");
                engine.kill().await;
                for event in decoder.finish() {
                    io.emit(event).await;
                }
                return Outcome::failed(format!("Claude protocol exchange failed: {error:#}"));
            }
        };
        let code = engine.wait().await;
        for event in decoder.finish() {
            io.emit(event).await;
        }
        if let Some(id) = decoder.session() {
            session = Some(id.to_owned());
        }
        if finished.cancelled {
            return Outcome::cancelled();
        }
        if let Some(result) = decoder.take_outcome() {
            outcome = Some(result);
        } else {
            let stderr = engine.stderr_tail();
            tracing::warn!(thread = %turn.thread_id, session = ?decoder.session(), ?code, "Claude exited without a result: {stderr}");
            return Outcome::failed(format!(
                "Claude exited ({}) before finishing{}",
                code.map_or_else(|| "signal".to_owned(), |code| format!("code {code}")),
                if stderr.is_empty() {
                    String::new()
                } else {
                    format!(": {stderr}")
                }
            ));
        }
        if outcome
            .as_ref()
            .is_some_and(|outcome: &Outcome| outcome.status != artisan_api::RunStatus::Done)
        {
            break;
        }
    }

    if fresh
        && !decoder.titled()
        && let Some(title) = session
            .as_deref()
            .and_then(|id| transcript_title(&turn, id))
    {
        io.emit(EngineEvent::Title { title }).await;
    }
    if io.cancel.is_cancelled() {
        return Outcome::cancelled();
    }
    outcome.unwrap_or_else(|| Outcome::failed("Claude produced no result"))
}

struct Finished {
    cancelled: bool,
}

/// Pumps one process until its stdout closes.
#[expect(
    clippy::too_many_lines,
    reason = "one select loop; each arm is one event source"
)]
async fn drive(
    thread: &artisan_api::ThreadId,
    engine: &mut EngineProcess,
    decoder: &mut ClaudeDecoder,
    io: &mut RunIo,
    next: &mut VecDeque<(UserInput, bool)>,
    inbox_open: &mut bool,
) -> Result<Finished> {
    let mut cancelling = false;
    let grace = tokio::time::sleep(Duration::from_secs(u64::from(u32::MAX)));
    tokio::pin!(grace);
    // Background agents outlived a `result`; input stays open for them.
    let mut waiting_on_background = false;
    let settle = tokio::time::sleep(Duration::from_secs(u64::from(u32::MAX)));
    tokio::pin!(settle);
    let mut settling = false;
    // A `result` left input open for a title request or steer still on its
    // way; once it is answered with no turn following, input closes.
    let mut held_for_input = false;
    loop {
        tokio::select! {
            line = engine.next_line() => {
                let line = match line {
                    Ok(Some(line)) => line,
                    Ok(None) => break,
                    Err(error) => {
                        return Err(error.context("reading Claude output"));
                    }
                };
                for event in decoder.feed(&line)? {
                    io.emit(event).await;
                }
                for reply in decoder.take_replies() {
                    if let Err(error) = engine.send(&reply).await {
                        tracing::warn!(%thread, session = ?decoder.session(), "answering Claude: {error:#}");
                    }
                }
                // Closing input ends the session: the CLI then waits only a
                // bounded while (`CLAUDE_CODE_PRINT_BG_WAIT_CEILING_MS`,
                // about 10 minutes) for background agents, kills them
                // mid-work and refuses steers meanwhile. So input closes at
                // a `result` only once no steer, title request or
                // background agent is still on its way.
                if decoder.take_result_seen() && engine.stdin_open() {
                    waiting_on_background |= after_result(engine, decoder, cancelling);
                    held_for_input = engine.stdin_open() && !decoder.background_running();
                } else if held_for_input
                    && engine.stdin_open()
                    && !decoder.in_turn()
                    && !decoder.awaiting_input()
                    && !decoder.background_running()
                {
                    // The title answer (or a steer's echo) came after the
                    // `result`: nothing else would close input, and the CLI
                    // would wait on it forever.
                    held_for_input = false;
                    engine.close_stdin("what held input open after the result was answered");
                }
                // The last agent ended: its report starts a follow-up turn
                // whose `result` closes input; failing that, the grace does.
                // The grace never closes mid-turn: that turn may still start
                // an agent, which the CLI would kill once input is closed.
                if waiting_on_background && !decoder.background_running() && !settling {
                    engine.note(&format!(
                        "background work ended; input closes in {}s unless more starts ({})",
                        SETTLED_INPUT_GRACE.as_secs(),
                        decoder.background_summary()
                    ));
                    settling = true;
                    settle.as_mut().reset(tokio::time::Instant::now() + SETTLED_INPUT_GRACE);
                } else if decoder.background_running() && settling {
                    engine.note("background work started again; input stays open");
                    settling = false;
                }
            }
            () = &mut settle, if settling => {
                settling = false;
                if engine.stdin_open()
                    && !decoder.in_turn()
                    && !decoder.awaiting_input()
                    && !decoder.background_running()
                {
                    engine.close_stdin(&format!(
                        "no background work for {}s ({})",
                        SETTLED_INPUT_GRACE.as_secs(),
                        decoder.background_summary()
                    ));
                }
            }
            input = io.inbox.recv(), if *inbox_open && !cancelling => match input {
                Some(Input::Steer(steer)) => {
                    let delivered = engine.stdin_open()
                        && deliver(engine, decoder, &steer, true).await.is_ok();
                    if !delivered {
                        next.push_back((steer, true));
                    }
                }
                Some(Input::Answer { key, answer }) => {
                    for event in decoder.answer(&key, &answer) {
                        io.emit(event).await;
                    }
                    for reply in decoder.take_replies() {
                        if let Err(error) = engine.send(&reply).await {
                            tracing::warn!(%thread, session = ?decoder.session(), key, "answering Claude: {error:#}");
                        }
                    }
                }
                // Forge clears a Claude goal with `/goal clear` as a message.
                Some(Input::ClearGoal) => {}
                Some(Input::SetPermissions(permissions)) => {
                    let mode = match permissions {
                        Permissions::Auto => "auto",
                        Permissions::Full => "bypassPermissions",
                    };
                    let request = json!({
                        "type": "control_request",
                        "request_id": format!("artisan-mode-{mode}"),
                        "request": { "subtype": "set_permission_mode", "mode": mode },
                    });
                    if let Err(error) = engine.send(&request).await {
                        tracing::warn!(%thread, session = ?decoder.session(), mode, "switching Claude's permission mode: {error:#}");
                    }
                }
                None => *inbox_open = false,
            },
            () = io.cancel.cancelled(), if !cancelling => {
                engine.note("the run was cancelled: interrupting");
                cancelling = true;
                let interrupt = json!({
                    "type": "control_request",
                    "request_id": "artisan-interrupt",
                    "request": { "subtype": "interrupt" },
                });
                if engine.send(&interrupt).await.is_err() {
                    engine.kill().await;
                }
                grace.as_mut().reset(tokio::time::Instant::now() + INTERRUPT_GRACE);
            }
            () = &mut grace, if cancelling => {
                tracing::warn!(%thread, session = ?decoder.session(), "Claude did not stop after an interrupt; killing it");
                engine.kill().await;
                grace.as_mut().reset(tokio::time::Instant::now() + Duration::from_secs(3600));
            }
        }
    }
    Ok(Finished {
        cancelled: cancelling,
    })
}

/// A `result` arrived: closes input unless a steer, a title request or
/// background work still needs it. Returns whether it stays open for
/// background work.
fn after_result(engine: &mut EngineProcess, decoder: &ClaudeDecoder, cancelling: bool) -> bool {
    if cancelling || (!decoder.awaiting_input() && !decoder.background_running()) {
        engine.close_stdin(&format!(
            "result (cancelling: {cancelling}; {})",
            decoder.background_summary()
        ));
        false
    } else if decoder.background_running() {
        engine.note(&format!(
            "result; input stays open for background work ({})",
            decoder.background_summary()
        ));
        true
    } else {
        engine.note("result; input stays open: a steer or title request is on its way");
        false
    }
}

async fn deliver(
    engine: &mut EngineProcess,
    decoder: &mut ClaudeDecoder,
    input: &UserInput,
    _steer: bool,
) -> Result<()> {
    let line = user_line(input).await?;
    engine.send(&line).await?;
    decoder.expect_replay(Some(input.id.clone()));
    Ok(())
}

async fn request_title(engine: &mut EngineProcess, decoder: &mut ClaudeDecoder, text: &str) {
    if text.trim().is_empty() {
        return;
    }
    let request = decoder.title_request(text);
    if let Err(error) = engine.send(&request).await {
        tracing::debug!("requesting a Claude title: {error:#}");
        decoder.finish_title();
    }
}

/// A stream-json user message: the text, then each image as a base64 block.
async fn user_line(input: &UserInput) -> Result<Value> {
    let mut content = Vec::new();
    let native_text = crate::normalize_command(artisan_api::EngineId::Claude, &input.text)?
        .unwrap_or_else(|| input.text.clone());
    if !native_text.is_empty() {
        content.push(json!({ "type": "text", "text": native_text }));
    }
    for path in &input.images {
        let bytes = tokio::fs::read(path)
            .await
            .with_context(|| format!("reading image {} for item {}", path.display(), input.id))?;
        content.push(json!({
            "type": "image",
            "source": {
                "type": "base64",
                "media_type": "image/webp",
                "data": base64::engine::general_purpose::STANDARD.encode(bytes),
            },
        }));
    }
    Ok(json!({
        "type": "user",
        "message": { "role": "user", "content": content },
        "parent_tool_use_id": null,
        "session_id": "",
    }))
}

fn launch_args(config: &EngineConfig, session: Option<&str>) -> Vec<String> {
    let mut args: Vec<String> = [
        "-p",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        "--verbose",
        "--include-partial-messages",
        "--forward-subagent-text",
        "--replay-user-messages",
        "--permission-prompt-tool",
        "stdio",
        // Public reasoning summaries; without it thinking streams empty.
        "--thinking-display",
        "summarized",
    ]
    .map(str::to_owned)
    .into();
    match config.permissions {
        // Allowed, not enabled: the user may switch to Full mid-run.
        Permissions::Auto => args.extend(
            [
                "--permission-mode",
                "auto",
                "--allow-dangerously-skip-permissions",
            ]
            .map(str::to_owned),
        ),
        Permissions::Full => args.push("--dangerously-skip-permissions".to_owned()),
    }
    if let Some(effort) = config.effort.as_deref().filter(|effort| !effort.is_empty()) {
        args.extend(["--effort".to_owned(), effort.to_owned()]);
    }
    if let Some(model) = config.model.as_deref().filter(|model| !model.is_empty()) {
        args.extend(["--model".to_owned(), model.to_owned()]);
    }
    // Fast mode is a setting (`fastMode`), not a flag; this run's settings
    // layer over the engine home's.
    let mut settings = serde_json::Map::new();
    if config.speed.as_deref() == Some(crate::catalog::CLAUDE_FAST) {
        settings.insert("fastMode".to_owned(), json!(true));
    }
    if !settings.is_empty() {
        args.extend(["--settings".to_owned(), Value::Object(settings).to_string()]);
    }
    if let Some(session) = session {
        args.extend(["--resume".to_owned(), session.to_owned()]);
    }
    args
}

/// The newest `ai-title` in the session's transcript, if any. Best effort.
fn transcript_title(turn: &Turn, session: &str) -> Option<String> {
    let path = config_dir(turn)
        .join("projects")
        .join(side::project_dir(turn))
        .join(format!("{session}.jsonl"));
    title_from_transcript(&path)
}

/// Where the CLI keeps its sessions for this launch.
fn config_dir(turn: &Turn) -> std::path::PathBuf {
    turn.launch
        .env
        .iter()
        .find(|(key, _)| key == "CLAUDE_CONFIG_DIR")
        .map_or_else(|| turn.home.join(".claude"), |(_, value)| value.into())
}

fn title_from_transcript(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    text.lines()
        .rev()
        .filter(|line| line.contains("\"ai-title\""))
        .find_map(|line| {
            let record: Value = serde_json::from_str(line).ok()?;
            let title = record.get("aiTitle")?.as_str()?.trim();
            (!title.is_empty()).then(|| title.to_owned())
        })
}

/// The CLI caps auto-compaction without changing the model's identity.
fn context_window_env(config: &EngineConfig) -> Option<(&'static str, String)> {
    config
        .context_window_override()
        .map(|tokens| ("CLAUDE_CODE_AUTO_COMPACT_WINDOW", tokens.to_string()))
}

#[cfg(test)]
mod tests {
    use super::ClaudeDecoder;
    use artisan_api::ItemId;

    #[tokio::test]
    async fn generated_title_is_persisted_without_replacing_the_claude_transcript() {
        use crate::{Harness as _, Launch, Turn, UserInput};
        use artisan_api::{EngineConfig, EngineId, ThreadId};
        let dir = tempfile::tempdir().unwrap();
        let turn = Turn {
            thread_id: ThreadId::new(),
            engine: EngineId::Claude,
            binary: dir.path().join("unused-claude"),
            home: dir.path().join("home"),
            cwd: dir.path().to_owned(),
            config: EngineConfig::new(EngineId::Claude),
            session_id: Some("working-session".to_owned()),
            input: UserInput::default(),
            launch: Launch::default(),
            trace_dir: None,
        };
        let path = super::config_dir(&turn)
            .join("projects")
            .join(super::side::project_dir(&turn))
            .join("working-session.jsonl");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let history = "{\"type\":\"user\",\"message\":{\"content\":\"repair the sidebar\"}}\n";
        std::fs::write(&path, history).unwrap();
        super::ClaudeHarness
            .set_title(turn, "Fix sidebar titles".to_owned())
            .await
            .unwrap();
        assert!(std::fs::read_to_string(&path).unwrap().starts_with(history));
        assert_eq!(
            super::title_from_transcript(&path).as_deref(),
            Some("Fix sidebar titles")
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn protocol_output_errors_fail_even_after_a_successful_result() {
        use crate::{Harness as _, Launch, RunIo, Turn, UserInput};
        use artisan_api::{EngineConfig, EngineId, RunStatus, ThreadId};
        use std::os::unix::fs::PermissionsExt as _;
        for (after_result, malformed) in
            [(false, false), (true, false), (false, true), (true, true)]
        {
            let directory = tempfile::tempdir().unwrap();
            let binary = directory.path().join("fake-claude");
            let earlier = if after_result {
                "printf '%s\\n' '{\"type\":\"result\",\"subtype\":\"success\",\"result\":\"done\",\"usage\":{}}'\n"
            } else {
                ""
            };
            let output = if malformed {
                "printf '%s\\n' '{broken-json'\n".to_owned()
            } else {
                format!(
                    "head -c {} /dev/zero\nprintf '\\n'\n",
                    crate::process::MAX_LINE_BYTES + 1
                )
            };
            let parent_pid = directory.path().join("parent-pid");
            let child_pid = directory.path().join("child-pid");
            std::fs::write(
                &binary,
                format!("#!/bin/sh\nprintf '%s\\n' \"$$\" > \"$ARTISAN_TEST_PARENT_PID\"\nsleep 30 & child=$!\nprintf '%s\\n' \"$child\" > \"$ARTISAN_TEST_CHILD_PID\"\nread -r input\n{earlier}{output}wait\n"),
            )
            .unwrap();
            std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
            let turn = Turn {
                trace_dir: None,
                thread_id: ThreadId::from("protocol-error"),
                engine: EngineId::Claude,
                binary,
                home: directory.path().join("home"),
                cwd: directory.path().into(),
                config: EngineConfig::new(EngineId::Claude),
                session_id: Some("existing".into()),
                input: UserInput {
                    text: "go".into(),
                    ..Default::default()
                },
                launch: Launch {
                    env: vec![
                        (
                            "ARTISAN_TEST_PARENT_PID".into(),
                            parent_pid.to_string_lossy().into_owned(),
                        ),
                        (
                            "ARTISAN_TEST_CHILD_PID".into(),
                            child_pid.to_string_lossy().into_owned(),
                        ),
                    ],
                    ..Launch::default()
                },
            };
            let (_sender, inbox) = tokio::sync::mpsc::channel(8);
            let (events, mut received) = tokio::sync::mpsc::channel(256);
            let drain = tokio::spawn(async move { while received.recv().await.is_some() {} });
            let outcome = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                super::ClaudeHarness.run(
                    turn,
                    RunIo {
                        inbox,
                        events,
                        cancel: tokio_util::sync::CancellationToken::new(),
                    },
                ),
            )
            .await
            .expect("protocol failure must not await a blocked process");
            assert_eq!(outcome.status, RunStatus::Failed);
            assert!(outcome.error.unwrap().contains(if malformed {
                "protocol JSON"
            } else {
                "exceeds"
            }));
            for (path, parent) in [(parent_pid, true), (child_pid, false)] {
                let pid = std::fs::read_to_string(path).unwrap();
                tokio::time::timeout(std::time::Duration::from_secs(3), async {
                    loop {
                        let mut command = tokio::process::Command::new("ps");
                        command.args(["-o", "stat=", "-p", pid.trim()]);
                        let state =
                            crate::process::probe(command, std::time::Duration::from_secs(3), 1024)
                                .await
                                .unwrap();
                        let state = String::from_utf8_lossy(&state.stdout);
                        if state.trim().is_empty() || (!parent && state.trim().starts_with('Z')) {
                            break;
                        }
                        // A delivered SIGKILL still needs the scheduler to stop
                        // descendants; prove they stop rather than accepting R.
                        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                    }
                })
                .await
                .expect("owned process is reaped and its descendant stops");
            }
            drain.await.unwrap();
        }
    }

    /// A background agent outlives the turn's `result`: input stays open
    /// until it reports back, or the CLI would kill it mid-work. The fake
    /// CLI fails the run if its input closes while the agent works.
    #[cfg(unix)]
    #[tokio::test]
    async fn input_stays_open_while_a_background_agent_works() {
        use crate::{Harness as _, Launch, RunIo, Turn, UserInput};
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let fake = dir.path().join("claude");
        std::fs::write(
            &fake,
            r#"#!/usr/bin/env bash
read -r line
echo '{"type":"user","isReplay":true,"message":{"content":"go"}}'
echo '{"type":"system","subtype":"init","session_id":"s1","model":"m"}'
echo '{"type":"system","subtype":"background_tasks_changed","tasks":[{"task_id":"a1","task_type":"local_agent"}]}'
echo '{"type":"result","subtype":"success","result":"started","usage":{}}'
sleep 1
read -r -t 0.05 rest; code=$?
if [ "$code" -eq 1 ]; then
  echo '{"type":"result","subtype":"error_during_execution","errors":["input closed while the agent worked"],"usage":{}}'
  exit 0
fi
echo '{"type":"system","subtype":"background_tasks_changed","tasks":[]}'
echo '{"type":"result","subtype":"success","result":"agent reported","usage":{}}'
cat > /dev/null
"#,
        )
        .unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        // Tracing is explicitly enabled for this diagnostic fixture.
        let home = dir.path().join("data/engines/claude/home");
        std::fs::create_dir_all(&home).unwrap();
        let turn = Turn {
            trace_dir: Some(dir.path().join("data/traces")),
            thread_id: artisan_api::ThreadId::from("thread"),
            engine: artisan_api::EngineId::Claude,
            binary: fake,
            home,
            cwd: dir.path().to_owned(),
            config: artisan_api::EngineConfig::new(artisan_api::EngineId::Claude),
            session_id: Some("s1".into()),
            input: UserInput {
                text: "go".into(),
                ..UserInput::default()
            },
            launch: Launch::default(),
        };
        let (_steers, inbox) = tokio::sync::mpsc::channel(8);
        let (events, mut received) = tokio::sync::mpsc::channel(256);
        let drain = tokio::spawn(async move { while received.recv().await.is_some() {} });
        let io = RunIo {
            inbox,
            events,
            cancel: tokio_util::sync::CancellationToken::new(),
        };
        let outcome = tokio::time::timeout(
            std::time::Duration::from_secs(20),
            super::ClaudeHarness.run(turn, io),
        )
        .await
        .expect("the run ends once the agent reported");
        drain.abort();
        assert_eq!(outcome.status, artisan_api::RunStatus::Done, "{outcome:?}");

        // The trace says what Claude wrote and why input stayed open, then
        // closed: what an agent killed mid-work is read back from.
        let traces = dir.path().join("data/traces");
        let mut trace = String::new();
        for _ in 0..200 {
            trace = std::fs::read_dir(&traces)
                .into_iter()
                .flatten()
                .flatten()
                .map(|entry| std::fs::read_to_string(entry.path()).unwrap_or_default())
                .collect();
            if trace.contains("exited") {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        for expected in [
            "background_tasks_changed",
            "input stays open for background work (1 background task(s)",
            "closing stdin: result (cancelling: false; 0 background task(s)",
            "exited",
        ] {
            assert!(
                trace.contains(expected),
                "the trace lacks {expected:?}:\n{trace}"
            );
        }
    }

    /// An agent's report starts a follow-up turn that outlasts the settle
    /// grace and launches the next agent. Closing input mid-turn made the
    /// CLI kill that agent ten minutes later.
    #[cfg(unix)]
    #[tokio::test]
    async fn input_stays_open_through_a_long_turn_after_an_agent_reports() {
        use crate::{Harness as _, Launch, RunIo, Turn, UserInput};
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let fake = dir.path().join("claude");
        std::fs::write(
            &fake,
            r#"#!/usr/bin/env bash
read -r line
echo '{"type":"user","isReplay":true,"message":{"content":"go"}}'
echo '{"type":"system","subtype":"init","session_id":"s1","model":"m"}'
echo '{"type":"system","subtype":"background_tasks_changed","tasks":[{"task_id":"a1","task_type":"local_agent"}]}'
echo '{"type":"result","subtype":"success","result":"started","usage":{}}'
echo '{"type":"system","subtype":"background_tasks_changed","tasks":[]}'
echo '{"type":"assistant","message":{"id":"m2","role":"assistant","content":[{"type":"text","text":"Reading the report."}]}}'
sleep 1
read -r -t 0.05 rest; code=$?
if [ "$code" -eq 1 ]; then
  echo '{"type":"result","subtype":"error_during_execution","errors":["input closed mid-turn"],"usage":{}}'
  exit 0
fi
echo '{"type":"system","subtype":"background_tasks_changed","tasks":[{"task_id":"a2","task_type":"local_agent"}]}'
echo '{"type":"result","subtype":"success","result":"launched the next agent","usage":{}}'
echo '{"type":"system","subtype":"background_tasks_changed","tasks":[]}'
echo '{"type":"result","subtype":"success","result":"agent reported","usage":{}}'
cat > /dev/null
"#,
        )
        .unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let home = dir.path().join("data/engines/claude/home");
        std::fs::create_dir_all(&home).unwrap();
        let turn = Turn {
            trace_dir: None,
            thread_id: artisan_api::ThreadId::from("thread"),
            engine: artisan_api::EngineId::Claude,
            binary: fake,
            home,
            cwd: dir.path().to_owned(),
            config: artisan_api::EngineConfig::new(artisan_api::EngineId::Claude),
            session_id: Some("s1".into()),
            input: UserInput {
                text: "go".into(),
                ..UserInput::default()
            },
            launch: Launch::default(),
        };
        let (_steers, inbox) = tokio::sync::mpsc::channel(8);
        let (events, mut received) = tokio::sync::mpsc::channel(256);
        let drain = tokio::spawn(async move { while received.recv().await.is_some() {} });
        let io = RunIo {
            inbox,
            events,
            cancel: tokio_util::sync::CancellationToken::new(),
        };
        // The grace (300 ms under test) elapses during the turn's 1 s pause.
        let outcome = tokio::time::timeout(
            std::time::Duration::from_secs(20),
            super::ClaudeHarness.run(turn, io),
        )
        .await
        .expect("the run ends once the second agent reported");
        drain.abort();
        assert_eq!(outcome.status, artisan_api::RunStatus::Done, "{outcome:?}");
    }

    /// A side query answers from a fork: a tool the fork reaches for is
    /// refused by its hook, even in a full-access thread, the thread's own
    /// transcript is left alone and the fork's is deleted.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_side_query_answers_from_a_fork_and_leaves_the_thread_alone() {
        use crate::{Harness as _, Launch, Turn, UserInput};
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let fake = dir.path().join("claude");
        std::fs::write(
            &fake,
            r#"#!/usr/bin/env bash
case " $* " in *" --resume s1 "*) ;; *) exit 3;; esac
case " $* " in *" --fork-session "*) ;; *) exit 3;; esac
case " $* " in *" --dangerously-skip-permissions "*) ;; *) exit 4;; esac
projects="$CLAUDE_CONFIG_DIR/projects/$(pwd | sed 's/[^A-Za-z0-9]/-/g')"
read -r init
case "$init" in *'"PreToolUse"'*'"hookCallbackIds"'*) ;; *) exit 6;; esac
read -r line
echo '{"type":"system","subtype":"init","session_id":"f1","model":"m"}'
echo '{}' > "$projects/f1.jsonl"
echo '{"type":"control_request","request_id":"r1","request":{"subtype":"hook_callback","callback_id":"artisan-side-refuse-tools","input":{"hook_event_name":"PreToolUse","tool_name":"Bash","tool_input":{"command":"rm -rf /"}}}}'
read -r answer
case "$answer" in *'"permissionDecision":"deny"'*) ;; *) exit 5;; esac
echo '{"type":"result","subtype":"success","is_error":false,"session_id":"f1","result":" Ship it. ","usage":{"input_tokens":10,"cache_read_input_tokens":9000,"cache_creation_input_tokens":0,"output_tokens":4}}'
cat > /dev/null
"#,
        )
        .unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let home = dir.path().join("home");
        let cwd = dir.path().join("project");
        std::fs::create_dir_all(&cwd).unwrap();
        let project: String = cwd
            .to_string_lossy()
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect();
        let projects = home.join(".claude").join("projects").join(project);
        std::fs::create_dir_all(&projects).unwrap();
        std::fs::write(projects.join("s1.jsonl"), "{}\n").unwrap();
        let mut config = artisan_api::EngineConfig::new(artisan_api::EngineId::Claude);
        config.permissions = artisan_api::Permissions::Full;
        let turn = Turn {
            trace_dir: None,
            thread_id: artisan_api::ThreadId::from("thread"),
            engine: artisan_api::EngineId::Claude,
            binary: fake,
            home,
            cwd,
            config,
            session_id: Some("s1".into()),
            input: UserInput {
                text: "What next?".into(),
                ..UserInput::default()
            },
            launch: Launch::default(),
        };
        let reply = tokio::time::timeout(
            std::time::Duration::from_secs(20),
            super::ClaudeHarness.side_query(turn, tokio_util::sync::CancellationToken::new()),
        )
        .await
        .expect("the fork answers")
        .expect("the side query succeeds");
        assert_eq!(reply.text, "Ship it.");
        assert_eq!(reply.usage.cached_input, 9000);
        assert_eq!(reply.usage.input, 9010);
        assert!(
            projects.join("s1.jsonl").exists(),
            "the thread's transcript stays"
        );
        assert!(
            !projects.join("f1.jsonl").exists(),
            "the fork's transcript is gone"
        );
    }

    /// Fast mode is Claude's `fastMode` setting, given for this run only.
    #[test]
    fn the_fast_speed_turns_on_fast_mode() {
        let mut config = artisan_api::EngineConfig::new(artisan_api::EngineId::Claude);
        config.model = Some("claude-opus-5-5".into());
        let settings = |config: &artisan_api::EngineConfig| {
            let args = super::launch_args(config, None);
            args.iter()
                .position(|arg| arg == "--settings")
                .map(|at| args[at + 1].clone())
        };
        assert_eq!(settings(&config), None);
        config.speed = Some("standard".into());
        assert_eq!(settings(&config), None);
        config.speed = Some("fast".into());
        assert_eq!(settings(&config).as_deref(), Some(r#"{"fastMode":true}"#));
    }

    #[test]
    fn capped_context_uses_compaction_environment_and_keeps_model_identity() {
        let mut config = artisan_api::EngineConfig::new(artisan_api::EngineId::Claude);
        config.model = Some("claude-opus-test".to_owned());
        assert_eq!(super::context_window_env(&config), None);
        config
            .options
            .insert("context_window".to_owned(), serde_json::json!(400_000));
        assert_eq!(
            super::context_window_env(&config),
            Some(("CLAUDE_CODE_AUTO_COMPACT_WINDOW", "400000".to_owned()))
        );
        let args = super::launch_args(&config, None);
        let model = args.iter().position(|arg| arg == "--model").unwrap();
        assert_eq!(args[model + 1], "claude-opus-test");
    }

    #[tokio::test]
    async fn ultra_commands_reach_claude_as_keywords_of_their_turn() {
        for (typed, sent) in [
            ("/ultrathink audit this", "ultrathink\naudit this"),
            ("/ultracode ship the parser", "ultracode\nship the parser"),
            ("ship the parser", "ship the parser"),
        ] {
            let input = crate::UserInput {
                text: typed.to_owned(),
                ..crate::UserInput::default()
            };
            let line = super::user_line(&input).await.unwrap();
            assert_eq!(line["message"]["content"][0]["text"], sent);
        }
    }

    /// Auto is Claude's own auto mode; Full skips every permission prompt.
    #[test]
    fn permissions_pick_auto_mode_or_skip_prompts() {
        let mut config = artisan_api::EngineConfig::new(artisan_api::EngineId::Claude);
        let args = super::launch_args(&config, None);
        let mode = args.iter().position(|arg| arg == "--permission-mode");
        assert_eq!(mode.map(|at| args[at + 1].as_str()), Some("auto"));
        assert!(
            !args
                .iter()
                .any(|arg| arg == "--dangerously-skip-permissions")
        );

        config.permissions = artisan_api::Permissions::Full;
        let args = super::launch_args(&config, None);
        assert!(
            args.iter()
                .any(|arg| arg == "--dangerously-skip-permissions")
        );
        assert!(!args.iter().any(|arg| arg == "--permission-mode"));
    }

    /// Every recorded transcript, decoded. The snapshots are the mapping:
    /// a wrong item kind, a lost tool call, duplicated subagent text or a
    /// missed approval shows up as a diff.
    #[test]
    fn transcripts() {
        let directory = concat!(env!("CARGO_MANIFEST_DIR"), "/transcripts/claude");
        let mut files: Vec<_> = std::fs::read_dir(directory)
            .expect("transcripts directory")
            .map(|entry| entry.expect("entry").path())
            .filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "jsonl")
            })
            .collect();
        files.sort();
        assert!(!files.is_empty());
        for path in files {
            let name = path
                .file_stem()
                .expect("name")
                .to_string_lossy()
                .into_owned();
            let text = crate::transcript_tests::read_path(&path);
            // Write what the driver writes: the message, the title request
            // on a fresh session, and every later user line as a steer.
            let mut decoder = ClaudeDecoder::default();
            let mut events = Vec::new();
            let mut user_lines = 0;
            if text.contains("\"artisan-title\"") {
                decoder.title_request("first message");
            }
            // A line Claude's protocol can't carry fails the run.
            let mut protocol_errors = Vec::new();
            for line in text.lines() {
                if line.contains("\"isReplay\":true") {
                    user_lines += 1;
                    decoder.expect_replay(
                        (user_lines > 1).then(|| ItemId::from(format!("steer-{user_lines}"))),
                    );
                }
                match decoder.feed(line) {
                    Ok(line_events) => events.extend(line_events),
                    Err(error) => protocol_errors.push(format!("{error:#}")),
                }
            }
            events.extend(decoder.finish());
            let outcome = decoder.take_outcome();
            if protocol_errors.is_empty() {
                insta::assert_debug_snapshot!(name, (events, outcome));
            } else {
                insta::assert_debug_snapshot!(name, (events, outcome, protocol_errors));
            }
        }
    }

    fn turn_end(case: &str) -> crate::Outcome {
        let text = crate::transcript_tests::read("claude", case);
        let mut decoder = ClaudeDecoder::default();
        for line in text.lines() {
            decoder.feed(line).unwrap();
        }
        decoder.take_outcome().expect("the turn ends")
    }

    /// The CLI says the plan's limit was hit before the request, and when
    /// it resets; the failed turn waits for that.
    #[test]
    fn a_turn_the_usage_limit_rejected_waits_for_the_reset_it_named() {
        let outcome = turn_end("usage-limit");
        assert_eq!(outcome.status, artisan_api::RunStatus::Failed);
        assert_eq!(
            outcome.retry,
            Some(crate::Retry::UsageLimit {
                resets_at: Some(1_791_931_800_000)
            })
        );
    }

    /// An overloaded API (529) fails the turn, and Forge may retry it.
    #[test]
    fn an_overloaded_turn_can_be_retried() {
        let outcome = turn_end("overloaded");
        assert_eq!(outcome.status, artisan_api::RunStatus::Failed);
        assert_eq!(outcome.retry, Some(crate::Retry::Overloaded));
    }

    /// A foreground command past its timeout moves to the background
    /// (recorded: Claude 2.1.296). It stays running, in the background,
    /// until its task notification, not ending at its tool result.
    #[test]
    fn a_command_moved_to_the_background_runs_until_its_notification() {
        use artisan_api::Status::{Done, Running};
        let text = crate::transcript_tests::read("claude", "command-moved-to-background");
        let mut decoder = ClaudeDecoder::default();
        let mut states = Vec::new();
        for line in text.lines() {
            for event in decoder.feed(line).unwrap() {
                if let crate::EngineEvent::Item {
                    status,
                    kind:
                        artisan_api::ItemKind::Tool {
                            tool: artisan_api::ToolKind::Shell { background, .. },
                            output,
                            ..
                        },
                    ..
                } = event
                {
                    states.push((status, background, output.is_some()));
                }
            }
        }
        assert_eq!(
            states,
            [
                (Running, false, false),
                (Running, true, false),
                // Its result only says it moved.
                (Running, true, true),
                (Done, true, true),
            ],
        );
    }

    /// An agent that ends its turn to wait on its own background shell is
    /// resumed when the shell ends (recorded: Claude 2.1.296). Closing
    /// input then would let the CLI kill it mid-work, and its "completed"
    /// progress line is not its report.
    #[test]
    fn an_agent_waiting_on_its_own_shell_keeps_the_session_open_and_reports_at_the_end() {
        let text = crate::transcript_tests::read("claude", "agent-waits-on-its-shell");
        let mut decoder = ClaudeDecoder::default();
        let mut results = Vec::new();
        let mut open_while_waiting = true;
        for line in text.lines() {
            for event in decoder.feed(line).unwrap() {
                if let crate::EngineEvent::Item {
                    status,
                    kind: artisan_api::ItemKind::Agent { result, .. },
                    ..
                } = event
                {
                    results.push((status, result));
                }
            }
            // The agent's shell is the only live task while it sleeps.
            if line.contains("\"local_bash\"") && line.contains("background_tasks_changed") {
                open_while_waiting &= decoder.background_running();
            }
        }
        assert!(
            open_while_waiting,
            "the session keeps taking input while the agent's shell runs"
        );
        assert_eq!(
            results.last(),
            Some(&(artisan_api::Status::Done, Some("FINISHED".into()))),
            "the agent's report is what it said once resumed: {results:?}"
        );
        assert!(
            results
                .iter()
                .all(|(status, _)| *status == artisan_api::Status::Running
                    || results.last().is_some_and(|(last, _)| last == status)),
            "the agent never shows finished while it waits: {results:?}"
        );
    }
}
