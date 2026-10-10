//! The mutation lane: composer draft work and sends run beside the command
//! loop, so navigation never waits for a durable write.
//!
//! A draft save, an image upload, or a send is one Forge write, and a write
//! can stall for seconds when the disk is contended. Run on the command
//! loop, every later command (`Unsubscribe`, `Subscribe`, `ReadActiveRun`,
//! the next thread's snapshot) queued behind it, and a thread switch stalled
//! until the write finished. The Forge answers a connection's requests
//! concurrently, so the lane sends these on the same session through a
//! [`SessionRequester`], each in its own task, while the command loop keeps
//! serving everything else.
//!
//! Order is kept where it matters. Jobs of one draft scope run one at a
//! time in the order they were queued, so a scope's saves apply in order and
//! a send is issued only after the save that stored its body. Subscribe,
//! Unsubscribe, and conversation deliveries never enter the lane and keep
//! their order on the command loop. Results publish on the same events
//! channel as every other answer.
//!
//! The lane never connects. A job that finds its connection gone asks the
//! command loop for a session ([`SessionAsk`]); the loop reconnects only when
//! the failed connection is still its current one, so several jobs that
//! lose the same connection cause one reconnect. Draft writes are idempotent
//! stable mutations, so a write lost with its connection is sent once more
//! under the same identity, exactly as on the command loop.

#![forbid(unsafe_code)]

use std::collections::{HashMap, VecDeque};

use artisan_domain::ComposerDraftScope;
use artisan_protocol::ConnectionId;
use artisan_transport::SessionRequester;
use tokio::sync::{mpsc, oneshot};
use tokio::task::{Id as TaskId, JoinSet};

use super::request_delivery::validate_resolved_request;
use super::*;

/// One unit of lane work; each belongs to one draft scope.
pub(super) enum LaneJob {
    /// A draft save, read, upload, or attachment read.
    Draft(ComposerDraftCommand),
    /// A send of a stored draft revision.
    Submit(SubmitComposerDraft),
}

impl LaneJob {
    fn scope(&self) -> ComposerDraftScope {
        match self {
            Self::Draft(ComposerDraftCommand::Save { command, .. }) => command.scope().clone(),
            Self::Draft(
                ComposerDraftCommand::Read(scope)
                | ComposerDraftCommand::Upload { scope, .. }
                | ComposerDraftCommand::ReadAttachment { scope, .. },
            ) => scope.clone(),
            Self::Submit(command) => command.scope.clone(),
        }
    }
}

/// A job waiting for its scope's turn, with the connection hold it was
/// queued under; the hold drops when the job ends.
struct QueuedJob {
    job: LaneJob,
    hold: Option<Hold>,
}

/// A lane job's request for a usable session.
pub(super) struct SessionAsk {
    /// The connection the job lost, if it had one.
    failed: Option<ConnectionId>,
    /// Whether the failed connection must be replaced even if it still
    /// looks open (the durable-write retry rule).
    replace: bool,
    reply: oneshot::Sender<Result<SessionRequester, ServiceFailure>>,
}

/// What the lane has for the command loop.
pub(super) enum LaneEvent {
    /// A job ended.
    Finished(Result<(TaskId, Result<(), ServiceFailure>), tokio::task::JoinError>),
    /// A job needs a session.
    Ask(SessionAsk),
}

/// The jobs in flight and waiting, per scope.
pub(super) struct MutationLane {
    /// Scopes with a running job, each with the jobs queued behind it.
    scopes: HashMap<ComposerDraftScope, VecDeque<QueuedJob>>,
    running: JoinSet<Result<(), ServiceFailure>>,
    running_scopes: HashMap<TaskId, ComposerDraftScope>,
    asks: mpsc::Sender<SessionAsk>,
    asked: mpsc::Receiver<SessionAsk>,
    frames: FrameFactory,
    events: SyncSender<NativeTransportEvent>,
    /// Ends every job's wait when the lane is dropped.
    cancel: Arc<CancelHandle>,
}

impl MutationLane {
    pub(super) fn new(frames: &FrameFactory, events: &SyncSender<NativeTransportEvent>) -> Self {
        let (asks, asked) = mpsc::channel(artisan_transport::CONCURRENT_REQUEST_CAPACITY);
        Self {
            scopes: HashMap::new(),
            running: JoinSet::new(),
            running_scopes: HashMap::new(),
            asks,
            asked,
            frames: frames.clone(),
            events: events.clone(),
            cancel: Arc::new(CancelHandle::new()),
        }
    }

