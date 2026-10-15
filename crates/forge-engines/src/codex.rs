//! Codex over `codex app-server` (JSON-RPC, one JSON object per line).
//!
//! One process per run: `initialize`, `thread/start` or `thread/resume`,
//! then `turn/start`. A steer goes in as `turn/steer` against the running
//! turn; if Codex says the turn already ended, the steer starts the next
//! turn in the same process. The run ends when the root thread's turn
//! completes with nothing left to send. Cancel is `turn/interrupt`.
//!
//! Verified against Codex 0.162.1: approvals answered `acceptForSession`
//! skip the next identical prompt and `decline` declines the call, though
//! `availableDecisions` lists neither; MCP servers in the managed config
//! reach every thread (a `mcp_servers` table in `thread/start` would replace
//! that config, so it is not sent); a
//! steer after the turn ended is rejected with "no active turn to steer".

mod citations;
mod decoder;
mod protocol;
#[cfg(test)]
mod tests;

use crate::process::{self, EngineProcess};
use crate::{EngineEvent, Harness, Input, Outcome, RunIo, Turn, UserInput};
use anyhow::{Context, Result, anyhow};
use artisan_api::{EngineConfig, ItemId, Permissions};
use decoder::Decoder;
use futures::future::BoxFuture;
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::time::Duration;
use tokio::time::Instant;

/// How long an interrupted turn may take to wind down before Codex is killed.
const INTERRUPT_GRACE: Duration = Duration::from_secs(5);

pub struct CodexHarness;

impl Harness for CodexHarness {
    fn run(&self, turn: Turn, io: RunIo) -> BoxFuture<'static, Outcome> {
        Box::pin(async move {
            let thread = turn.thread_id.clone();
            let process = match start(&turn) {
                Ok(process) => process,
                Err(error) => {
                    tracing::error!(%thread, "starting Codex: {error:#}");
                    return Outcome::failed(format!("Codex could not start: {error:#}"));
                }
            };
            let mut run = Run {
                process,
                decoder: Decoder::default(),
                io,
                queued: Vec::new(),
                starting: None,
                steering: HashMap::new(),
                cancelling: false,
                nudge: false,
                outcome: None,
            };
            let outcome = run.run(&turn).await.unwrap_or_else(|error| {
                tracing::warn!(%thread, session = ?turn.session_id, "Codex run failed: {error:#}");
                Outcome::failed(format!("{error:#}"))
            });
            let events = run.decoder.finish();
            run.emit(events).await;
            run.process.close_stdin("the turn ended");
            run.process.kill().await;
            outcome
        })
    }
}

/// Codex refuses to start when `CODEX_HOME` doesn't exist.
fn start(turn: &Turn) -> Result<EngineProcess> {
    let codex_home = turn
        .launch
        .env
        .iter()
        .find(|(key, _)| key == "CODEX_HOME")
        .map_or_else(|| turn.home.join(".codex"), |(_, value)| value.into());
    std::fs::create_dir_all(&codex_home)
        .with_context(|| format!("creating CODEX_HOME {}", codex_home.display()))?;
    process::spawn(turn, &["app-server".to_owned()])
}

/// One queued provider input and whether it acknowledges a persisted user input.
struct Queued {
    input: UserInput,
    /// Provider-generated follow-ups have no durable user input to acknowledge.
    acknowledge: bool,
}

struct Run {
    process: EngineProcess,
    decoder: Decoder,
    io: RunIo,
    /// Messages for the next `turn/start`.
    queued: Vec<Queued>,
    /// The `turn/start` in flight and the durable input IDs it acknowledges.
    starting: Option<(u64, Vec<ItemId>)>,
    /// `turn/steer` requests in flight.
    steering: HashMap<u64, Queued>,
    cancelling: bool,
    /// Start a turn with no new input: Codex holds a message it never answered.
    nudge: bool,
    /// How the last finished turn ended.
    outcome: Option<Outcome>,
}

impl Run {
    async fn run(&mut self, turn: &Turn) -> Result<Outcome> {
        let initialize = json!({
            "clientInfo": { "name": "artisan", "title": "Artisan", "version": env!("CARGO_PKG_VERSION") },
            // Experimental: `item/tool/requestUserInput`, `turn/steer` fields.
            "capabilities": {
                "experimentalApi": true,
                "optOutNotificationMethods": [
                    "account/rateLimits/updated",
                    "mcpServer/startupStatus/updated",
                    "remoteControl/status/changed",
                ],
            },
        });
        let Some(_) = self.call("initialize", initialize).await? else {
            return Ok(Outcome::cancelled());
        };
        self.decoder.notify("initialized");
        let (method, params) = thread_params(turn);
        match self.call(method, params).await {
            Ok(Some(_)) => {}
            Ok(None) => return Ok(Outcome::cancelled()),
            Err(error) => {
                return Err(
                    error.context(format!("Codex {method} (session {:?})", turn.session_id))
                );
            }
        }
        self.queued.push(Queued {
            input: turn.input.clone(),
            acknowledge: true,
        });
        self.pump(turn).await?;
        self.drive(turn).await
    }

