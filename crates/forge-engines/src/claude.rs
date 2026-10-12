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
mod tools;

pub use decoder::ClaudeDecoder;

use crate::process::{self, EngineProcess};
use crate::{EngineEvent, Harness, Input, Outcome, RunIo, Turn, UserInput};
use anyhow::{Context, Result};
use artisan_api::{EngineConfig, Permissions};
use base64::Engine as _;
use futures::future::BoxFuture;
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::path::Path;
use std::time::Duration;

/// How long an interrupted turn may take to wind down before it is killed.
const INTERRUPT_GRACE: Duration = Duration::from_secs(5);
/// How long input stays open after the last background agent ends with no
/// follow-up turn. The ending agent queues one whose frames arrive well
/// inside this; it only bounds the case where none comes.
const SETTLED_INPUT_GRACE: Duration = Duration::from_secs(30);

pub struct ClaudeHarness;

impl Harness for ClaudeHarness {
    fn run(&self, turn: Turn, io: RunIo) -> BoxFuture<'static, Outcome> {
        Box::pin(run(turn, io))
    }
}

async fn run(turn: Turn, mut io: RunIo) -> Outcome {
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
            &mut engine,
            &mut decoder,
            &mut io,
            &mut next,
            &mut inbox_open,
        )
        .await;
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
            outcome = Some(result)
        } else {
            let stderr = engine.stderr_tail();
            tracing::warn!(thread = %turn.thread_id, ?code, "Claude exited without a result: {stderr}");
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
async fn drive(
    engine: &mut EngineProcess,
    decoder: &mut ClaudeDecoder,
    io: &mut RunIo,
    next: &mut VecDeque<(UserInput, bool)>,
    inbox_open: &mut bool,
) -> Finished {
    let mut cancelling = false;
    let grace = tokio::time::sleep(Duration::from_secs(u64::from(u32::MAX)));
    tokio::pin!(grace);
    // Background agents outlived a `result`; input stays open for them.
    let mut waiting_on_background = false;
    let settle = tokio::time::sleep(Duration::from_secs(u64::from(u32::MAX)));
    tokio::pin!(settle);
    let mut settling = false;
    loop {
        tokio::select! {
            line = engine.next_line() => {
                let line = match line {
                    Ok(Some(line)) => line,
                    Ok(None) => break,
                    Err(error) => {
                        tracing::warn!("reading Claude output: {error:#}");
                        break;
                    }
                };
                for event in decoder.feed(&line) {
                    io.emit(event).await;
                }
                for reply in decoder.take_replies() {
                    if let Err(error) = engine.send(&reply).await {
                        tracing::warn!("answering Claude: {error:#}");
                    }
                }
                // Closing input ends the session: the CLI then waits only a
                // bounded while (`CLAUDE_CODE_PRINT_BG_WAIT_CEILING_MS`,
                // about 10 minutes) for background agents, kills them
                // mid-work and refuses steers meanwhile. So input closes at
                // a `result` only once no steer, title request or
                // background agent is still on its way.
                if decoder.take_result_seen() && engine.stdin_open() {
                    if cancelling || (!decoder.awaiting_input() && !decoder.background_running()) {
                        engine.close_stdin();
                    } else if decoder.background_running() {
                        waiting_on_background = true;
                    }
                }
                // The last agent ended: its report starts a follow-up turn
                // whose `result` closes input; failing that, the grace does.
                if waiting_on_background && !decoder.background_running() && !settling {
                    settling = true;
                    settle.as_mut().reset(tokio::time::Instant::now() + SETTLED_INPUT_GRACE);
                } else if decoder.background_running() {
                    settling = false;
                }
            }
            () = &mut settle, if settling => {
                settling = false;
                if engine.stdin_open() && !decoder.awaiting_input() && !decoder.background_running() {
                    engine.close_stdin();
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
                            tracing::warn!(key, "answering Claude: {error:#}");
                        }
                    }
                }
                None => *inbox_open = false,
            },
            () = io.cancel.cancelled(), if !cancelling => {
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
                tracing::warn!("Claude did not stop after an interrupt; killing it");
                engine.kill().await;
                grace.as_mut().reset(tokio::time::Instant::now() + Duration::from_secs(3600));
            }
        }
    }
    Finished {
        cancelled: cancelling,
    }
}