    /// Whether no job runs or waits.
    pub(super) fn is_idle(&self) -> bool {
        self.scopes.is_empty()
    }

    /// Queues one job behind its scope's earlier ones, starting it at once
    /// when the scope is free. `session` is the command loop's current one.
    pub(super) fn submit(
        &mut self,
        job: LaneJob,
        hold: Option<Hold>,
        session: Option<&ClientSession>,
    ) {
        let scope = job.scope();
        let queued = QueuedJob { job, hold };
        if let Some(waiting) = self.scopes.get_mut(&scope) {
            waiting.push_back(queued);
            return;
        }
        self.scopes.insert(scope.clone(), VecDeque::new());
        self.start(scope, queued, session.map(ClientSession::requester));
    }

    fn start(
        &mut self,
        scope: ComposerDraftScope,
        queued: QueuedJob,
        requester: Option<SessionRequester>,
    ) {
        let mut requests = LaneRequests {
            requester,
            frames: self.frames.clone(),
            asks: self.asks.clone(),
            cancel: Arc::clone(&self.cancel),
        };
        let events = self.events.clone();
        let QueuedJob { job, hold } = queued;
        let task = self.running.spawn(async move {
            let _hold = hold;
            match job {
                LaneJob::Draft(command) => {
                    composer_draft_operations::handle_composer_draft_command(
                        &mut requests,
                        &events,
                        command,
                    )
                    .await
                }
                LaneJob::Submit(command) => {
                    handlers::submit_composer_draft(&mut requests, &events, command).await
                }
            }
        });
        self.running_scopes.insert(task.id(), scope);
    }

    /// Waits for the next job to end or ask for a session; pending forever
    /// while the lane is idle.
    pub(super) async fn next(&mut self) -> LaneEvent {
        tokio::select! {
            Some(finished) = self.running.join_next_with_id(), if !self.running.is_empty() => {
                LaneEvent::Finished(finished)
            }
            Some(ask) = self.asked.recv() => LaneEvent::Ask(ask),
        }
    }

    /// Handles one lane event on the command loop: answers an ask with a
    /// session, or starts the next job of a scope whose job ended.
    ///
    /// # Errors
    ///
    /// Returns the failure that ends the service: a closed event bridge.
    pub(super) async fn handle(
        &mut self,
        event: LaneEvent,
        runtime: &mut ServiceRuntime,
        frames: &mut FrameFactory,
    ) -> Result<(), ServiceFailure> {
        match event {
            LaneEvent::Ask(ask) => {
                let _ = ask
                    .reply
                    .send(lane_session(runtime, frames, ask.failed, ask.replace).await);
                Ok(())
            }
            LaneEvent::Finished(finished) => {
                let (task, outcome) = match finished {
                    Ok((task, outcome)) => (task, outcome),
                    Err(error) if error.is_panic() => std::panic::resume_unwind(error.into_panic()),
                    Err(error) => (error.id(), Ok(())),
                };
                if let Some(scope) = self.running_scopes.remove(&task) {
                    self.advance(scope, runtime.session.as_ref());
                }
                outcome
            }
        }
    }

    fn advance(&mut self, scope: ComposerDraftScope, session: Option<&ClientSession>) {
        let next = self.scopes.get_mut(&scope).and_then(VecDeque::pop_front);
        match next {
            Some(queued) => self.start(scope, queued, session.map(ClientSession::requester)),
            None => {
                self.scopes.remove(&scope);
            }
        }
    }

    /// Runs every running and waiting job to its end, answering their
    /// asks, before the service stops.
    ///
    /// # Errors
    ///
    /// Returns the failure that ends the service: a closed event bridge.
    pub(super) async fn drain(
        &mut self,
        runtime: &mut ServiceRuntime,
        frames: &mut FrameFactory,
    ) -> Result<(), ServiceFailure> {
        while !self.is_idle() {
            let event = self.next().await;
            self.handle(event, runtime, frames).await?;
        }
        Ok(())
    }
}

impl Drop for MutationLane {
    fn drop(&mut self) {
        // The join set aborts its tasks on drop; cancelling first ends any
        // wait a task is polled through before the abort lands.
        self.cancel.cancel();
    }
}