    /// Sends a request and waits for its result, decoding whatever Codex
    /// sends meanwhile. `None` when the run was cancelled.
    async fn call(&mut self, method: &str, params: Value) -> Result<Option<Value>> {
        let id = self.decoder.request(method, params);
        self.flush().await?;
        loop {
            let line = tokio::select! {
                line = self.process.next_line() => line?,
                () = self.io.cancel.cancelled() => return Ok(None),
            };
            let Some(line) = line else {
                return Err(self.exited(method));
            };
            self.handle_line(&line).await?;
            if let Some(response) = self
                .decoder
                .take_responses()
                .into_iter()
                .find(|response| response.id == id)
            {
                return response
                    .result
                    .map(Some)
                    .map_err(|error| anyhow!("Codex {method}: {error}"));
            }
        }
    }

    async fn drive(&mut self, turn: &Turn) -> Result<Outcome> {
        let mut inbox_open = true;
        let mut deadline: Option<Instant> = None;
        loop {
            tokio::select! {
                line = self.process.next_line() => {
                    let Some(line) = line? else {
                        if self.cancelling {
                            return Ok(Outcome::cancelled());
                        }
                        return Err(self.exited("the turn"));
                    };
                    self.handle_line(&line).await?;
                    for response in self.decoder.take_responses() {
                        self.responded(response).await?;
                    }
                    if let Some(outcome) = self.decoder.take_turn_end() {
                        if self.cancelling {
                            return Ok(Outcome::cancelled());
                        }
                        self.outcome = Some(outcome);
                        if self.decoder.take_stranded() {
                            tracing::debug!(thread = %turn.thread_id, "Codex ended the turn without answering a steer; starting another");
                            self.nudge = true;
                        }
                    }
                    for text in self.decoder.take_messages() {
                        self.queued.push(Queued { input: UserInput { id: ItemId::default(), text, images: Vec::new() }, acknowledge: false });
                    }
                    if self.idle() {
                        // A steer that raced the turn's end still gets its turn.
                        while let Ok(input) = self.io.inbox.try_recv() {
                            self.input(input).await?;
                        }
                    }
                    self.pump(turn).await?;
                    if self.idle()
                        && self.queued.is_empty()
                        && !self.nudge
                        && let Some(outcome) = self.outcome.take()
                    {
                        return Ok(outcome);
                    }
                }
                input = self.io.inbox.recv(), if inbox_open && !self.cancelling => match input {
                    Some(input) => {
                        self.input(input).await?;
                        self.pump(turn).await?;
                    }
                    None => inbox_open = false,
                },
                () = self.io.cancel.cancelled(), if !self.cancelling => {
                    self.cancelling = true;
                    let (Some(thread), Some(turn_id)) = (self.decoder.thread(), self.decoder.active_turn()) else {
                        return Ok(Outcome::cancelled());
                    };
                    let params = json!({ "threadId": thread, "turnId": turn_id });
                    self.decoder.request("turn/interrupt", params);
                    self.flush().await?;
                    deadline = Some(Instant::now() + INTERRUPT_GRACE);
                }
                () = tokio::time::sleep_until(deadline.unwrap_or_else(Instant::now)), if deadline.is_some() => {
                    tracing::warn!(thread = %turn.thread_id, "Codex did not stop after an interrupt; killing it");
                    return Ok(Outcome::cancelled());
                }
            }
        }
    }

    /// No turn running and nothing on its way to Codex.
    fn idle(&self) -> bool {
        self.decoder.active_turn().is_none() && self.starting.is_none() && self.steering.is_empty()
    }

    async fn input(&mut self, input: Input) -> Result<()> {
        match input {
            Input::Steer(input) => self.queued.push(Queued {
                input,
                acknowledge: true,
            }),
            Input::Answer { key, answer } => {
                let events = self.decoder.answer(&key, &answer);
                self.emit(events).await;
                self.flush().await?;
            }
        }
        Ok(())
    }

    /// Sends queued messages: as steers into the running turn, or as the
    /// next turn once the last one is over.
    async fn pump(&mut self, turn: &Turn) -> Result<()> {
        if self.cancelling || self.starting.is_some() || (self.queued.is_empty() && !self.nudge) {
            return Ok(());
        }
        let thread = self.decoder.thread().context("no Codex thread")?.to_owned();
        if let Some(turn_id) = self.decoder.active_turn().map(str::to_owned) {
            for queued in std::mem::take(&mut self.queued) {
                let params = json!({ "threadId": thread, "expectedTurnId": turn_id, "input": user_input(&queued.input) });
                let id = self.decoder.request("turn/steer", params);
                self.steering.insert(id, queued);
            }
        } else if self.steering.is_empty() {
            self.nudge = false;
            let queued = std::mem::take(&mut self.queued);
            let input: Vec<Value> = queued
                .iter()
                .flat_map(|queued| user_input(&queued.input))
                .collect();
            let mut params = json!({ "threadId": thread, "input": input });
            turn_overrides(&turn.config, &mut params);
            let id = self.decoder.request("turn/start", params);
            let inputs = queued
                .into_iter()
                .filter(|queued| queued.acknowledge)
                .map(|queued| queued.input.id)
                .collect();
            self.starting = Some((id, inputs));
        }
        self.flush().await
    }

