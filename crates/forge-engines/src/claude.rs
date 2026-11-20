mod decoder;
mod door;
mod side;
pub(crate) mod tools;

pub(crate) use decoder::{ClaudeDecoder, DecodeError};
use door::{Door, Ending, Session};
pub use side::SideQueryError;

use crate::process::{self, EngineProcess, Exit, ReadError, SendError, StderrWritten};
use crate::{EngineEvent, Harness, Input, Outcome, RunIo, SideReply, Turn, UserInput};
use artisan_api::{EngineConfig, ItemId, Permissions, ThreadId};
use base64::Engine as _;
use futures::future::BoxFuture;
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

const INTERRUPT_GRACE: Duration = Duration::from_secs(5);
const EXIT_GRACE: Duration = Duration::from_secs(5);
const SETTLED_INPUT_GRACE: Duration = Duration::from_secs(30);
const SIDE_QUERY_LIMIT: Duration = Duration::from_secs(120);

pub(crate) trait InputGrace: Send + Sync + std::fmt::Debug {
    fn length(&self) -> Duration;
    fn countdown(&self) -> BoxFuture<'static, ()>;
}

#[derive(Debug)]
pub(crate) struct WallClock;

impl InputGrace for WallClock {
    fn length(&self) -> Duration {
        SETTLED_INPUT_GRACE
    }

    fn countdown(&self) -> BoxFuture<'static, ()> {
        Box::pin(tokio::time::sleep(SETTLED_INPUT_GRACE))
    }
}

pub(crate) fn retitle(
    turn: Turn,
    title: String,
) -> BoxFuture<'static, Result<(), crate::SetTitleError>> {
    let thread = turn.thread_id.clone();
    Box::pin(async move {
        match turn
            .session_id
            .as_deref()
            .and_then(|session| session.strip_prefix(artisan_api::CLAUDE_AI_SESSION_PREFIX))
        {
            Some(native) => rename_on_claude_ai(&turn, native, &title).await,
            None => set_title(&turn, &title).map_err(|source| crate::SetTitleError::Claude {
                thread,
                source: Box::new(source),
            }),
        }
    })
}

#[derive(Debug, Clone)]
pub struct ClaudeHarness {
    grace: std::sync::Arc<dyn InputGrace>,
}

impl ClaudeHarness {
    pub(crate) fn new(grace: std::sync::Arc<dyn InputGrace>) -> Self {
        Self { grace }
    }
}

impl Harness for ClaudeHarness {
    fn set_title(
        &self,
        turn: Turn,
        title: String,
    ) -> BoxFuture<'static, Result<(), crate::SetTitleError>> {
        retitle(turn, title)
    }

    fn compact(
        &self,
        turn: Turn,
        cancel: CancellationToken,
    ) -> BoxFuture<'static, Result<SideReply, crate::SideQueryError>> {
        let native = turn.session_id.as_deref().map_or_else(
            || {
                turn.config.options.get("transport").and_then(Value::as_str)
                    == Some(CLAUDE_AI_TRANSPORT)
            },
            |id| id.starts_with(artisan_api::CLAUDE_AI_SESSION_PREFIX),
        );
        let thread = turn.thread_id.clone();
        if native {
            Box::pin(async move {
                crate::claudeai::compact(turn, cancel)
                    .await
                    .map_err(|source| crate::SideQueryError::ClaudeAi {
                        thread,
                        source: Box::new(source),
                    })
            })
        } else {
            Box::pin(async move {
                side::compact(turn, cancel)
                    .await
                    .map_err(|source| crate::SideQueryError::Claude {
                        thread,
                        source: Box::new(source),
                    })
            })
        }
    }

    fn run(&self, turn: Turn, io: RunIo) -> BoxFuture<'static, Outcome> {
        Box::pin(run(turn, io, std::sync::Arc::clone(&self.grace)))
    }

    fn side_query(
        &self,
        turn: Turn,
        cancel: CancellationToken,
    ) -> BoxFuture<'static, Result<SideReply, crate::SideQueryError>> {
        let thread = turn.thread_id.clone();
        if turn.session_id.as_deref().map_or_else(
            || {
                turn.config.options.get("transport").and_then(Value::as_str)
                    == Some(CLAUDE_AI_TRANSPORT)
            },
            |id| id.starts_with(artisan_api::CLAUDE_AI_SESSION_PREFIX),
        ) {
            Box::pin(async move {
                crate::claudeai::side_query(turn, cancel)
                    .await
                    .map_err(|source| crate::SideQueryError::ClaudeAi {
                        thread,
                        source: Box::new(source),
                    })
            })
        } else {
            Box::pin(async move {
                side::side_query(turn, cancel).await.map_err(|source| {
                    crate::SideQueryError::Claude {
                        thread,
                        source: Box::new(source),
                    }
                })
            })
        }
    }
}

