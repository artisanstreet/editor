//! The ACP adapter: one Agent Client Protocol client (JSON-RPC over stdio)
//! with thin profiles for Grok (`grok agent stdio`) and Cursor
//! (`cursor-agent acp`).
//!
//! One process per run: `initialize`, `session/new` or `session/load`,
//! then `session/prompt` until the prompt returns. ACP has no in-place
//! steer, so a steer cancels the prompt and the next prompt carries it.

mod decode;
#[cfg(test)]
mod tests;

use crate::process::{self, EngineProcess};
use crate::{EngineEvent, Harness, Input, McpServer, Outcome, RunIo, Turn, UserInput};
use anyhow::{Context, Result};
use artisan_api::{EngineConfig, Permissions, RunStatus};
use base64::Engine as _;
use decode::{AcpDecoder, needs_sign_in};
use futures::future::BoxFuture;
use serde_json::{Value, json};
use std::time::Duration;
use tokio::time::Instant;

/// How long a cancelled prompt may take to return before the engine is killed.
const CANCEL_GRACE: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Profile {
    Grok,
    Cursor,
}

impl Profile {
    const fn name(self) -> &'static str {
        match self {
            Self::Grok => "Grok",
            Self::Cursor => "Cursor",
        }
    }

    /// Launch flags. Model, effort and permissions are CLI flags for both
    /// engines; neither needs a `session/set_*` call.
    fn args(self, config: &EngineConfig) -> Vec<String> {
        let model = config.model.as_deref().filter(|model| !model.is_empty());
        let effort = config.effort.as_deref().filter(|effort| !effort.is_empty());
        let mut args = Vec::new();
        match self {
            Self::Grok => {
                args.push("--no-auto-update".to_owned());
                if config.permissions == Permissions::Auto {
                    args.extend(["--permission-mode".to_owned(), "auto".to_owned()]);
                }
                args.extend(["agent".to_owned(), "--no-leader".to_owned()]);
                if let Some(model) = model {
                    args.extend(["--model".to_owned(), model.to_owned()]);
                }
                if let Some(effort) = effort {
                    args.extend(["--reasoning-effort".to_owned(), effort.to_owned()]);
                }
                if config.permissions == Permissions::Full {
                    args.push("--always-approve".to_owned());
                }
                // `agent` options go before the transport subcommand.
                args.push("stdio".to_owned());
            }
            Self::Cursor => {
                if let Some(model) = model {
                    args.extend([
                        "--model".to_owned(),
                        cursor_model(model, effort, config.speed.as_deref() == Some("fast")),
                    ]);
                }
                // Cursor has no automatic mode: Auto is its default, which
                // asks before terminal commands.
                if config.permissions == Permissions::Full {
                    args.push("--force".to_owned());
                }
                args.push("acp".to_owned());
            }
        }
        args
    }
}

/// Cursor model ids carry effort and speed as suffixes (`gpt-5.6-high-fast`)
/// or as bracket parameters (`model[effort=high]`), which pass through.
fn cursor_model(model: &str, effort: Option<&str>, fast: bool) -> String {
    if model.contains('[') {
        return model.to_owned();
    }
    let base = model.strip_suffix("-fast").unwrap_or(model);
    let has_effort = base.rsplit_once('-').is_some_and(|(_, suffix)| {
        matches!(
            suffix,
            "low" | "medium" | "high" | "xhigh" | "max" | "ultra"
        )
    });
    let mut resolved = match effort {
        Some(effort) if !has_effort => format!("{base}-{effort}"),
        _ => base.to_owned(),
    };
    if fast || model.ends_with("-fast") {
        resolved.push_str("-fast");
    }
    resolved
}

pub(crate) struct AcpHarness {
    pub(crate) profile: Profile,
}

impl Harness for AcpHarness {
    fn run(&self, turn: Turn, io: RunIo) -> BoxFuture<'static, Outcome> {
        let profile = self.profile;
        Box::pin(async move {
            let process = match process::spawn(&turn, &profile.args(&turn.config)) {
                Ok(process) => process,
                Err(error) => {
                    tracing::error!(engine = profile.name(), thread = %turn.thread_id, "starting the engine: {error:#}");
                    return Outcome::failed(format!("{error:#}"));
                }
            };
            let mut run = Run {
                profile,
                process,
                decoder: AcpDecoder::new(profile),
                io,
                steers: Vec::new(),
                cancelled: false,
            };
            let outcome = run.run(&turn).await.unwrap_or_else(|error| {
                tracing::warn!(engine = profile.name(), thread = %turn.thread_id, session = ?turn.session_id, "ACP run failed: {error:#}");
                Outcome::failed(format!("{error:#}"))
            });
            let events = run.decoder.end_run(outcome.status != RunStatus::Done);
            run.emit(events).await;
            run.process.kill().await;
            outcome
        })
    }
}

