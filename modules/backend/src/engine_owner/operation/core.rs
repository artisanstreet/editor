//! Handoff and vocabulary types for the engine owner operation lane.

use std::collections::HashMap;
use std::sync::Arc;

use artisan_domain::{ObservationId, RunId};
use tokio::sync::{mpsc, oneshot};
use tokio::time::Instant;

use artisan_transport::CancelHandle;

use super::super::catalog::{CatalogError, CatalogResult};
use super::super::http::HealthError;
use super::super::interaction::{
    InteractionDeliveryError, InteractionTarget, TurnInteractionLedger, TurnInteractionOutcome,
};
use super::super::observation::{EngineObservation, TerminalState};
use super::super::process::RetainedEngine;
use super::super::readiness::ReadinessError;
use super::super::{InternalCatalogInput, InternalPreflightInput, InternalTurnInput};
/// Payload-free engine health observable by the facade.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum HealthState {
    /// Admission is open and the owner task is serving work.
    Active,
    /// The owner irreversibly stopped serving new work.
    Quarantined,
}

/// Checked nonzero generation allocator.
///
/// Values start at 1, are handed out immediately before each spawn attempt,
/// and are burned whether or not the spawn succeeds. Allocation stops
/// permanently at [`u64::MAX`]; values never wrap, reset, or repeat within
/// the process. This is engine-owner incarnation numbering, NOT the durable
/// E1 run generation; it scopes only the live child identity for this owner
/// instance and has no effect on persisted run lifecycle.
pub(crate) struct GenerationAllocator {
    next: u64,
    exhausted: bool,
}

impl GenerationAllocator {
    /// Creates the allocator beginning at generation 1.
    pub(crate) const fn new() -> Self {
        Self {
            next: 1,
            exhausted: false,
        }
    }

    /// Mints and burns the next generation, if any remain.
    ///
    /// Returning `None` means the checked space is exhausted; the caller
    /// must refuse to launch any further child rather than wrap or reset.
    pub(crate) fn mint(&mut self) -> Option<u64> {
        if self.exhausted {
            return None;
        }
        let generation = self.next;
        match self.next.checked_add(1) {
            Some(next) => self.next = next,
            None => self.exhausted = true,
        }
        Some(generation)
    }

    /// Forces the internal counter for private end-of-space unit tests.
    ///
    /// This exists only under `cfg(test)`; there is deliberately no public
    /// setter for allocator state.
    #[cfg(test)]
    pub(crate) fn force_next(&mut self, value: u64) {
        self.next = value;
        self.exhausted = false;
    }
}

/// Why an admission was refused without queueing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LaunchAdmissionError {
    /// The owner is shut down, quarantined, or its channel is closed.
    Unavailable,
    /// The bounded queue is full.
    Busy,
    /// The caller-supplied budget cannot form a representable deadline.
    InvalidDeadline,
    /// The persisted observation capacity cannot form a bounded channel.
    InvalidCapacity,
}

/// Typed, payload-free failure of one engine operation.
///
/// Variants carry no path, payload, or operating-system strings; raw I/O
/// stays private to the owner task.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum EngineOperationError {
    /// The owner shut down before the operation settled.
    Shutdown,
    /// The operation was cancelled or abandoned by its caller.
    Cancelled,
    /// The caller-supplied budget elapsed, queue waiting included.
    Deadline,
    /// The checked generation space is exhausted.
    GenerationExhausted,
    /// The child could not be spawned.
    SpawnFailed,
    /// Operating-system entropy for the 32-byte secret failed.
    EntropyFailed,
    /// Bounded readiness parsing failed.
    ReadinessFailed(ReadinessError),
    /// Bounded health handshake failed.
    HealthFailed(HealthError),
    /// Runtime model catalog discovery failed after the owner authenticated
    /// the certified engine.
    CatalogFailed(CatalogError),
    /// Health version was incompatible with the expected value.
    IncompatibleVersion,
    /// Persisted turn settings could not be translated into owner limits.
    Configuration,
    /// The provider session could not be created or the prompt could not be
    /// delivered. The exact provider payload remains private to the owner.
    ProviderRequestFailed,
    /// The authenticated observation stream did not settle normally.
    StreamFailed,
    /// Cleanup could not observe the child's death and no primary cause
    /// existed to preserve alongside it.
    ReapUnresolved,
    /// A primary failure whose separately observed cleanup could not confirm
    /// the child's reap within the close budget.
    UnresolvedReapDuring {
        /// The original typed cause.
        primary: Box<EngineOperationError>,
    },
}