/// Answers one lane job's session ask on the command loop. The connection
/// is replaced only when the job lost the current one (or the current one
/// has closed), so jobs that lost an already-replaced connection simply get
/// the new one.
async fn lane_session(
    runtime: &mut ServiceRuntime,
    frames: &mut FrameFactory,
    failed: Option<ConnectionId>,
    replace: bool,
) -> Result<SessionRequester, ServiceFailure> {
    let current = runtime.session.as_ref().map(ClientSession::requester);
    let lost_current = current.as_ref().is_some_and(|current| {
        current.is_closed() || (replace && failed.as_ref() == Some(current.connection_id()))
    });
    runtime.ensure_session(frames, lost_current).await?;
    runtime
        .session
        .as_ref()
        .map(ClientSession::requester)
        .ok_or(ServiceFailure::local_session())
}

/// The requests of one lane job: its own requester on the shared session,
/// and its own frames from the service's shared counter.
pub(super) struct LaneRequests {
    requester: Option<SessionRequester>,
    frames: FrameFactory,
    asks: mpsc::Sender<SessionAsk>,
    cancel: Arc<CancelHandle>,
}

impl LaneRequests {
    /// A requester on an open connection: the job's own, or a fresh one
    /// from the command loop. `replace` asks the loop to replace the job's
    /// connection even if it still looks open.
    async fn requester(&mut self, replace: bool) -> Result<SessionRequester, ServiceFailure> {
        if !replace
            && let Some(requester) = self.requester.as_ref().filter(|held| !held.is_closed())
        {
            return Ok(requester.clone());
        }
        let failed = self
            .requester
            .take()
            .map(|requester| requester.connection_id().clone());
        let (reply, answer) = oneshot::channel();
        self.asks
            .send(SessionAsk {
                failed,
                replace,
                reply,
            })
            .await
            .map_err(|_| ServiceFailure::local_session())?;
        let requester = answer
            .await
            .map_err(|_| ServiceFailure::local_session())??;
        self.requester = Some(requester.clone());
        Ok(requester)
    }

    /// Sends one query or command under a fresh frame identity, once.
    pub(super) async fn request(
        &mut self,
        request: ClientRequest,
        expected: ExpectedResponse,
    ) -> Result<ResponsePayload, RequestFailure> {
        let requester = self
            .requester(false)
            .await
            .map_err(RequestFailure::terminal)?;
        let (envelope, request_id) =
            make_request_frame(&mut self.frames, requester.protocol_version(), request)
                .map_err(RequestFailure::terminal)?;
        self.exchange(&requester, envelope, request_id, expected)
            .await
    }

    /// Sends one durable mutation under its stable identity, and once more
    /// on a replaced connection when the first attempt's failure allows it;
    /// the lane's form of `durable_save_request`.
    pub(super) async fn durable(
        &mut self,
        mutation: &StableMutation,
        expected: ExpectedResponse,
    ) -> Result<ResponsePayload, RequestFailure> {
        match self.stable(mutation, expected.clone(), false).await {
            Err(error) if error.durable_save_retry_allowed() => {
                self.stable(mutation, expected, true).await
            }
            attempt => attempt,
        }
    }

    async fn stable(
        &mut self,
        mutation: &StableMutation,
        expected: ExpectedResponse,
        replace: bool,
    ) -> Result<ResponsePayload, RequestFailure> {
        let requester = self
            .requester(replace)
            .await
            .map_err(RequestFailure::terminal)?;
        let (envelope, request_id) = mutation
            .envelope(requester.protocol_version())
            .map_err(RequestFailure::terminal)?;
        self.exchange(&requester, envelope, request_id, expected)
            .await
    }

    async fn exchange(
        &mut self,
        requester: &SessionRequester,
        envelope: WireEnvelope,
        request_id: RequestId,
        expected: ExpectedResponse,
    ) -> Result<ResponsePayload, RequestFailure> {
        let resolved = requester
            .request(envelope, &self.cancel)
            .await
            .map_err(|error| {
                // A lost connection (or a spent admission budget) is the
                // session's, not the request's: a retry asks the command
                // loop to replace it.
                RequestFailure {
                    failure: ServiceFailure::local_session(),
                    peer: None,
                    retryable_local_session_loss: local_session_request_loss_is_retryable(&error)
                        || matches!(error, ClientRequestError::Admission(_)),
                }
            })?;
        validate_resolved_request(&request_id, resolved, expected)
    }
}