/// Why a handshake call didn't produce a result.
enum CallError {
    Cancelled,
    /// The engine answered with an error.
    Rejected(crate::rpc::RpcError),
    Failed(anyhow::Error),
}

struct Run {
    profile: Profile,
    process: EngineProcess,
    decoder: AcpDecoder,
    io: RunIo,
    /// Steers waiting for the cancelled prompt to return.
    steers: Vec<UserInput>,
    cancelled: bool,
}

impl Run {
    async fn run(&mut self, turn: &Turn) -> Result<Outcome> {
        let initialize = json!({
            "protocolVersion": 1,
            "clientCapabilities": {
                "fs": { "readTextFile": false, "writeTextFile": false },
                "terminal": false,
                "elicitation": { "form": {} },
            },
            "clientInfo": { "name": "Artisan", "version": env!("CARGO_PKG_VERSION") },
        });
        match self.call("initialize", initialize).await {
            Ok(_) => {}
            Err(CallError::Cancelled) => return Ok(Outcome::cancelled()),
            Err(CallError::Rejected(error)) => {
                return Err(anyhow::Error::new(error).context("ACP initialize"));
            }
            Err(CallError::Failed(error)) => return Err(error.context("ACP initialize")),
        }
        let mut params =
            json!({ "cwd": turn.cwd, "mcpServers": mcp_servers(&turn.launch.mcp_servers) });
        let method = match &turn.session_id {
            Some(session) => {
                params["sessionId"] = json!(session);
                "session/load"
            }
            None => "session/new",
        };
        match self.call(method, params).await {
            Ok(_) => {}
            Err(CallError::Cancelled) => return Ok(Outcome::cancelled()),
            Err(CallError::Rejected(error)) if needs_sign_in(&error) => {
                return Ok(Outcome::failed(format!(
                    "Sign in to {} to use it.",
                    self.profile.name()
                )));
            }
            Err(CallError::Rejected(error)) => {
                return Err(anyhow::Error::new(error)
                    .context(format!("ACP {method} (session {:?})", turn.session_id)));
            }
            Err(CallError::Failed(error)) => {
                return Err(error.context(format!("ACP {method} (session {:?})", turn.session_id)));
            }
        }
        self.prompt(std::slice::from_ref(&turn.input)).await?;
        self.io
            .emit(EngineEvent::InputAccepted {
                id: turn.input.id.clone(),
            })
            .await;
        self.prompt_loop().await
    }

    /// Sends a request and waits for its response, decoding whatever the
    /// engine sends meanwhile. Used before the first prompt.
    async fn call(&mut self, method: &str, params: Value) -> Result<Value, CallError> {
        let id = self.decoder.request(method, params);
        self.flush().await.map_err(CallError::Failed)?;
        loop {
            let line = tokio::select! {
                line = self.process.next_line() => line.map_err(CallError::Failed)?,
                () = self.io.cancel.cancelled() => return Err(CallError::Cancelled),
            };
            let Some(line) = line else {
                return Err(CallError::Failed(self.exited(method)));
            };
            self.handle_line(&line).await.map_err(CallError::Failed)?;
            if let Some(response) = self
                .decoder
                .take_responses()
                .into_iter()
                .find(|response| response.id == id)
            {
                return response.result.map_err(CallError::Rejected);
            }
        }
    }