/// Observed cleanup mode for a successful configured-engine preflight.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PreflightReap {
    /// The child exited after stdin was closed without a termination request.
    WithoutKill,
    /// The child exited only after the bounded cleanup requested termination.
    AfterKill,
}

/// Small, payload-safe receipt for a completed configured-engine preflight.
///
/// The stable profile and version are available to the caller, while the
/// `Debug` representation redacts their bytes. No endpoint, executable,
/// process identity, secret, headers, or provider payload is retained.
#[allow(dead_code)]
pub(crate) struct PreflightReceipt {
    profile_id: String,
    version: String,
    reap: PreflightReap,
}

#[allow(dead_code)]
impl PreflightReceipt {
    pub(super) fn new(profile_id: String, version: String, reap: PreflightReap) -> Self {
        Self {
            profile_id,
            version,
            reap,
        }
    }

    pub(crate) fn profile_id(&self) -> &str {
        &self.profile_id
    }

    pub(crate) fn version(&self) -> &str {
        &self.version
    }

    pub(crate) const fn reap(&self) -> PreflightReap {
        self.reap
    }
}

impl std::fmt::Debug for PreflightReceipt {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PreflightReceipt")
            .field("profile_id", &"<redacted>")
            .field("version", &"<redacted>")
            .field("reap", &self.reap)
            .finish()
    }
}

pub(crate) type PreflightResult = Result<PreflightReceipt, EngineOperationError>;

/// Honest spawn and exit observation of one launch.
///
/// Reports only actually observed spawn and exit facts — observed generation
/// and whether the child exited. It does not claim readiness, binding,
/// provider state, or completion semantics.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LaunchOutcome {
    /// The child was spawned and its exit was observed with this generation.
    ObservedExit {
        /// The burned generation for this launch.
        generation: u64,
        /// Whether the observed exit status was success.
        success: bool,
    },
}

/// Result carried by one launch response channel.
pub(crate) type LaunchResult = Result<LaunchOutcome, EngineOperationError>;

/// One admitted launch travelling from the facade to the owner task.
pub(crate) enum Job {
    /// Existing readiness/health launch used by the isolated owner tests.
    #[allow(dead_code)]
    Legacy {
        run_id: RunId,
        deadline: Instant,
        control: Arc<CancelHandle>,
        respond: oneshot::Sender<LaunchResult>,
    },
    /// Configured spawn/readiness/health/observed-reap preflight that stops
    /// before provider session creation.
    #[allow(dead_code)]
    Preflight {
        input: Box<InternalPreflightInput>,
        control: Arc<CancelHandle>,
        respond: oneshot::Sender<PreflightResult>,
    },
    /// Configured spawn/readiness/health/location-scoped model discovery that
    /// stops before provider session creation.
    Catalog {
        input: Box<InternalCatalogInput>,
        control: Arc<CancelHandle>,
        respond: oneshot::Sender<CatalogOperationResult>,
    },
    /// A fully immutable configured turn handed to the owner after durable
    /// launch. Carries the single internal input so production and `#[cfg(test)]`
    /// fixture admissions share exactly one queued type and one executor.
    /// `steer_rx` is `Some` only for steer-capable engines (codex/claude);
    /// every other engine carries `None` and every steer attempt on
    /// it resolves [`SteerError::Unsupported`] without prompt-state plumbing.
    Turn {
        input: Box<InternalTurnInput>,
        deadline: Instant,
        control: Arc<CancelHandle>,
        prepared: oneshot::Sender<Result<PreparedSession, EngineOperationError>>,
        authorize: oneshot::Receiver<()>,
        observations: mpsc::Sender<EngineObservation>,
        respond: oneshot::Sender<TurnResult>,
        steer_rx: Option<mpsc::Receiver<SteerDelivery>>,
    },
}

