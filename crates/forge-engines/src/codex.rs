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

pub(crate) use decoder::DecodeError;
pub use goal::{CallError, ClearGoalError};
pub use server::{AttachError, Ended, InitializeError, ReleaseError, StartError};
pub use side::SideQueryError;

use crate::{EngineEvent, Harness, Input, Outcome, RunIo, SideReply, Turn, UserInput};
use artisan_api::{EngineConfig, ItemId, Permissions, ThreadId};
use decoder::Decoder;
use futures::future::BoxFuture;
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::time::Instant;

const INTERRUPT_GRACE: Duration = Duration::from_secs(5);
const IMAGE_BUDGET: u32 = 128 * 1024 * 1024;
pub const MAX_TURN_IMAGE_BYTES: usize = IMAGE_BUDGET as usize;
const MAX_QUEUED: usize = 32;
const MAX_LINE_BYTES: usize = MAX_TURN_IMAGE_BYTES.div_ceil(3) * 4 + crate::process::MAX_LINE_BYTES;

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
    #[error("attach to a fresh Codex app-server after the cached one ended")]
    Reattach(#[source] AttachError),
    #[error("Codex exited during {during}{}", if ended.is_empty() { String::new() } else { format!(": {ended}") })]
    Exited { during: &'static str, ended: String },
    #[error("decode Codex output")]
    Decode(#[source] DecodeError),
    #[error("write to Codex")]
    Write(#[source] Ended),
    #[error("Codex refused turn/start")]
    TurnStart(#[source] crate::rpc::RpcError),
    #[error("Codex refused thread/compact/start")]
    CompactStart(#[source] crate::rpc::RpcError),
    #[error("Codex answered turn/start without a turn id")]
    NoTurn,
    #[error("Codex has no thread for this run")]
    NoThread,
    #[error("read the image {} of message {item}", path.display())]
    Image {
        item: ItemId,
        path: std::path::PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("read the images of message {item}")]
    ImageTask {
        item: ItemId,
        #[source]
        source: tokio::task::JoinError,
    },
    #[error("{}; requested thread unload and retired the server. Underlying work may still be running.", if *interrupt_sent { "Codex did not confirm the turn stopped after an interrupt" } else { "Codex did not return the started turn ID before cancellation timed out" })]
    StopTimedOut { interrupt_sent: bool },
}

#[derive(Debug, thiserror::Error)]
pub enum RequestError {
    #[error("Codex exited{}", if ended.is_empty() { String::new() } else { format!(": {ended}") })]
    Exited { ended: String },
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
                        crate::rpc::told(&failure)
                    ));
                }
            };
            let mut run = Run {
                conn,
                decoder: Decoder::default(),
                io,
                queued: Vec::new(),
                intake: Intake::Open,
                budget: Budget::default(),
                admitted: 0,
                phase: Phase::Idle,
                steering: HashMap::new(),
                permissions: None,
            };
            let outcome = run.run(&turn).await.unwrap_or_else(|failure| {
                let error: &dyn std::error::Error = &failure;
                tracing::warn!(%thread, session = ?turn.session_id, error, "Codex run failed");
                Outcome::failed(crate::rpc::told(&failure))
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

struct Budget(Arc<Semaphore>);

impl Default for Budget {
    fn default() -> Self {
        Self(Arc::new(Semaphore::new(MAX_TURN_IMAGE_BYTES)))
    }
}

impl Budget {
    fn charge(image_bytes: usize) -> u32 {
        u32::try_from(image_bytes).map_or(IMAGE_BUDGET, |bytes| bytes.min(IMAGE_BUDGET))
    }

    fn try_reserve(&self, image_bytes: usize) -> Option<Reserved> {
        Arc::clone(&self.0)
            .try_acquire_many_owned(Self::charge(image_bytes))
            .ok()
            .map(|permit| Reserved {
                _permit: Arc::new(permit),
            })
    }

    async fn reserve(&self, image_bytes: usize) -> Reserved {
        let permit = Arc::clone(&self.0)
            .acquire_many_owned(Self::charge(image_bytes))
            .await
            .expect("invariant: a run never closes its image budget");
        Reserved {
            _permit: Arc::new(permit),
        }
    }
}

#[derive(Clone)]
struct Reserved {
    _permit: Arc<OwnedSemaphorePermit>,
}

struct Queued {
    admission: Admission,
    input: UserInput,
    image_bytes: usize,
    reserved: Reserved,
    acknowledge: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Intake {
    Open,
    Holding,
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
    Compacting(Starting),
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
    intake: Intake,
    budget: Budget,
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
        let (method, params) = thread_params(turn);
        let mut opened = self.call(method, params.clone()).await;
        if let Err(RequestError::Exited { .. } | RequestError::Write(_)) = opened {
            tracing::info!(
                thread = %turn.thread_id,
                "the Codex app-server ended before it opened the session; opening it on a fresh server"
            );
            self.conn = server::attach(turn).await.map_err(RunError::Reattach)?;
            self.decoder = Decoder::default();
            opened = self.call(method, params).await;
        }
        match opened {
            Ok(Some(_)) => {}
            Ok(None) => return Ok(Outcome::Cancelled),
            Err(RequestError::Refused(source))
                if source
                    .said()
                    .is_some_and(|said| said.contains(ACTIVE_WRITER))
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
        self.admit(turn.input.clone(), true).await?;
        self.pump(turn).await?;
        self.drive(turn).await
    }

    async fn call(&mut self, method: &str, params: Value) -> Result<Option<Value>, RequestError> {
        let id = self.decoder.request(method, params);
        self.send_outbox(&[]).map_err(RequestError::Write)?;
        loop {
            let line = tokio::select! {
                line = self.conn.next_line() => line,
                () = self.io.cancel.cancelled() => return Ok(None),
            };
            let Some(line) = line else {
                return Err(RequestError::Exited {
                    ended: self.conn.ended(),
                });
            };
            let events = self.decoder.feed(&line).map_err(RequestError::Decode)?;
            self.emit(events).await;
            self.send_outbox(&[]).map_err(RequestError::Write)?;
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
                | Phase::Compacting(_)
                | Phase::Active { .. }
                | Phase::Finished { .. } => None,
            };
            let stopping = deadline.is_some();
            let step = tokio::select! {
                line = self.conn.next_line() => match line {
                    Some(line) => self.line(turn, &line).await?,
                    None if stopping => Step::End(Outcome::Cancelled),
                    None => return Err(RunError::Exited { during: "the turn", ended: self.conn.ended() }),
                },
                input = self.io.inbox.recv(), if inbox_open && !stopping => {
                    match input {
                        Some(input) => {
                            self.input(input).await?;
                            self.pump(turn).await?;
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
            if let Phase::Starting(starting) | Phase::Compacting(starting) =
                std::mem::replace(&mut self.phase, Phase::Idle)
            {
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
            )
            .await?;
        }
        if self.idle() {
            while let Ok(input) = self.io.inbox.try_recv() {
                self.input(input).await?;
            }
        }
        self.pump(turn).await?;
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
            Phase::Compacting(starting)
                if starting.acknowledges.is_empty() && starting.request == 0 =>
            {
                self.phase = Phase::Active {
                    turn: running.to_owned(),
                };
            }
            Phase::Active { turn } if turn != running => running.clone_into(turn),
            Phase::Active { .. }
            | Phase::Starting(_)
            | Phase::Compacting(_)
            | Phase::Stopping { .. } => {}
        }
    }

    fn stop(&mut self) -> Result<Step, RunError> {
        let deadline = Instant::now() + INTERRUPT_GRACE;
        let interrupt_sent = self.interrupt()?;
        let starting = match std::mem::replace(&mut self.phase, Phase::Idle) {
            Phase::Starting(starting) | Phase::Compacting(starting) => Some(starting),
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

    async fn admit(&mut self, input: UserInput, acknowledge: bool) -> Result<(), RunError> {
        let image_bytes = image_bytes(&input).await?;
        let reserved = self.budget.reserve(image_bytes).await;
        self.enqueue(input, image_bytes, reserved, acknowledge);
        Ok(())
    }

    fn enqueue(
        &mut self,
        input: UserInput,
        image_bytes: usize,
        reserved: Reserved,
        acknowledge: bool,
    ) {
        self.admitted += 1;
        self.queued.push(Queued {
            admission: Admission(self.admitted),
            input,
            image_bytes,
            reserved,
            acknowledge,
        });
    }

    async fn steer(&mut self, input: UserInput) -> Result<(), RunError> {
        let image_bytes = image_bytes(&input).await?;
        let outstanding = self.queued.len() + self.steering.len();
        if let Some(reserved) = intake(self.intake, outstanding, &self.budget, image_bytes) {
            self.enqueue(input, image_bytes, reserved, true);
        } else {
            self.intake = Intake::Holding;
            tracing::info!(item = %input.id, outstanding, "Codex's queue is full; the steer waits for the next run");
        }
        Ok(())
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
            Input::Steer(input) => self.steer(input).await?,
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

    async fn pump(&mut self, turn: &Turn) -> Result<(), RunError> {
        let nudge = match &self.phase {
            Phase::Starting(_) | Phase::Compacting(_) | Phase::Stopping { .. } => return Ok(()),
            Phase::Finished { nudge, .. } => *nudge,
            Phase::Idle | Phase::Active { .. } => false,
        };
        if self.queued.is_empty() && !nudge {
            return Ok(());
        }
        let thread = self.decoder.thread().ok_or(RunError::NoThread)?.to_owned();
        if let Phase::Active { turn: turn_id } = &self.phase {
            let turn_id = turn_id.clone();
            for queued in std::mem::take(&mut self.queued) {
                let input = user_input(std::slice::from_ref(&queued.input)).await?;
                let params =
                    json!({ "threadId": thread, "expectedTurnId": turn_id, "input": input });
                let id = self.decoder.request("turn/steer", params);
                self.flush_holding(std::slice::from_ref(&queued.reserved))?;
                self.steering.insert(id, queued);
            }
            self.flush()
        } else if self.steering.is_empty() {
            if let Some(rest) = self
                .queued
                .first()
                .and_then(|queued| crate::commands::compact_argument(&queued.input.text))
                .map(str::to_owned)
            {
                let first = self.queued.remove(0);
                if !rest.is_empty() || !first.input.images.is_empty() {
                    self.queued.insert(
                        0,
                        Queued {
                            admission: first.admission,
                            input: UserInput {
                                id: first.input.id.clone(),
                                text: rest,
                                images: first.input.images,
                            },
                            image_bytes: first.image_bytes,
                            reserved: first.reserved,
                            acknowledge: false,
                        },
                    );
                }
                if let Some(event) = self.decoder.start_compaction() {
                    self.io.emit(event).await;
                }
                let request = self
                    .decoder
                    .request("thread/compact/start", json!({ "threadId": thread }));
                let acknowledges = first
                    .acknowledge
                    .then_some(first.input.id)
                    .into_iter()
                    .collect();
                self.phase = Phase::Compacting(Starting {
                    request,
                    acknowledges,
                });
                return self.flush();
            }
            let later = self.queued.split_off(within_turn_images(&self.queued));
            let queued = std::mem::replace(&mut self.queued, later);
            let inputs: Vec<UserInput> = queued.iter().map(|queued| queued.input.clone()).collect();
            let input = user_input(&inputs).await?;
            let mut params = json!({ "threadId": thread, "input": input });
            turn_overrides(turn, &mut params);
            if let Some(permissions) = self.permissions {
                access_overrides(permissions, &mut params);
            }
            let request = self.decoder.request("turn/start", params);
            let held: Vec<Reserved> = queued
                .iter()
                .map(|queued| queued.reserved.clone())
                .collect();
            let acknowledges = queued
                .into_iter()
                .filter(|queued| queued.acknowledge)
                .map(|queued| queued.input.id)
                .collect();
            self.phase = Phase::Starting(Starting {
                request,
                acknowledges,
            });
            self.flush_holding(&held)
        } else {
            self.flush()
        }
    }

    async fn acknowledge(&self, inputs: Vec<ItemId>) {
        for id in inputs {
            self.io.emit(EngineEvent::InputAccepted { id }).await;
        }
    }

    async fn compact_started(&mut self, response: decoder::Response) -> Result<(), RunError> {
        let compacting = match &mut self.phase {
            Phase::Compacting(starting) if starting.request == response.id => {
                starting.request = 0;
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
            | Phase::Compacting(_)
            | Phase::Active { .. }
            | Phase::Finished { .. }
            | Phase::Stopping { .. } => None,
        };
        let Some(acknowledges) = compacting else {
            return Ok(());
        };
        if let Err(source) = response.result {
            if let Some(event) = self.decoder.fail_compaction() {
                self.io.emit(event).await;
            }
            return Err(RunError::CompactStart(source));
        }
        if matches!(self.phase, Phase::Compacting(_))
            && let Some(turn) = self.decoder.active_turn()
        {
            self.phase = Phase::Active {
                turn: turn.to_owned(),
            };
        }
        self.acknowledge(acknowledges).await;
        Ok(())
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
                    | Phase::Compacting(_)
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
            "thread/compact/start" => self.compact_started(response).await?,
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
        self.flush_holding(&[])
    }

    fn flush_holding(&mut self, held: &[Reserved]) -> Result<(), RunError> {
        self.send_outbox(held).map_err(RunError::Write)
    }

    fn send_outbox(&mut self, held: &[Reserved]) -> Result<(), Ended> {
        for message in self.decoder.take_outbox() {
            self.conn.send_holding(message, held.to_vec())?;
        }
        Ok(())
    }

    async fn emit(&self, events: Vec<EngineEvent>) {
        for event in events {
            self.io.emit(event).await;
        }
    }
}

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
    for (name, url) in &turn.launch.gateway_mcp {
        let server = format!("mcp_servers.{name}");
        settings.insert(format!("{server}.url"), json!(url));
        settings.insert(format!("{server}.tool_timeout_sec"), json!(600));
    }
    let mut params = json!({
        "cwd": turn.cwd,
        "config": settings,
    });
    match config.permissions {
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
            params["excludeTurns"] = json!(true);
            ("thread/resume", params)
        }
        None => ("thread/start", params),
    }
}

fn turn_overrides(turn: &Turn, params: &mut Value) {
    let config = &turn.config;
    params["cwd"] = json!(turn.cwd);
    if let Some(model) = non_empty(config.model.as_deref()) {
        params["model"] = json!(model);
    }
    if let Some(effort) = non_empty(config.effort.as_deref()) {
        params["effort"] = json!(effort);
    }
    params["serviceTierForTurn"] = json!(service_tier(config).unwrap_or("default"));
    access_overrides(config.permissions, params);
}

fn access_overrides(permissions: Permissions, params: &mut Value) {
    match permissions {
        Permissions::Auto => {
            params["approvalPolicy"] = json!("on-request");
            params["approvalsReviewer"] = json!("auto_review");
            params["sandboxPolicy"] = json!({ "type": "workspaceWrite" });
        }
        Permissions::Full => {
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

fn within_turn_images(queued: &[Queued]) -> usize {
    let mut total = 0_usize;
    let fitting = queued
        .iter()
        .take_while(|queued| {
            total = total.saturating_add(queued.image_bytes);
            total <= MAX_TURN_IMAGE_BYTES
        })
        .count();
    fitting.max(1).min(queued.len())
}

fn intake(
    intake: Intake,
    outstanding: usize,
    budget: &Budget,
    image_bytes: usize,
) -> Option<Reserved> {
    match intake {
        Intake::Open if outstanding < MAX_QUEUED => budget.try_reserve(image_bytes),
        Intake::Open | Intake::Holding => None,
    }
}

async fn image_bytes(input: &UserInput) -> Result<usize, RunError> {
    let mut total = 0_usize;
    for path in &input.images {
        let size = tokio::fs::metadata(path)
            .await
            .map_err(|source| RunError::Image {
                item: input.id.clone(),
                path: path.clone(),
                source,
            })?
            .len();
        total = total.saturating_add(usize::try_from(size).unwrap_or(usize::MAX));
    }
    Ok(total)
}

async fn user_input(inputs: &[UserInput]) -> Result<Vec<Value>, RunError> {
    let Some(first) = inputs.iter().find(|input| !input.images.is_empty()) else {
        return Ok(inputs.iter().filter_map(text_item).collect());
    };
    let item = first.id.clone();
    let owned = inputs.to_vec();
    tokio::task::spawn_blocking(move || {
        owned.iter().try_fold(Vec::new(), |mut items, input| {
            items.extend(text_item(input));
            items.extend(inline_images(&input.id, &input.images)?);
            Ok::<_, RunError>(items)
        })
    })
    .await
    .map_err(|source| RunError::ImageTask { item, source })?
}

fn text_item(input: &UserInput) -> Option<Value> {
    (!input.text.is_empty() || input.images.is_empty())
        .then(|| json!({ "type": "text", "text": input.text, "text_elements": [] }))
}

fn inline_images(item: &ItemId, paths: &[std::path::PathBuf]) -> Result<Vec<Value>, RunError> {
    use base64::Engine as _;
    paths
        .iter()
        .map(|path| {
            let bytes = std::fs::read(path).map_err(|source| RunError::Image {
                item: item.clone(),
                path: path.clone(),
                source,
            })?;
            let url = format!(
                "data:image/webp;base64,{}",
                base64::engine::general_purpose::STANDARD.encode(bytes)
            );
            Ok(json!({ "type": "image", "url": url }))
        })
        .collect()
}