    async fn responded(&mut self, response: decoder::Response) -> Result<()> {
        match response.method.as_str() {
            "turn/start" => {
                let Some((_, steers)) = self.starting.take_if(|(id, _)| *id == response.id) else {
                    return Ok(());
                };
                if let Err(error) = response.result {
                    return Err(anyhow!("Codex turn/start: {error}"));
                }
                for id in steers {
                    self.io.emit(EngineEvent::InputAccepted { id }).await;
                }
            }
            "turn/steer" => {
                let Some(queued) = self.steering.remove(&response.id) else {
                    return Ok(());
                };
                match response.result {
                    Ok(_) => {
                        if queued.acknowledge {
                            self.io
                                .emit(EngineEvent::InputAccepted {
                                    id: queued.input.id,
                                })
                                .await;
                        }
                    }
                    Err(error) => {
                        tracing::debug!(item = %queued.input.id, "Codex takes rejected steer as the next turn: {error}");
                        self.queued.insert(0, queued);
                    }
                }
            }
            "turn/interrupt" => {
                if let Err(error) = response.result {
                    tracing::debug!("Codex turn/interrupt: {error}");
                }
            }
            _ => {}
        }
        Ok(())
    }

    async fn handle_line(&mut self, line: &str) -> Result<()> {
        let events = self.decoder.feed(line)?;
        self.emit(events).await;
        self.flush().await
    }

    async fn flush(&mut self) -> Result<()> {
        for message in self.decoder.take_outbox() {
            self.process
                .send(&message)
                .await
                .context("writing to Codex")?;
        }
        Ok(())
    }

    async fn emit(&self, events: Vec<EngineEvent>) {
        for event in events {
            self.io.emit(event).await;
        }
    }

    fn exited(&self, during: &str) -> anyhow::Error {
        let tail = self.process.stderr_tail();
        anyhow!(
            "Codex exited during {during}{}",
            if tail.is_empty() {
                String::new()
            } else {
                format!(": {tail}")
            }
        )
    }
}

/// `thread/start`, or `thread/resume` for a known session, with the run's
/// model, sandbox and approval policy.
fn thread_params(turn: &Turn) -> (&'static str, Value) {
    let config = &turn.config;
    let mut settings = Map::new();
    settings.insert("model_reasoning_summary".to_owned(), json!("auto"));
    if let Some(effort) = non_empty(config.effort.as_deref()) {
        settings.insert("model_reasoning_effort".to_owned(), json!(effort));
    }
    if let Some(tokens) = config.context_window_override() {
        settings.insert("model_context_window".to_owned(), json!(tokens));
    }
    let mut params = json!({
        "cwd": turn.cwd,
        "config": settings,
    });
    match config.permissions {
        // Codex's own Auto: sandboxed to the project, with what it would
        // ask about reviewed by its automatic reviewer first.
        Permissions::Auto => {
            params["approvalPolicy"] = json!("on-request");
            params["approvalsReviewer"] = json!("auto_review");
            params["sandbox"] = json!("workspace-write");
        }
        Permissions::Full => {
            params["approvalPolicy"] = json!("never");
            params["sandbox"] = json!("danger-full-access");
        }
    }
    if let Some(model) = non_empty(config.model.as_deref()) {
        params["model"] = json!(model);
    }
    if let Some(tier) = service_tier(config) {
        params["serviceTier"] = json!(tier);
    }
    match &turn.session_id {
        Some(session) => {
            params["threadId"] = json!(session);
            // Only the thread's identity is needed, not its history.
            params["excludeTurns"] = json!(true);
            ("thread/resume", params)
        }
        None => ("thread/start", params),
    }
}

fn turn_overrides(config: &EngineConfig, params: &mut Value) {
    if let Some(effort) = non_empty(config.effort.as_deref()) {
        params["effort"] = json!(effort);
    }
    if let Some(tier) = service_tier(config) {
        params["serviceTier"] = json!(tier);
    }
}

/// The chosen tier's id (`priority`); the standard tier sends nothing.
fn service_tier(config: &EngineConfig) -> Option<&str> {
    match non_empty(config.speed.as_deref()) {
        Some(speed) => Some(speed).filter(|speed| *speed != crate::catalog::STANDARD_SPEED),
        None => None,
    }
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.filter(|value| !value.is_empty())
}

/// Codex `UserInput` items: the text, then each image by path.
fn user_input(input: &UserInput) -> Vec<Value> {
    let mut items = Vec::new();
    if !input.text.is_empty() || input.images.is_empty() {
        items.push(json!({ "type": "text", "text": input.text, "text_elements": [] }));
    }
    for path in &input.images {
        items.push(json!({ "type": "localImage", "path": path }));
    }
    items
}