/// Single-owner future for one admitted launch.
///
/// Deliberately not `Clone`. Dropping the future cancels its private signal
/// before admission ordering guarantees the owner learns of abandonment.
#[allow(dead_code)]
pub(crate) struct AcceptedLaunch {
    receiver: oneshot::Receiver<LaunchResult>,
    control: Arc<CancelHandle>,
}

#[allow(dead_code)]
impl AcceptedLaunch {
    /// Creates an accepted launch from its parts (facade-only).
    pub(crate) fn from_parts(
        receiver: oneshot::Receiver<LaunchResult>,
        control: Arc<CancelHandle>,
    ) -> Self {
        Self { receiver, control }
    }

    /// Cancels this launch explicitly.
    pub fn cancel(&self) {
        self.control.cancel();
    }
}

impl std::future::Future for AcceptedLaunch {
    type Output = LaunchResult;

    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        context: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        use std::pin::Pin;
        use std::task::Poll;
        match Pin::new(&mut self.receiver).poll(context) {
            Poll::Ready(Ok(result)) => Poll::Ready(result),
            Poll::Ready(Err(_)) => Poll::Ready(Err(EngineOperationError::ReapUnresolved)),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl Drop for AcceptedLaunch {
    fn drop(&mut self) {
        self.control.cancel();
    }
}

/// Single-owner future for one configured-engine preflight.
#[allow(dead_code)]
pub(crate) struct AcceptedPreflight {
    receiver: oneshot::Receiver<PreflightResult>,
    control: Arc<CancelHandle>,
}

#[allow(dead_code)]
impl AcceptedPreflight {
    /// Creates an accepted preflight from its owner-only response parts.
    pub(crate) fn from_parts(
        receiver: oneshot::Receiver<PreflightResult>,
        control: Arc<CancelHandle>,
    ) -> Self {
        Self { receiver, control }
    }

    /// Cancels this preflight explicitly.
    pub(crate) fn cancel(&self) {
        self.control.cancel();
    }
}

impl std::future::Future for AcceptedPreflight {
    type Output = PreflightResult;

    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        context: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        use std::pin::Pin;
        use std::task::Poll;
        match Pin::new(&mut self.receiver).poll(context) {
            Poll::Ready(Ok(result)) => Poll::Ready(result),
            Poll::Ready(Err(_)) => Poll::Ready(Err(EngineOperationError::ReapUnresolved)),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl Drop for AcceptedPreflight {
    fn drop(&mut self) {
        self.control.cancel();
    }
}

/// Single-owner future for one configured-engine runtime model catalog.
pub(crate) struct AcceptedCatalog {
    receiver: oneshot::Receiver<CatalogOperationResult>,
    control: Arc<CancelHandle>,
}

impl AcceptedCatalog {
    /// Creates an accepted catalog request from its owner-only response parts.
    pub(crate) fn from_parts(
        receiver: oneshot::Receiver<CatalogOperationResult>,
        control: Arc<CancelHandle>,
    ) -> Self {
        Self { receiver, control }
    }

    /// Cancels this catalog request explicitly.
    #[allow(dead_code)]
    pub(crate) fn cancel(&self) {
        self.control.cancel();
    }
}

impl std::future::Future for AcceptedCatalog {
    type Output = CatalogOperationResult;

    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        context: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        use std::pin::Pin;
        use std::task::Poll;
        match Pin::new(&mut self.receiver).poll(context) {
            Poll::Ready(Ok(result)) => Poll::Ready(result),
            Poll::Ready(Err(_)) => Poll::Ready(Err(EngineOperationError::ReapUnresolved)),
            Poll::Pending => Poll::Pending,
        }
    }
}

pub(crate) type CatalogOperationResult = Result<CatalogResult, EngineOperationError>;

impl Drop for AcceptedCatalog {
    fn drop(&mut self) {
        self.control.cancel();
    }
}

/// Safe session metadata released only after `OpenCode2` `CreateSession`.
pub(crate) struct PreparedSession {
    session: String,
}

impl PreparedSession {
    pub(super) fn new(session: String) -> Self {
        Self { session }
    }

    pub(crate) fn session(&self) -> &str {
        &self.session
    }
}

impl std::fmt::Debug for PreparedSession {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("PreparedSession { <redacted> }")
    }
}

/// Result of one configured provider turn after the owner has cleaned up its
/// child and transport drivers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct EngineTurnResult {
    pub(super) terminal: TerminalState,
}

impl EngineTurnResult {
    pub(crate) const fn terminal(self) -> TerminalState {
        self.terminal
    }
}

pub(crate) type TurnResult = Result<EngineTurnResult, EngineOperationError>;

/// Bounded steer-write failure. Carries no payload bytes.
///
/// `Unsupported` covers every path without a steer verb (cursor/grok/
/// opencode2 turns, or a turn whose pump never wired a steer channel) and
/// maps from the existing prompt-state decisions without per-call
/// fabrication. `DeliveryFailed` covers the failed provider write (stdin
/// write, gateway request, or a rejected correlated reply) as well as a
/// steer cut short by stop/cancel/deadline: only the provider ack counts,
/// never channel enqueue.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub(crate) enum SteerError {
    /// No steer verb exists on this path.
    #[error("steer is not supported on this engine path")]
    Unsupported,
    /// The provider write failed or the steer did not settle.
    #[error("steer write to the provider failed")]
    DeliveryFailed,
}