    async fn prompt_loop(&mut self) -> Result<Outcome> {
        let mut inbox_open = true;
        let mut deadline = None;
        loop {
            tokio::select! {
                line = self.process.next_line() => {
                    let Some(line) = line? else {
                        if self.cancelled {
                            return Ok(Outcome::cancelled());
                        }
                        return Err(self.exited("session/prompt"));
                    };
                    self.handle_line(&line).await?;
                    for response in self.decoder.take_responses() {
                        if response.method != "session/prompt" {
                            continue;
                        }
                        if self.cancelled {
                            return Ok(Outcome::cancelled());
                        }
                        // A prompt cancelled for a steer may end in an error; the steer goes on.
                        if let Err(error) = response.result
                            && self.steers.is_empty()
                        {
                            return Ok(Outcome::failed(error.message));
                        }
                        // A steer that raced the prompt's end still starts the next prompt.
                        while let Ok(input) = self.io.inbox.try_recv() {
                            self.input(input).await?;
                        }
                        if self.steers.is_empty() {
                            return Ok(match self.decoder.stop_reason.as_deref() {
                                Some("cancelled") => Outcome::cancelled(),
                                Some("refusal") => Outcome { status: RunStatus::Refused, error: None },
                                _ => Outcome::done(),
                            });
                        }
                        let steers = std::mem::take(&mut self.steers);
                        self.prompt(&steers).await?;
                        for steer in steers {
                            self.io.emit(EngineEvent::InputAccepted { id: steer.id }).await;
                        }
                    }
                }
                input = self.io.inbox.recv(), if inbox_open && !self.cancelled => match input {
                    Some(input) => self.input(input).await?,
                    None => inbox_open = false,
                },
                () = self.io.cancel.cancelled(), if !self.cancelled => {
                    self.cancelled = true;
                    self.cancel_prompt().await?;
                    deadline = Some(Instant::now() + CANCEL_GRACE);
                }
                () = tokio::time::sleep_until(deadline.unwrap_or_else(Instant::now)), if deadline.is_some() => {
                    return Ok(Outcome::cancelled());
                }
            }
        }
    }

    async fn input(&mut self, input: Input) -> Result<()> {
        match input {
            Input::Steer(steer) => {
                if self.steers.is_empty() {
                    self.cancel_prompt().await?;
                }
                self.steers.push(steer);
            }
            Input::Answer { key, answer } => {
                let events = self.decoder.answer(&key, &answer);
                self.emit(events).await;
                self.flush().await?;
            }
        }
        Ok(())
    }

    async fn cancel_prompt(&mut self) -> Result<()> {
        let events = self.decoder.cancel_pending();
        self.emit(events).await;
        let session = self.decoder.session_id.clone();
        self.decoder
            .notify("session/cancel", json!({ "sessionId": session }));
        self.flush().await
    }

    /// Starts a prompt with one or more user messages (several when steers
    /// piled up while the previous prompt wound down).
    async fn prompt(&mut self, inputs: &[UserInput]) -> Result<()> {
        let text = inputs
            .iter()
            .map(|input| input.text.as_str())
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n");
        let mut content = Vec::new();
        if !text.is_empty() {
            content.push(json!({ "type": "text", "text": text }));
        }
        for path in inputs.iter().flat_map(|input| &input.images) {
            if !self.decoder.image_blocks && !self.decoder.embedded_context {
                tracing::warn!(engine = self.profile.name(), session = ?self.decoder.session_id, path = %path.display(), "engine takes no images; leaving one out");
                continue;
            }
            let bytes = tokio::fs::read(path)
                .await
                .with_context(|| format!("reading image {}", path.display()))?;
            let data = base64::engine::general_purpose::STANDARD.encode(bytes);
            content.push(if self.decoder.image_blocks {
                json!({ "type": "image", "mimeType": "image/webp", "data": data })
            } else {
                // Grok takes images only as embedded resources.
                json!({ "type": "resource", "resource": { "uri": format!("file://{}", path.display()), "mimeType": "image/webp", "blob": data } })
            });
        }
        if content.is_empty() {
            content.push(json!({ "type": "text", "text": "" }));
        }
        let session = self
            .decoder
            .session_id
            .clone()
            .context("no ACP session to prompt")?;
        self.decoder.request(
            "session/prompt",
            json!({ "sessionId": session, "prompt": content }),
        );
        self.flush().await
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
                .with_context(|| format!("writing to {}", self.profile.name()))?;
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
        anyhow::anyhow!(
            "{} exited during {during}{}",
            self.profile.name(),
            if tail.is_empty() {
                String::new()
            } else {
                format!(": {tail}")
            }
        )
    }
}

/// ACP `McpServer` entries for `session/new` and `session/load`.
fn mcp_servers(servers: &[McpServer]) -> Value {
    servers
        .iter()
        .map(|server| match &server.url {
            Some(url) => json!({ "type": "http", "name": server.name, "url": url, "headers": [] }),
            None => json!({
                "name": server.name,
                "command": server.command,
                "args": server.args,
                "env": server.env.iter().map(|(name, value)| json!({ "name": name, "value": value })).collect::<Vec<_>>(),
            }),
        })
        .collect()
}