async fn rename_on_claude_ai(
    turn: &Turn,
    native: &str,
    title: &str,
) -> Result<(), crate::SetTitleError> {
    let conversation = native
        .split_once('/')
        .map_or(native, |(conversation, _)| conversation);
    crate::claudeai::set_title(&turn.home, conversation, title)
        .await
        .map_err(|source| crate::SetTitleError::ClaudeAi {
            thread: turn.thread_id.clone(),
            source: Box::new(source),
        })
}

#[derive(Debug, thiserror::Error)]
pub enum SetTitleError {
    #[error("thread {thread} has no Claude session")]
    NoSession { thread: ThreadId },
    #[error("encode the title record of session {session}")]
    Encode {
        session: String,
        #[source]
        source: serde_json::Error,
    },
    #[error("append the title to {}", path.display())]
    Append {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum ExchangeError {
    #[error("read Claude output")]
    Read(#[source] ReadError),
    #[error("decode Claude output")]
    Decode(#[source] DecodeError),
}

#[derive(Debug, thiserror::Error)]
pub enum DeliverError {
    #[error("expand the command in message {item}")]
    Command {
        item: ItemId,
        #[source]
        source: crate::commands::MissingPrompt,
    },
    #[error("read image {} of message {item}", path.display())]
    Image {
        item: ItemId,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("send message {item}")]
    Send {
        item: ItemId,
        #[source]
        source: SendError,
    },
}

fn set_title(turn: &Turn, title: &str) -> Result<(), SetTitleError> {
    let session = turn
        .session_id
        .as_deref()
        .ok_or_else(|| SetTitleError::NoSession {
            thread: turn.thread_id.clone(),
        })?;
    let path = config_dir(turn)
        .join("projects")
        .join(side::project_dir(turn))
        .join(format!("{session}.jsonl"));
    let mut record = serde_json::to_vec(&json!({
        "type": "ai-title", "aiTitle": title, "sessionId": session,
    }))
    .map_err(|source| SetTitleError::Encode {
        session: session.to_owned(),
        source,
    })?;
    record.push(b'\n');
    let appended = match path.strip_prefix(&turn.home) {
        Ok(relative) => crate::sandbox::home::append(&turn.home, relative, &record),
        Err(_outside_the_home) => std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .and_then(|mut file| file.write_all(&record)),
    };
    appended.map_err(|source| SetTitleError::Append { path, source })
}

fn apply_env(turn: &mut Turn) {
    if let Some((key, value)) = context_window_env(&turn.config) {
        turn.launch.env.retain(|(existing, _)| existing != key);
        turn.launch.env.push((key.to_owned(), value));
    }
}

async fn run(turn: Turn, io: RunIo, grace: std::sync::Arc<dyn InputGrace>) -> Outcome {
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
    run_claude_code(turn, io, grace.as_ref()).await
}

pub const CLAUDE_AI_TRANSPORT: &str = "claude.ai";
pub const CLAUDE_AI_UNAVAILABLE: &str = "claude.ai-unavailable";

async fn run_claude_code(mut turn: Turn, mut io: RunIo, grace: &dyn InputGrace) -> Outcome {
    apply_env(&mut turn);
    let mut decoder = ClaudeDecoder::default();
    let mut session = turn.session_id.clone();
    let fresh = session.is_none();
    let mut next: VecDeque<(UserInput, bool)> = VecDeque::from([(turn.input.clone(), false)]);
    let mut inbox_open = true;
    let mut outcome = None;

    while !next.is_empty() && !io.cancel.is_cancelled() {
        let args = launch_args(&turn.config, session.as_deref(), &turn.launch.readable);
        let mut engine = match process::spawn(&turn, &args).await {
            Ok(engine) => engine,
            Err(failure) => {
                let error: &dyn std::error::Error = &failure;
                tracing::error!(thread = %turn.thread_id, error, "Claude could not start");
                return Outcome::failed(format!(
                    "Claude could not start: {}",
                    diagnostics::chain(&failure)
                ));
            }
        };
        let mut delivered = 0;
        while let Some((input, steer)) = next.pop_front() {
            if let Err(failure) = deliver(&mut engine, &mut decoder, &input, steer).await {
                let error: &dyn std::error::Error = &failure;
                tracing::error!(thread = %turn.thread_id, item = %input.id, error, "Claude did not take a message");
                return Outcome::failed(format!(
                    "Claude did not take the message: {}",
                    diagnostics::chain(&failure)
                ));
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
            grace,
        )
        .await;
        let finished = match finished {
            Ok(finished) => finished,
            Err(failure) => {
                {
                    let error: &dyn std::error::Error = &failure;
                    tracing::warn!(thread = %turn.thread_id, session = ?decoder.session(), error, "Claude protocol exchange failed");
                }
                engine.kill().await;
                emit_finished(&mut decoder, &io).await;
                return Outcome::failed(format!(
                    "Claude protocol exchange failed: {}",
                    diagnostics::chain(&failure)
                ));
            }
        };
        let (exit, cancelled) = reap(&mut engine, &io.cancel, finished.cancelled).await;
        emit_finished(&mut decoder, &io).await;
        if let Some(id) = decoder.session() {
            session = Some(id.to_owned());
        }
        if cancelled {
            return Outcome::cancelled();
        }
        if let Some(result) = decoder.take_outcome() {
            outcome = Some(result);
        } else {
            let stderr = engine.stderr();
            let failure = exited_without_result(&exit, &stderr);
            tracing::warn!(thread = %turn.thread_id, session = ?decoder.session(), exit = %diagnostics::chain(&exit), stderr_bytes = stderr.bytes(), "Claude exited without a result");
            return failure;
        }
        if outcome
            .as_ref()
            .is_some_and(|outcome: &Outcome| *outcome != Outcome::Done)
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

async fn emit_finished(decoder: &mut ClaudeDecoder, io: &RunIo) {
    for event in decoder.finish() {
        io.emit(event).await;
    }
}

fn exited_without_result(exit: &Exit, stderr: &StderrWritten) -> Outcome {
    Outcome::failed(format!(
        "Claude {} before finishing; {stderr}",
        diagnostics::chain(exit)
    ))
}

struct Finished {
    cancelled: bool,
}

async fn reap(
    engine: &mut EngineProcess,
    cancel: &CancellationToken,
    cancelling: bool,
) -> (Exit, bool) {
    let cancelled = tokio::select! {
        exit = engine.wait() => return (exit, cancelling),
        () = cancel.cancelled() => true,
        () = tokio::time::sleep(EXIT_GRACE) => cancelling,
    };
    engine.note("the output closed but the process lives on: killing it");
    engine.kill().await;
    (Exit::Killed, cancelled)
}

#[expect(
    clippy::too_many_lines,
    reason = "one select loop; each arm is one event source"
)]
async fn drive(
    thread: &ThreadId,
    engine: &mut EngineProcess,
    decoder: &mut ClaudeDecoder,
    io: &mut RunIo,
    next: &mut VecDeque<(UserInput, bool)>,
    inbox_open: &mut bool,
    grace: &dyn InputGrace,
) -> Result<Finished, ExchangeError> {
    let mut cancelling = false;
    let interrupted = tokio::time::sleep(Duration::from_secs(u64::from(u32::MAX)));
    tokio::pin!(interrupted);
    let mut door = Door::Open;
    let mut settle: Option<BoxFuture<'static, ()>> = None;
    loop {
        tokio::select! {
            line = engine.next_line() => {
                let line = match line {
                    Ok(Some(line)) => line,
                    Ok(None) => break,
                    Err(error) => {
                        return Err(ExchangeError::Read(error));
                    }
                };
                for event in decoder.feed(&line).map_err(ExchangeError::Decode)? {
                    io.emit(event).await;
                }
                for reply in decoder.take_replies() {
                    if let Err(error) = engine.send(&reply).await {
                        tracing::warn!(%thread, session = ?decoder.session(), "answering Claude: {error:#}");
                    }
                }
                let session = Session {
                    in_turn: decoder.in_turn(),
                    awaiting_input: decoder.awaiting_input(),
                    background: decoder.background_running(),
                };
                let ending = decoder.take_result_seen().then_some(if cancelling {
                    Ending::Cancelled
                } else {
                    Ending::Replied
                });
                let next = ending.map_or_else(
                    || door.after_frame(session),
                    |ending| door.after_result(session, ending),
                );
                match (door, next) {
                    (Door::Settling, Door::Settling) => {}
                    (_, Door::Settling) => settle = Some(grace.countdown()),
                    (
                        _,
                        Door::Open | Door::HeldForReply | Door::HeldForBackground | Door::Closed,
                    ) => settle = None,
                }
                let why = ending.map_or(Moved::Frame, Moved::Result);
                move_door(engine, decoder, grace, &mut door, next, why);
            }
            () = counted_down(&mut settle), if settle.is_some() => {
                settle = None;
                let next = door.after_grace(Session {
                    in_turn: decoder.in_turn(),
                    awaiting_input: decoder.awaiting_input(),
                    background: decoder.background_running(),
                });
                move_door(engine, decoder, grace, &mut door, next, Moved::Grace);
            }
            input = io.inbox.recv(), if *inbox_open && !cancelling => match input {
                Some(Input::Steer(steer)) => {
                    let delivered = door.takes_input()
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
                interrupted.as_mut().reset(tokio::time::Instant::now() + INTERRUPT_GRACE);
            }
            () = &mut interrupted, if cancelling => {
                tracing::warn!(%thread, session = ?decoder.session(), "Claude did not stop after an interrupt; killing it");
                engine.kill().await;
                interrupted.as_mut().reset(tokio::time::Instant::now() + Duration::from_secs(3600));
            }
        }
    }
    Ok(Finished {
        cancelled: cancelling,
    })
}

async fn counted_down(settle: &mut Option<BoxFuture<'static, ()>>) {
    match settle {
        Some(countdown) => countdown.await,
        None => std::future::pending().await,
    }
}

#[derive(Clone, Copy)]
enum Moved {
    Frame,
    Result(Ending),
    Grace,
}

fn move_door(
    engine: &mut EngineProcess,
    decoder: &ClaudeDecoder,
    grace: &dyn InputGrace,
    door: &mut Door,
    next: Door,
    why: Moved,
) {
    let from = std::mem::replace(door, next);
    if from == next {
        return;
    }
    let jobs = decoder.background_summary();
    match (next, why) {
        (Door::Closed, Moved::Grace) => engine.close_stdin(&format!(
            "no background work for {}s ({jobs})",
            grace.length().as_secs()
        )),
        (Door::Closed, Moved::Result(ending)) => engine.close_stdin(&format!(
            "result (cancelling: {}; {jobs})",
            ending == Ending::Cancelled
        )),
        (Door::Closed, Moved::Frame) => {
            engine.close_stdin("what held input open after the result was answered");
        }
        (Door::HeldForBackground, Moved::Result(_)) => {
            engine.note(&format!(
                "result; input stays open for background work ({jobs})"
            ));
        }
        (Door::HeldForBackground, Moved::Frame | Moved::Grace) if from == Door::Settling => {
            engine.note("background work started again; input stays open");
        }
        (Door::HeldForReply, Moved::Result(_)) => {
            engine.note("result; input stays open: a steer or title request is on its way");
        }
        (Door::Open, Moved::Frame) if from == Door::Settling => {
            engine.note("a turn started while input was counting down; input stays open");
        }
        (Door::Settling, _) => engine.note(&format!(
            "background work ended; input closes in {}s unless more starts ({jobs})",
            grace.length().as_secs()
        )),
        (Door::Open | Door::HeldForReply | Door::HeldForBackground, _) => {}
    }
}

async fn deliver(
    engine: &mut EngineProcess,
    decoder: &mut ClaudeDecoder,
    input: &UserInput,
    _steer: bool,
) -> Result<(), DeliverError> {
    let line = user_line(input).await?;
    engine
        .send(&line)
        .await
        .map_err(|source| DeliverError::Send {
            item: input.id.clone(),
            source,
        })?;
    decoder.expect_replay(Some(input.id.clone()));
    Ok(())
}

async fn request_title(engine: &mut EngineProcess, decoder: &mut ClaudeDecoder, text: &str) {
    if text.trim().is_empty() {
        return;
    }
    let request = decoder.title_request(text);
    if let Err(failure) = engine.send(&request).await {
        let error: &dyn std::error::Error = &failure;
        tracing::debug!(error, "Claude did not take the title request");
        decoder.finish_title();
    }
}

async fn user_line(input: &UserInput) -> Result<Value, DeliverError> {
    let mut content = Vec::new();
    let native_text = crate::normalize_command(artisan_api::EngineId::Claude, &input.text)
        .map_err(|source| DeliverError::Command {
            item: input.id.clone(),
            source,
        })?
        .unwrap_or_else(|| input.text.clone());
    if !native_text.is_empty() {
        content.push(json!({ "type": "text", "text": native_text }));
    }
    for path in &input.images {
        let bytes = tokio::fs::read(path)
            .await
            .map_err(|source| DeliverError::Image {
                item: input.id.clone(),
                path: path.clone(),
                source,
            })?;
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

fn launch_args(config: &EngineConfig, session: Option<&str>, readable: &[PathBuf]) -> Vec<String> {
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
        "--thinking-display",
        "summarized",
    ]
    .map(str::to_owned)
    .into();
    match config.permissions {
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
    let mut settings = serde_json::Map::new();
    match config.speed.as_deref() {
        Some(crate::catalog::CLAUDE_FAST) => {
            settings.insert("fastMode".into(), json!(true));
        }
        Some(crate::catalog::STANDARD_SPEED) => {
            settings.insert("fastMode".into(), json!(false));
        }
        _ => {}
    }
    if !settings.is_empty() {
        args.extend(["--settings".to_owned(), Value::Object(settings).to_string()]);
    }
    for dir in readable {
        args.extend(["--add-dir".to_owned(), dir.to_string_lossy().into_owned()]);
    }
    if let Some(session) = session {
        args.extend(["--resume".to_owned(), session.to_owned()]);
    }
    args
}

fn transcript_title(turn: &Turn, session: &str) -> Option<String> {
    let path = config_dir(turn)
        .join("projects")
        .join(side::project_dir(turn))
        .join(format!("{session}.jsonl"));
    title_from_transcript(&path)
}

fn config_dir(turn: &Turn) -> PathBuf {
    turn.launch
        .env
        .iter()
        .find(|(key, _)| key == "CLAUDE_CONFIG_DIR")
        .map_or_else(|| turn.home.join(".claude"), |(_, value)| value.into())
}

fn title_from_transcript(path: &Path) -> Option<String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(error) => {
            tracing::debug!(path = %path.display(), %error, "the Claude transcript can't be read for its title");
            return None;
        }
    };
    text.lines()
        .rev()
        .filter(|line| line.contains("\"ai-title\""))
        .find_map(|line| match serde_json::from_str::<Value>(line) {
            Ok(record) => {
                let title = record.get("aiTitle")?.as_str()?.trim();
                (!title.is_empty()).then(|| title.to_owned())
            }
            Err(error) => {
                tracing::debug!(path = %path.display(), %error, "skipped a transcript title line that isn't JSON");
                None
            }
        })
}

fn context_window_env(config: &EngineConfig) -> Option<(&'static str, String)> {
    config
        .compaction_window_override()
        .or_else(|| {
            config
                .context_window_override()
                .filter(|tokens| *tokens == 500_000)
        })
        .map(|tokens| ("CLAUDE_CODE_AUTO_COMPACT_WINDOW", tokens.to_string()))
}

#[cfg(test)]
#[path = "../tests/claude/scenario.rs"]
mod scenario;

#[cfg(test)]
#[path = "../tests/claude/cancellation.rs"]
mod cancellation_tests;

#[cfg(test)]
#[path = "../tests/claude/background.rs"]
mod background_tests;

#[cfg(test)]
#[path = "../tests/claude/door.rs"]
mod door_tests;

#[cfg(test)]
#[path = "../tests/claude/reply.rs"]
mod reply_tests;

#[cfg(test)]
#[path = "../tests/claude/tools.rs"]
mod tools_tests;

#[cfg(test)]
#[path = "../tests/claude/unit.rs"]
mod tests;