/// One follow-up text delivery into a live provider pump.
///
/// PRIVATE provider implementation: constructed only inside
/// [`AcceptedTurn::steer_text`] and consumed only by the owning pump loop.
/// Nothing outside `steer_text` names this shape; `message_id` stays with
/// durable dispatch and never enters the provider request. The pump writes
/// the engine verb with its owned writer/handles, then resolves `ack` with
/// the real provider ack/write outcome — never mere channel enqueue.
pub(crate) struct SteerDelivery {
    #[allow(dead_code)]
    pub(super) request_id: String,
    pub(super) text: String,
    pub(super) ack: oneshot::Sender<Result<(), SteerError>>,
}

impl SteerDelivery {
    /// Creates one delivery for focused provider tests.
    ///
    /// Production construction stays inside [`AcceptedTurn::steer_text`];
    /// this constructor exists only so tests can drive the servicing path
    /// without a live owner.
    #[cfg(test)]
    pub(crate) fn new(
        request_id: String,
        text: String,
        ack: oneshot::Sender<Result<(), SteerError>>,
    ) -> Self {
        Self {
            request_id,
            text,
            ack,
        }
    }
}

/// Capacity of one turn's steer channel: bounded like every other owner
/// queue so a burst of follow-ups back-pressures instead of buffering
/// without bound.
pub(crate) const STEER_CHANNEL_CAPACITY: usize = 16;

/// Resolves every unsettled steer delivery as failed: buffered pre-turn
/// writes, correlated-but-unanswered provider requests, and queued channel
/// arrivals. Called exactly once on every pump exit (terminal, failure,
/// shutdown, cancel, deadline) so `steer_text` never wedges on an
/// indefinite ack await and stop/cancel interrupts pending steers
/// deterministically. Sending is best-effort: a gone caller already
/// observed cancellation through its own control race.
pub(super) fn settle_steers_closed(
    steer_rx: &mut Option<mpsc::Receiver<SteerDelivery>>,
    buffered: &mut Vec<SteerDelivery>,
    pending: &mut HashMap<u64, oneshot::Sender<Result<(), SteerError>>>,
) {
    for delivery in buffered.drain(..) {
        let _ = delivery.ack.send(Err(SteerError::DeliveryFailed));
    }
    // Correlated-but-unanswered provider requests fail typed: their waiter
    // never saw an ack, so dropping the entry must resolve it failed.
    for (_, ack) in pending.drain() {
        let _ = ack.send(Err(SteerError::DeliveryFailed));
    }
    if let Some(rx) = steer_rx.as_mut() {
        rx.close();
        while let Ok(delivery) = rx.try_recv() {
            let _ = delivery.ack.send(Err(SteerError::DeliveryFailed));
        }
    }
}