async fn deliver(
    engine: &mut EngineProcess,
    decoder: &mut ClaudeDecoder,
    input: &UserInput,
    steer: bool,
) -> Result<()> {
    let line = user_line(input).await?;
    engine.send(&line).await?;
    decoder.expect_replay(steer.then(|| input.id.clone()));
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
    if !input.text.is_empty() {
        content.push(json!({ "type": "text", "text": input.text }));
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
        Permissions::Auto => args.extend(["--permission-mode".to_owned(), "auto".to_owned()]),
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
    if config.speed.as_deref() == Some(crate::catalog::CLAUDE_FAST) {
        args.extend(["--settings".to_owned(), r#"{"fastMode":true}"#.to_owned()]);
    }
    if let Some(session) = session {
        args.extend(["--resume".to_owned(), session.to_owned()]);
    }
    args
}

/// The newest `ai-title` in the session's transcript, if any. Best effort.
fn transcript_title(turn: &Turn, session: &str) -> Option<String> {
    let config_dir = turn
        .launch
        .env
        .iter()
        .find(|(key, _)| key == "CLAUDE_CONFIG_DIR")
        .map_or_else(|| turn.home.join(".claude"), |(_, value)| value.into());
    let cwd = turn.cwd.to_string_lossy();
    let directory: String = cwd
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let path = config_dir
        .join("projects")
        .join(directory)
        .join(format!("{session}.jsonl"));
    title_from_transcript(&path)
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

#[cfg(test)]
mod tests {
    use super::ClaudeDecoder;
    use artisan_api::ItemId;

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
        let turn = Turn {
            thread_id: artisan_api::ThreadId::from("thread"),
            engine: artisan_api::EngineId::Claude,
            binary: fake,
            home: dir.path().to_owned(),
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
            let text = std::fs::read_to_string(&path).expect("transcript");
            // Write what the driver writes: the message, the title request
            // on a fresh session, and every later user line as a steer.
            let mut decoder = ClaudeDecoder::default();
            let mut events = Vec::new();
            let mut user_lines = 0;
            if text.contains("\"artisan-title\"") {
                decoder.title_request("first message");
            }
            for line in text.lines() {
                if line.contains("\"isReplay\":true") {
                    user_lines += 1;
                    decoder.expect_replay(
                        (user_lines > 1).then(|| ItemId::from(format!("steer-{user_lines}"))),
                    );
                }
                events.extend(decoder.feed(line));
            }
            events.extend(decoder.finish());
            let outcome = decoder.take_outcome();
            insta::assert_debug_snapshot!(name, (events, outcome));
        }
    }

    /// An agent that ends its turn to wait on its own background shell is
    /// resumed when the shell ends (recorded: Claude 2.1.296). Closing
    /// input then would let the CLI kill it mid-work, and its "completed"
    /// progress line is not its report.
    #[test]
    fn an_agent_waiting_on_its_own_shell_keeps_the_session_open_and_reports_at_the_end() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/transcripts/claude/agent-waits-on-its-shell.jsonl"
        );
        let text = std::fs::read_to_string(path).expect("transcript");
        let mut decoder = ClaudeDecoder::default();
        let mut results = Vec::new();
        let mut open_while_waiting = true;
        for line in text.lines() {
            for event in decoder.feed(line) {
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
            Some(&(artisan_api::Status::Done, Some("FINISHED".to_owned()))),
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

    /// After a session is resumed in a new process, an agent from the
    /// earlier one is resumed too; its report must still reach its item.
    #[test]
    fn an_agent_resumed_in_a_new_process_still_reports() {
        let mut decoder = ClaudeDecoder::default();
        let mut events = Vec::new();
        for line in [
            serde_json::json!({"type": "system", "subtype": "task_started", "task_id": "a4c1", "tool_use_id": "toolu_linnea", "task_type": "local_agent", "description": "Attachments drawn on guns", "is_backgrounded": true}),
            serde_json::json!({"type": "system", "subtype": "background_tasks_changed", "tasks": [{"task_id": "a4c1", "task_type": "local_agent"}]}),
            serde_json::json!({"type": "system", "subtype": "task_notification", "task_id": "a4c1", "tool_use_id": "toolu_linnea", "status": "completed", "summary": "Attachments now show on the gun."}),
        ] {
            events.extend(decoder.feed(&line.to_string()));
        }
        let last = events.iter().rev().find_map(|event| match event {
            crate::EngineEvent::Item {
                key,
                status,
                kind: artisan_api::ItemKind::Agent { result, .. },
                ..
            } if key == "toolu_linnea" => Some((*status, result.clone())),
            _ => None,
        });
        assert_eq!(
            last,
            Some((
                artisan_api::Status::Done,
                Some("Attachments now show on the gun.".to_owned())
            ))
        );
    }

    /// A background agent's view shows the task it got and what it really
    /// reported: Claude notifies each time the agent stops ("finished" with
    /// a progress line, then again with the report), and a stopped agent's
    /// summary is only its description.
    #[test]
    fn a_background_agent_reports_its_latest_result_and_never_its_description() {
        let agent = |id: &str, description: &str| {
            serde_json::json!({"type": "assistant", "message": {"id": format!("msg-{id}"), "content": [{
                "type": "tool_use", "id": id, "name": "Agent",
                "input": {"description": description, "prompt": format!("Do {description} in full."), "run_in_background": true}
            }]}})
            .to_string()
        };
        let notify = |id: &str, status: &str, summary: &str| {
            serde_json::json!({"type": "system", "subtype": "task_notification", "tool_use_id": id, "status": status, "summary": summary})
                .to_string()
        };
        let mut decoder = ClaudeDecoder::default();
        let mut events = Vec::new();
        for line in [
            agent("toolu_a", "Faithful CMAA2 port"),
            agent("toolu_b", "Temporal mode inputs"),
            notify("toolu_a", "stopped", "Faithful CMAA2 port"),
            notify("toolu_b", "completed", "Waiting for the suite to finish."),
            notify(
                "toolu_b",
                "completed",
                "Temporal mode is in on `render-taa`.",
            ),
        ] {
            events.extend(decoder.feed(&line));
        }
        let last = |key: &str| {
            events.iter().rev().find_map(|event| match event {
                crate::EngineEvent::Item {
                    key: k,
                    status,
                    kind: artisan_api::ItemKind::Agent { prompt, result, .. },
                    ..
                } if k == key => Some((*status, prompt.clone(), result.clone())),
                _ => None,
            })
        };
        assert_eq!(
            last("toolu_a"),
            Some((
                artisan_api::Status::Cancelled,
                Some("Do Faithful CMAA2 port in full.".to_owned()),
                None
            )),
            "a stopped agent reports nothing, and its task is the prompt"
        );
        assert_eq!(
            last("toolu_b").and_then(|(_, _, result)| result).as_deref(),
            Some("Temporal mode is in on `render-taa`."),
            "the later report replaces the progress line"
        );
    }
}
