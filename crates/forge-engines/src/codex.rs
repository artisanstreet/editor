#[cfg(test)]
#[path = "../tests/codex/activity.rs"]
mod activity_tests;
mod citations;
mod decoder;
mod goal;
mod protocol;
#[cfg(test)]
#[path = "../tests/codex/schema.rs"]
mod schema_tests;
mod server;
#[cfg(test)]
#[path = "../tests/codex/server/unit.rs"]
mod server_tests;
mod side;
#[cfg(test)]
#[path = "../tests/codex/unit.rs"]
mod tests;

pub use decoder::DecodeError;
pub use goal::{CallError, ClearGoalError};
pub use server::{AttachError, Ended, InitializeError, Overloaded, ReleaseError, StartError};
pub use side::SideQueryError;

use crate::{EngineEvent, Harness, Input, Outcome, RunIo, SideReply, Turn, UserInput};
use artisan_api::{EngineConfig, ItemId, Permissions, ThreadId};
use decoder::Decoder;
use futures::future::BoxFuture;
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::time::Duration;
use tokio::time::Instant;

const INTERRUPT_GRACE: Duration = Duration::from_secs(5);

#[derive(Debug, thiserror::Error)]
pub enum SetTitleError {
    #[error("thread {thread} has no Codex session")]
    NoSession { thread: ThreadId },
    #[error("attach to the Codex app-server")]
    Attach(#[source] AttachError),
    #[error("Codex didn't name session {session} in 30s")]
    TimedOut { session: String },
    #[error("name session {session}")]
    Call {
        session: String,
        #[source]
        source: CallError,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum RunError {
    #[error(
        "Codex session {session} is open in another Codex process, which holds its history. Close it there, then send the message again."
    )]
    SessionBusy {
        session: String,
        #[source]
        source: crate::rpc::RpcError,
    },
    #[error("start session {session:?} with {method}")]
    Open {
        method: &'static str,
        session: Option<String>,
        #[source]
        source: RequestError,
    },
    #[error("Codex exited during {during}{}", if ended.is_empty() { String::new() } else { format!(": {ended}") })]
    Exited { during: &'static str, ended: String },
    #[error("read Codex output during {during}")]
    Overloaded {
        during: &'static str,
        #[source]
        source: Overloaded,
    },
    #[error("decode Codex output")]
    Decode(#[source] DecodeError),
    #[error("write to Codex")]
    Write(#[source] Ended),
    #[error("Codex refused turn/start")]
    TurnStart(#[source] crate::rpc::RpcError),
    #[error("Codex answered turn/start without a turn id")]
    NoTurn,
    #[error("Codex has no thread for this run")]
    NoThread,
    #[error("{}; requested thread unload and retired the server. Underlying work may still be running.", if *interrupt_sent { "Codex did not confirm the turn stopped after an interrupt" } else { "Codex did not return the started turn ID before cancellation timed out" })]
    StopTimedOut { interrupt_sent: bool },
}

#[derive(Debug, thiserror::Error)]
pub enum RequestError {
    #[error("Codex exited{}", if ended.is_empty() { String::new() } else { format!(": {ended}") })]
    Exited { ended: String },
    #[error("read Codex output")]
    Overloaded(#[source] Overloaded),
    #[error("decode Codex output")]
    Decode(#[source] DecodeError),
    #[error("write to Codex")]
    Write(#[source] Ended),
    #[error("Codex refused the request")]
    Refused(#[source] crate::rpc::RpcError),
}

const ACTIVE_WRITER: &str = "already has an active writer";

#[derive(Debug)]
pub struct CodexHarness;

impl Harness for CodexHarness {
    fn set_title(
        &self,
        turn: Turn,
        title: String,
    ) -> BoxFuture<'static, Result<(), crate::SetTitleError>> {
        let thread = turn.thread_id.clone();
        Box::pin(async move {
            set_title(turn, title)
                .await
                .map_err(|source| crate::SetTitleError::Codex {
                    thread,
                    source: Box::new(source),
                })
        })
    }

    fn compact(
        &self,
        turn: Turn,
        cancel: tokio_util::sync::CancellationToken,
    ) -> BoxFuture<'static, Result<SideReply, crate::SideQueryError>> {
        let thread = turn.thread_id.clone();
        Box::pin(async move {
            side::compact(turn, cancel)
                .await
                .map_err(|source| crate::SideQueryError::Codex {
                    thread,
                    source: Box::new(source),
                })
        })
    }

    fn run(&self, turn: Turn, io: RunIo) -> BoxFuture<'static, Outcome> {
        Box::pin(async move {
            let thread = turn.thread_id.clone();
            let conn = match server::attach(&turn).await {
                Ok(conn) => conn,
                Err(failure) => {
                    let error: &dyn std::error::Error = &failure;
                    tracing::error!(%thread, error, "Codex could not start");
                    return Outcome::failed(format!(
                        "Codex could not start: {}",
                        diagnostics::chain(&failure)
                    ));
                }
            };
            let mut run = Run {
                conn,
                decoder: Decoder::default(),
                io,
                queued: Vec::new(),
                admitted: 0,
                phase: Phase::Idle,
                steering: HashMap::new(),
                permissions: None,
            };
            let outcome = run.run(&turn).await.unwrap_or_else(|failure| {
                let error: &dyn std::error::Error = &failure;
                tracing::warn!(%thread, session = ?turn.session_id, error, "Codex run failed");
                Outcome::failed(diagnostics::chain(&failure))
            });
            let events = run.decoder.finish();
            run.emit(events).await;
            outcome
        })
    }

    fn side_query(
        &self,
        turn: Turn,
        cancel: tokio_util::sync::CancellationToken,
    ) -> BoxFuture<'static, Result<SideReply, crate::SideQueryError>> {
        let thread = turn.thread_id.clone();
        Box::pin(async move {
            side::side_query(turn, cancel)
                .await
                .map_err(|source| crate::SideQueryError::Codex {
                    thread,
                    source: Box::new(source),
                })
        })
    }

    fn clear_goal(&self, turn: Turn) -> BoxFuture<'static, Result<(), crate::ClearGoalError>> {
        let thread = turn.thread_id.clone();
        Box::pin(async move {
            goal::clear_goal(turn)
                .await
                .map_err(|source| crate::ClearGoalError::Codex {
                    thread,
                    source: Box::new(source),
                })
        })
    }
}

fn codex_home(turn: &Turn) -> std::path::PathBuf {
    turn.launch
        .env
        .iter()
        .find(|(key, _)| key == "CODEX_HOME")
        .map_or_else(|| turn.home.join(".codex"), |(_, value)| value.into())
}

async fn set_title(turn: Turn, title: String) -> Result<(), SetTitleError> {
    let session = turn
        .session_id
        .clone()
        .ok_or_else(|| SetTitleError::NoSession {
            thread: turn.thread_id.clone(),
        })?;
    let mut conn = server::attach(&turn).await.map_err(SetTitleError::Attach)?;
    let named = tokio::time::timeout(
        Duration::from_secs(30),
        goal::call(
            &mut conn,
            1,
            "thread/name/set",
            json!({ "threadId": session, "name": title }),
        ),
    )
    .await;
    match named {
        Err(_elapsed) => Err(SetTitleError::TimedOut { session }),
        Ok(Err(source)) => Err(SetTitleError::Call { session, source }),
        Ok(Ok(_)) => Ok(()),
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Admission(u64);

struct Queued {
    admission: Admission,
    input: UserInput,
    acknowledge: bool,
}

#[derive(Debug)]
struct Starting {
    request: u64,
    acknowledges: Vec<ItemId>,
}

#[derive(Debug)]
enum Phase {
    Idle,
    Starting(Starting),
    Active {
        turn: String,
    },
    Finished {
        outcome: Outcome,
        nudge: bool,
    },
    Stopping {
        deadline: Instant,
        interrupt_sent: bool,
        starting: Option<Starting>,
    },
}

struct Run {
    conn: server::Attachment,
    decoder: Decoder,
    io: RunIo,
    queued: Vec<Queued>,
    admitted: u64,
    phase: Phase,
    steering: HashMap<u64, Queued>,
    permissions: Option<Permissions>,
}

enum Step {
    Continue,
    End(Outcome),
}

impl Run {
    async fn run(&mut self, turn: &Turn) -> Result<Outcome, RunError> {
        let (method, params) = thread_params(turn, ProjectGuard::of(turn));
        match self.call(method, params).await {
            Ok(Some(_)) => {}
            Ok(None) => return Ok(Outcome::Cancelled),
            Err(RequestError::Refused(source))
                if source.message.contains(ACTIVE_WRITER)
                    && let Some(session) = turn.session_id.clone() =>
            {
                return Err(RunError::SessionBusy { session, source });
            }
            Err(source) => {
                return Err(RunError::Open {
                    method,
                    session: turn.session_id.clone(),
                    source,
                });
            }
        }
        if let Some(model) = non_empty(turn.config.model.as_deref()) {
            self.io
                .emit(EngineEvent::Model {
                    id: model.to_owned(),
                })
                .await;
        }
        self.admit(turn.input.clone(), true);
        self.pump(turn)?;
        self.drive(turn).await
    }

    async fn call(&mut self, method: &str, params: Value) -> Result<Option<Value>, RequestError> {
        let id = self.decoder.request(method, params);
        self.send_outbox().map_err(RequestError::Write)?;
        loop {
            let line = tokio::select! {
                line = self.conn.next_line() => line.map_err(RequestError::Overloaded)?,
                () = self.io.cancel.cancelled() => return Ok(None),
            };
            let Some(line) = line else {
                return Err(RequestError::Exited {
                    ended: self.conn.ended(),
                });
            };
            let events = self.decoder.feed(&line).map_err(RequestError::Decode)?;
            self.emit(events).await;
            self.send_outbox().map_err(RequestError::Write)?;
            if let Some(response) = self
                .decoder
                .take_responses()
                .into_iter()
                .find(|response| response.id == id)
            {
                return response.result.map(Some).map_err(RequestError::Refused);
            }
        }
    }

    async fn drive(&mut self, turn: &Turn) -> Result<Outcome, RunError> {
        let mut inbox_open = true;
        loop {
            let deadline = match &self.phase {
                Phase::Stopping { deadline, .. } => Some(*deadline),
                Phase::Idle
                | Phase::Starting(_)
                | Phase::Active { .. }
                | Phase::Finished { .. } => None,
            };
            let stopping = deadline.is_some();
            let step = tokio::select! {
                line = self.conn.next_line() => match line.map_err(|source| RunError::Overloaded { during: "the turn", source })? {
                    Some(line) => self.line(turn, &line).await?,
                    None if stopping => Step::End(Outcome::Cancelled),
                    None => return Err(RunError::Exited { during: "the turn", ended: self.conn.ended() }),
                },
                input = self.io.inbox.recv(), if inbox_open && !stopping => {
                    match input {
                        Some(input) => {
                            self.input(input).await?;
                            self.pump(turn)?;
                        }
                        None => inbox_open = false,
                    }
                    Step::Continue
                }
                () = self.io.cancel.cancelled(), if !stopping => self.stop()?,
                () = tokio::time::sleep_until(deadline.unwrap_or_else(Instant::now)), if stopping => {
                    if let Some(thread) = self.decoder.thread() {
                        self.decoder.request("thread/unsubscribe", json!({ "threadId": thread }));
                        self.flush()?;
                    }
                    self.conn.retire();
                    let interrupt_sent = matches!(self.phase, Phase::Stopping { interrupt_sent: true, .. });
                    return Err(RunError::StopTimedOut { interrupt_sent });
                }
            };
            if let Step::End(outcome) = step {
                return Ok(outcome);
            }
        }
    }

    async fn line(&mut self, turn: &Turn, line: &str) -> Result<Step, RunError> {
        let events = self.decoder.feed(line).map_err(RunError::Decode)?;
        self.emit(events).await;
        self.flush()?;
        self.follow_turn();
        for response in self.decoder.take_responses() {
            self.responded(response).await?;
        }
        if let Some(outcome) = self.decoder.take_turn_end() {
            if matches!(self.phase, Phase::Stopping { .. }) {
                return Ok(Step::End(Outcome::Cancelled));
            }
            if let Phase::Starting(starting) = std::mem::replace(&mut self.phase, Phase::Idle) {
                self.acknowledge(starting.acknowledges).await;
            }
            let nudge = self.decoder.take_stranded();
            if nudge {
                tracing::debug!(thread = %turn.thread_id, "Codex ended the turn without answering a steer; starting another");
            }
            self.phase = Phase::Finished { outcome, nudge };
        }
        for text in self.decoder.take_messages() {
            self.admit(
                UserInput {
                    id: ItemId::default(),
                    text,
                    images: Vec::new(),
                },
                false,
            );
        }
        if self.idle() {
            while let Ok(input) = self.io.inbox.try_recv() {
                self.input(input).await?;
            }
        }
        self.pump(turn)?;
        if let Phase::Stopping {
            interrupt_sent: false,
            starting,
            ..
        } = &self.phase
        {
            let starting = starting.is_some();
            if self.interrupt()? {
                if let Phase::Stopping {
                    interrupt_sent,
                    deadline,
                    ..
                } = &mut self.phase
                {
                    *interrupt_sent = true;
                    *deadline = Instant::now() + INTERRUPT_GRACE;
                }
            } else if !starting && self.steering.is_empty() {
                return Ok(Step::End(Outcome::Cancelled));
            }
        }
        if self.queued.is_empty()
            && self.steering.is_empty()
            && matches!(self.phase, Phase::Finished { nudge: false, .. })
            && let Phase::Finished { outcome, .. } = std::mem::replace(&mut self.phase, Phase::Idle)
        {
            return Ok(Step::End(outcome));
        }
        Ok(Step::Continue)
    }

    fn follow_turn(&mut self) {
        let Some(running) = self.decoder.active_turn() else {
            return;
        };
        match &mut self.phase {
            Phase::Idle | Phase::Finished { .. } => {
                self.phase = Phase::Active {
                    turn: running.to_owned(),
                };
            }
            Phase::Active { turn } if turn != running => running.clone_into(turn),
            Phase::Active { .. } | Phase::Starting(_) | Phase::Stopping { .. } => {}
        }
    }

    fn stop(&mut self) -> Result<Step, RunError> {
        let deadline = Instant::now() + INTERRUPT_GRACE;
        let interrupt_sent = self.interrupt()?;
        let starting = match std::mem::replace(&mut self.phase, Phase::Idle) {
            Phase::Starting(starting) => Some(starting),
            Phase::Idle
            | Phase::Active { .. }
            | Phase::Finished { .. }
            | Phase::Stopping { .. } => None,
        };
        if !interrupt_sent && starting.is_none() && self.steering.is_empty() {
            return Ok(Step::End(Outcome::Cancelled));
        }
        self.phase = Phase::Stopping {
            deadline,
            interrupt_sent,
            starting,
        };
        Ok(Step::Continue)
    }

    fn interrupt(&mut self) -> Result<bool, RunError> {
        let (Some(thread), Some(turn_id)) = (self.decoder.thread(), self.decoder.active_turn())
        else {
            return Ok(false);
        };
        self.decoder.request(
            "turn/interrupt",
            json!({ "threadId": thread, "turnId": turn_id }),
        );
        self.flush()?;
        Ok(true)
    }

    fn admit(&mut self, input: UserInput, acknowledge: bool) {
        self.admitted += 1;
        self.queued.push(Queued {
            admission: Admission(self.admitted),
            input,
            acknowledge,
        });
    }

    fn requeue(&mut self, queued: Queued) {
        let at = self
            .queued
            .partition_point(|earlier| earlier.admission < queued.admission);
        self.queued.insert(at, queued);
    }

    fn idle(&self) -> bool {
        matches!(self.phase, Phase::Idle | Phase::Finished { .. }) && self.steering.is_empty()
    }

    async fn input(&mut self, input: Input) -> Result<(), RunError> {
        match input {
            Input::Steer(input) => self.admit(input, true),
            Input::Answer { key, answer } => {
                let events = self.decoder.answer(&key, &answer);
                self.emit(events).await;
                self.flush()?;
            }
            Input::SetPermissions(permissions) => self.permissions = Some(permissions),
            Input::ClearGoal => {
                let thread = self.decoder.thread().ok_or(RunError::NoThread)?.to_owned();
                self.decoder
                    .request("thread/goal/clear", json!({ "threadId": thread }));
                self.flush()?;
            }
        }
        Ok(())
    }

    fn pump(&mut self, turn: &Turn) -> Result<(), RunError> {
        let nudge = match &self.phase {
            Phase::Starting(_) | Phase::Stopping { .. } => return Ok(()),
            Phase::Finished { nudge, .. } => *nudge,
            Phase::Idle | Phase::Active { .. } => false,
        };
        if self.queued.is_empty() && !nudge {
            return Ok(());
        }
        let thread = self.decoder.thread().ok_or(RunError::NoThread)?.to_owned();
        if let Phase::Active { turn: turn_id } = &self.phase {
            for queued in std::mem::take(&mut self.queued) {
                let params = json!({ "threadId": thread, "expectedTurnId": turn_id, "input": user_input(&queued.input) });
                let id = self.decoder.request("turn/steer", params);
                self.steering.insert(id, queued);
            }
        } else if self.steering.is_empty() {
            let queued = std::mem::take(&mut self.queued);
            let input: Vec<Value> = queued
                .iter()
                .flat_map(|queued| user_input(&queued.input))
                .collect();
            let mut params = json!({ "threadId": thread, "input": input });
            let guard = ProjectGuard::of(turn);
            turn_overrides(turn, guard, &mut params);
            if let Some(permissions) = self.permissions {
                access_overrides(permissions, guard, &mut params);
            }
            let request = self.decoder.request("turn/start", params);
            let acknowledges = queued
                .into_iter()
                .filter(|queued| queued.acknowledge)
                .map(|queued| queued.input.id)
                .collect();
            self.phase = Phase::Starting(Starting {
                request,
                acknowledges,
            });
        }
        self.flush()
    }

    async fn acknowledge(&self, inputs: Vec<ItemId>) {
        for id in inputs {
            self.io.emit(EngineEvent::InputAccepted { id }).await;
        }
    }

    async fn responded(&mut self, response: decoder::Response) -> Result<(), RunError> {
        match response.method.as_str() {
            "turn/start" => {
                let starting = match &mut self.phase {
                    Phase::Starting(starting) if starting.request == response.id => {
                        Some(std::mem::take(&mut starting.acknowledges))
                    }
                    Phase::Stopping {
                        starting: starting @ Some(_),
                        ..
                    } if starting
                        .as_ref()
                        .is_some_and(|starting| starting.request == response.id) =>
                    {
                        starting.take().map(|starting| starting.acknowledges)
                    }
                    Phase::Idle
                    | Phase::Starting(_)
                    | Phase::Active { .. }
                    | Phase::Finished { .. }
                    | Phase::Stopping { .. } => None,
                };
                let Some(acknowledges) = starting else {
                    return Ok(());
                };
                response.result.map_err(RunError::TurnStart)?;
                if matches!(self.phase, Phase::Starting(_)) {
                    let turn = self
                        .decoder
                        .active_turn()
                        .ok_or(RunError::NoTurn)?
                        .to_owned();
                    self.phase = Phase::Active { turn };
                }
                self.acknowledge(acknowledges).await;
            }
            "turn/steer" => {
                let Some(queued) = self.steering.remove(&response.id) else {
                    return Ok(());
                };
                match response.result {
                    Ok(_) => {
                        if queued.acknowledge {
                            self.acknowledge(vec![queued.input.id]).await;
                        }
                    }
                    Err(error) => {
                        tracing::debug!(item = %queued.input.id, %error, "Codex takes a rejected steer as the next turn");
                        self.requeue(queued);
                    }
                }
            }
            "turn/interrupt" => {
                if let Err(error) = response.result {
                    tracing::debug!(%error, "Codex refused turn/interrupt");
                }
            }
            "thread/goal/clear" => {
                if let Err(error) = response.result {
                    tracing::warn!(%error, "Codex refused thread/goal/clear");
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<(), RunError> {
        self.send_outbox().map_err(RunError::Write)
    }

    fn send_outbox(&mut self) -> Result<(), Ended> {
        for message in self.decoder.take_outbox() {
            self.conn.send(message)?;
        }
        Ok(())
    }

    async fn emit(&self, events: Vec<EngineEvent>) {
        for event in events {
            self.io.emit(event).await;
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum ProjectGuard {
    Codex,
    Forge,
}

impl ProjectGuard {
    fn of(turn: &Turn) -> Self {
        if crate::sandbox::forge_confines(turn.config.permissions, forge_isolation::support()) {
            Self::Forge
        } else {
            Self::Codex
        }
    }
}

fn thread_params(turn: &Turn, guard: ProjectGuard) -> (&'static str, Value) {
    let config = &turn.config;
    let mut settings = Map::new();
    settings.insert("model_reasoning_summary".to_owned(), json!("auto"));
    if let Some(effort) = non_empty(config.effort.as_deref()) {
        settings.insert("model_reasoning_effort".to_owned(), json!(effort));
    }
    if let Some(tokens) = config.context_window_override() {
        settings.insert("model_context_window".to_owned(), json!(tokens));
    }
    for (name, url) in &turn.launch.gateway_mcp {
        let server = format!("mcp_servers.{name}");
        settings.insert(format!("{server}.url"), json!(url));
        settings.insert(format!("{server}.tool_timeout_sec"), json!(600));
    }
    let mut params = json!({
        "cwd": turn.cwd,
        "config": settings,
    });
    match (config.permissions, guard) {
        (Permissions::Auto, ProjectGuard::Codex) => {
            params["approvalPolicy"] = json!("on-request");
            params["approvalsReviewer"] = json!("auto_review");
            params["sandbox"] = json!("workspace-write");
        }
        (Permissions::Auto, ProjectGuard::Forge) => {
            params["approvalPolicy"] = json!("on-request");
            params["approvalsReviewer"] = json!("auto_review");
            params["sandbox"] = json!("danger-full-access");
        }
        (Permissions::Full, _) => {
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
            params["excludeTurns"] = json!(true);
            ("thread/resume", params)
        }
        None => ("thread/start", params),
    }
}

fn turn_overrides(turn: &Turn, guard: ProjectGuard, params: &mut Value) {
    let config = &turn.config;
    params["cwd"] = json!(turn.cwd);
    if let Some(model) = non_empty(config.model.as_deref()) {
        params["model"] = json!(model);
    }
    if let Some(effort) = non_empty(config.effort.as_deref()) {
        params["effort"] = json!(effort);
    }
    params["serviceTierForTurn"] = json!(service_tier(config).unwrap_or("default"));
    access_overrides(config.permissions, guard, params);
}

fn access_overrides(permissions: Permissions, guard: ProjectGuard, params: &mut Value) {
    match (permissions, guard) {
        (Permissions::Auto, ProjectGuard::Codex) => {
            params["approvalPolicy"] = json!("on-request");
            params["approvalsReviewer"] = json!("auto_review");
            params["sandboxPolicy"] = json!({ "type": "workspaceWrite" });
        }
        (Permissions::Auto, ProjectGuard::Forge) => {
            params["approvalPolicy"] = json!("on-request");
            params["approvalsReviewer"] = json!("auto_review");
            params["sandboxPolicy"] = json!({ "type": "dangerFullAccess" });
        }
        (Permissions::Full, _) => {
            params["approvalPolicy"] = json!("never");
            params["sandboxPolicy"] = json!({ "type": "dangerFullAccess" });
        }
    }
}

fn service_tier(config: &EngineConfig) -> Option<&str> {
    non_empty(config.speed.as_deref()).filter(|speed| *speed != crate::catalog::STANDARD_SPEED)
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.filter(|value| !value.is_empty())
}

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