/// Single-owner handoff for the configured turn phases.
pub(crate) struct AcceptedTurn {
    prepared: oneshot::Receiver<Result<PreparedSession, EngineOperationError>>,
    authorize_sender: Option<oneshot::Sender<()>>,
    observations: mpsc::Receiver<EngineObservation>,
    receiver: Option<oneshot::Receiver<TurnResult>>,
    control: Arc<CancelHandle>,
    interactions: TurnInteractionLedger,
    steer_tx: Option<mpsc::Sender<SteerDelivery>>,
}

impl AcceptedTurn {
    pub(crate) fn from_parts(
        run_id: RunId,
        prepared: oneshot::Receiver<Result<PreparedSession, EngineOperationError>>,
        authorize_sender: oneshot::Sender<()>,
        observations: mpsc::Receiver<EngineObservation>,
        receiver: oneshot::Receiver<TurnResult>,
        control: Arc<CancelHandle>,
        steer_tx: Option<mpsc::Sender<SteerDelivery>>,
    ) -> Self {
        Self {
            interactions: TurnInteractionLedger::new(run_id),
            prepared,
            authorize_sender: Some(authorize_sender),
            observations,
            receiver: Some(receiver),
            control,
            steer_tx,
        }
    }

    pub(crate) async fn prepare(&mut self) -> TurnResultPrepared {
        match (&mut self.prepared).await {
            Ok(result) => result,
            Err(_) => Err(EngineOperationError::ReapUnresolved),
        }
    }

    pub(crate) fn authorize(&mut self) -> Result<(), EngineOperationError> {
        self.authorize_sender
            .take()
            .ok_or(EngineOperationError::ProviderRequestFailed)?
            .send(())
            .map_err(|()| EngineOperationError::ProviderRequestFailed)
    }

    pub(crate) async fn next_observation(&mut self) -> Option<EngineObservation> {
        self.observations.recv().await
    }

    /// Writes follow-up text into the live turn's provider session.
    ///
    /// Returns an OWNED future (not `async fn`): the future captures a
    /// CLONED sender and owned request/text, never a borrowed turn, so the
    /// dispatch-side arm can drive it with `select!` while draining
    /// observations inline through `&mut turn`. A taken/consumed sender
    /// or a `&self`-borrowing future would either break subsequent steers
    /// or collide with the drain borrow — both are rejected shapes.
    ///
    /// Provider-ack result ONLY: `Ok(())` means the exact engine bytes
    /// reached the provider transport AND its ack resolved (never
    /// fire-and-forget: write-only acks that swallow late provider errors
    /// were rejected). Late provider rejections resolve `Err` with their
    /// typed reason preserved. No durability, no ledger, no commit, no row
    /// transitions — the dispatch-side arm owns all of those and polls
    /// the returned future exactly once per applied steer (redelivery
    /// dedup happens before this call via ledger `seen_commands`, never
    /// inside it).
    ///
    /// `request_id` is the ORIGINAL client request id (for wire correlation
    /// and log tracing only). `text` is the validated follow-up text. The
    /// sender is retained across calls, so consecutive steers on one turn
    /// each write exactly once; no accessor exposes the sender. A turn
    /// without a steer channel resolves [`SteerError::Unsupported`]; a
    /// steer cut short by stop/cancel, a dead pump, or a failed provider
    /// write resolves [`SteerError::DeliveryFailed`]. Channel enqueue
    /// alone never counts as success.
    ///
    /// Progress invariant (no drain-coupled deadlock): the ack waits for
    /// the actual provider outcome - the correlated `turn/steer` result
    /// for codex, the fold write for
    /// claude - and progress while it is outstanding comes from split
    /// ownership: the dispatch Steer arm keeps draining
    /// `next_observation()` through the existing `handle_observation`
    /// while awaiting the returned future, and the pump polls the steer
    /// receiver alongside its observation sends, so a provider withholding
    /// its reply while streaming past channel capacity wedges neither
    /// side. There is no ack timeout and no retry: one write attempt per
    /// call, and an ambiguous outcome fails typed instead of resending.
    pub(crate) fn steer_text(
        &self,
        request_id: &str,
        text: &str,
    ) -> impl std::future::Future<Output = Result<(), SteerError>> + Send + use<> {
        let sender = self.steer_tx.clone();
        let control = Arc::clone(&self.control);
        let request_id = request_id.to_owned();
        let text = text.to_owned();
        async move {
            let Some(sender) = sender else {
                return Err(SteerError::Unsupported);
            };
            let (ack_tx, ack_rx) = oneshot::channel();
            let delivery = SteerDelivery {
                request_id,
                text,
                ack: ack_tx,
            };
            let sent = tokio::select! {
                biased;
                () = control.wait() => false,
                result = sender.send(delivery) => result.is_ok(),
            };
            if !sent {
                return Err(SteerError::DeliveryFailed);
            }
            tokio::select! {
                biased;
                () = control.wait() => Err(SteerError::DeliveryFailed),
                result = ack_rx => result.unwrap_or(Err(SteerError::DeliveryFailed)),
            }
        }
    }

    /// Notes one pending provider target on this turn's delivery ledger.
    ///
    /// The owning dispatch loop seeds pending targets from durable state
    /// before delivering their responses. Re-noting never regresses a
    /// resolved target.
    pub(crate) fn note_interaction_requested(
        &mut self,
        target_id: &ObservationId,
        target: InteractionTarget,
    ) {
        self.interactions.note_requested(target_id, target);
    }

    /// Delivers one validated mid-turn response onto this turn.
    ///
    /// Applies idempotent command ids and per-target resolution tracking to
    /// the turn ledger with [`CommandTargetError`] on unknown or resolved
    /// ids. Delivery records the decision only and never disturbs control
    /// flow: answering never cancels, interrupts, or steers the turn.
    pub(crate) fn deliver_interaction_response(
        &mut self,
        command_id: &str,
        target_id: &ObservationId,
        target: InteractionTarget,
        intent: &str,
    ) -> Result<TurnInteractionOutcome, InteractionDeliveryError> {
        self.interactions
            .deliver(command_id, target_id, target, intent)
    }

    /// Nonmutating delivery preflight for one mid-turn response.
    ///
    /// Shares the ledger validation with
    /// [`Self::deliver_interaction_response`] without recording anything,
    /// so the steer arm can prove eligibility before provider contact and
    /// record only after the actual provider ack. Approval and question
    /// paths keep calling `deliver_interaction_response` unchanged.
    pub(crate) fn preflight_interaction_response(
        &self,
        command_id: &str,
        target_id: &ObservationId,
        target: InteractionTarget,
        intent: &str,
    ) -> Result<TurnInteractionOutcome, InteractionDeliveryError> {
        self.interactions
            .preflight(command_id, target_id, target, intent)
    }

    pub(crate) async fn finish(mut self) -> TurnResult {
        let Some(receiver) = self.receiver.take() else {
            return Err(EngineOperationError::ReapUnresolved);
        };
        match receiver.await {
            Ok(result) => result,
            Err(_) => Err(EngineOperationError::ReapUnresolved),
        }
    }

    pub(crate) fn cancel(&self) {
        self.control.cancel();
    }
}

impl Drop for AcceptedTurn {
    fn drop(&mut self) {
        self.control.cancel();
    }
}

type TurnResultPrepared = Result<PreparedSession, EngineOperationError>;

/// How one executed job ended for the owner loop.
pub(crate) enum Execution {
    /// The response was settled and no custody remains.
    Completed,
    /// Cleanup could not observe a reap; exact custody is retained for
    /// quarantine.
    Quarantined(Box<RetainedEngine>),
}
