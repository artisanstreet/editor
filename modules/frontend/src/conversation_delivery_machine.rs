//! Statig conversation delivery machine for one thread.
//!
//! The one event-driven owner that composes the existing
//! [`crate::conversation_projection::ConversationProjection`] with
//! transport-delivery lifecycle. No network I/O, no rendering, no clocks,
//! sleeps, tasks, channels, callbacks, `unsafe`, or GPUI types.
//!
//! State chart (hierarchical):
//!
//! - `Delivery` superstate owns shared close handling.
//! - `AwaitingSnapshot` — initial; no accepted baseline.
//! - `Ready` — contiguous delivery.
//! - `Recovering` — projection requested a resnapshot; last good snapshot stays
//!   visible.
//! - `Closed` — terminal; later delivery is ignored.
//!
//! Effects are appended to an explicit external controller context and drained
//! via safe ownership. Entry to `AwaitingSnapshot` requests the initial
//! baseline. Entry to `Recovering` requests exactly one fresh snapshot.
//! Repeated invalid batches while recovering do not storm requests. Explicit
//! retry increments exactly one generation. Generation overflow is a typed
//! error/terminal effect and never wraps.

use crate::conversation_projection::{
    ConversationProjection, ProjectionError, ProjectionStatus, SnapshotDisposition,
};
use artisan_domain::{ConversationCursor, ConversationSnapshot, PatchBatch, ThreadId};
use statig::Outcome::{Handled, Super, Transition};
use statig::prelude::*;

// Re-export for tests that want to match on projection types without
// reaching into the projection module separately.
pub use crate::conversation_projection::ProjectionError as DeliveryProjectionError;

/// Delivery phase visible through the read view.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum DeliveryPhase {
    /// No accepted baseline.
    AwaitingSnapshot,
    /// Contiguous snapshot/patch delivery.
    Ready,
    /// A resnapshot was requested; last good snapshot stays visible.
    Recovering,
    /// Terminal local owner state.
    Closed,
}

/// Closed event vocabulary.
///
/// All state changes are driven only by these events and the underlying
/// projection outcomes.
#[derive(Clone, Debug)]
pub enum ConversationDeliveryEvent {
    /// Authoritative snapshot received.
    SnapshotReceived(ConversationSnapshot),
    /// Authoritative patch batch received.
    BatchReceived(PatchBatch),
    /// Explicit resnapshot retry requested by the owner.
    RetryRequested,
    /// Owner closed.
    Closed,
}

/// Typed effect vocabulary.
///
/// Effects are appended to an external ordered outbox and drained explicitly.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConversationDeliveryEffect {
    /// Request an authoritative snapshot.
    RequestSnapshot {
        /// Fixed thread this owner serves.
        thread_id: ThreadId,
        /// Monotonically increasing local request generation.
        generation: u64,
        /// Last-good cursor when present.
        after: Option<ConversationCursor>,
    },
    /// Renderer invalidation after visible state changes.
    Invalidate,
    /// Typed projection refusal report; carries no message bodies.
    ReportRefusal {
        /// Projection diagnosis.
        error: ProjectionError,
    },
    /// Owner-closed notification.
    OwnerClosed {
        /// Fixed thread that was closed.
        thread_id: ThreadId,
    },
    /// Checked generation overflow — never wraps.
    GenerationExhausted,
}

/// Why a new request generation could not be allocated.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConversationDeliveryError {
    /// The representable generation space is exhausted.
    GenerationExhausted,
}

impl std::fmt::Display for ConversationDeliveryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::GenerationExhausted => {
                formatter.write_str("conversation delivery generation is exhausted")
            }
        }
    }
}

impl std::error::Error for ConversationDeliveryError {}

/// Read-only view without exposing mutable state.
#[derive(Clone, Debug)]
pub struct ConversationDeliveryView {
    /// Current delivery phase.
    pub phase: DeliveryPhase,
    /// Fixed thread identity.
    pub thread_id: ThreadId,
    /// Underlying projection status.
    pub projection_status: ProjectionStatus,
    /// Last-good cursor when a snapshot is present.
    pub cursor: Option<ConversationCursor>,
    /// Whether a last-good snapshot is present.
    pub has_snapshot: bool,
    /// Number of pending effects in the outbox.
    pub pending_effects: usize,
}

/// External effect outbox owned by the controller and passed as Statig context.
///
/// Keeping effects outside Statig shared storage allows the controller to drain
/// them without `unsafe` `inner_mut`/`state_mut` access.
type DeliveryContext = Vec<ConversationDeliveryEffect>;

/// Internal delivery storage that the Statig machine owns.
///
/// Fields are shared storage; state-local storage is not required for this
/// machine. All projection mutation stays here and is never duplicated.
/// Effects are emitted into the external [`DeliveryContext`].
struct Delivery {
    thread_id: ThreadId,
    projection: ConversationProjection,
    generation: u64,
}

impl Delivery {
    fn new(thread_id: ThreadId) -> Self {
        let projection = ConversationProjection::new(thread_id.clone());
        Self {
            thread_id,
            projection,
            generation: 0,
        }
    }

    fn with_generation(thread_id: ThreadId, generation: u64) -> Self {
        let projection = ConversationProjection::new(thread_id.clone());
        Self {
            thread_id,
            projection,
            generation,
        }
    }

    fn next_generation(&mut self) -> Result<u64, ConversationDeliveryError> {
        let next = self
            .generation
            .checked_add(1)
            .ok_or(ConversationDeliveryError::GenerationExhausted)?;
        self.generation = next;
        Ok(next)
    }

    fn emit_snapshot_request(&mut self, context: &mut DeliveryContext) {
        match self.next_generation() {
            Ok(generation) => {
                let after = self.projection.snapshot().map(|snapshot| snapshot.cursor());
                context.push(ConversationDeliveryEffect::RequestSnapshot {
                    thread_id: self.thread_id.clone(),
                    generation,
                    after,
                });
            }
            Err(_) => {
                context.push(ConversationDeliveryEffect::GenerationExhausted);
            }
        }
    }

    fn current_phase(state: &State) -> DeliveryPhase {
        match state {
            State::AwaitingSnapshot { .. } => DeliveryPhase::AwaitingSnapshot,
            State::Ready { .. } => DeliveryPhase::Ready,
            State::Recovering { .. } => DeliveryPhase::Recovering,
            State::Closed { .. } => DeliveryPhase::Closed,
        }
    }
}

#[statig::state_machine(
    initial = "State::awaiting_snapshot()",
    state(derive(Debug)),
    superstate(derive(Debug))
)]
impl Delivery {
    #[statig::state(superstate = "delivery", entry_action = "enter_awaiting_snapshot")]
    fn awaiting_snapshot(
        &mut self,
        context: &mut DeliveryContext,
        event: &ConversationDeliveryEvent,
    ) -> Outcome<State> {
        match event {
            ConversationDeliveryEvent::Closed => Super,
            ConversationDeliveryEvent::SnapshotReceived(snapshot) => {
                if snapshot.thread_id() != &self.thread_id {
                    context.push(ConversationDeliveryEffect::ReportRefusal {
                        error: ProjectionError::ThreadMismatch,
                    });
                    return Handled;
                }
                match self.projection.install_snapshot(snapshot) {
                    Ok(_) => {
                        context.push(ConversationDeliveryEffect::Invalidate);
                        Transition(State::ready())
                    }
                    Err(error) => {
                        context.push(ConversationDeliveryEffect::ReportRefusal { error });
                        Handled
                    }
                }
            }
            ConversationDeliveryEvent::BatchReceived(batch) => {
                if batch.thread_id() != &self.thread_id {
                    context.push(ConversationDeliveryEffect::ReportRefusal {
                        error: ProjectionError::ThreadMismatch,
                    });
                    return Handled;
                }
                match self.projection.apply_batch(batch) {
                    Ok(_) => {
                        context.push(ConversationDeliveryEffect::Invalidate);
                        Transition(State::ready())
                    }
                    Err(error) => {
                        let needs_recovery =
                            self.projection.status() == ProjectionStatus::ResnapshotRequired;
                        context.push(ConversationDeliveryEffect::ReportRefusal { error });
                        if needs_recovery {
                            Transition(State::recovering())
                        } else {
                            Handled
                        }
                    }
                }
            }
            ConversationDeliveryEvent::RetryRequested => {
                self.emit_snapshot_request(context);
                Handled
            }
        }
    }

    #[statig::state(superstate = "delivery")]
    fn ready(
        &mut self,
        context: &mut DeliveryContext,
        event: &ConversationDeliveryEvent,
    ) -> Outcome<State> {
        match event {
            ConversationDeliveryEvent::Closed => Super,
            ConversationDeliveryEvent::SnapshotReceived(snapshot) => {
                if snapshot.thread_id() != &self.thread_id {
                    context.push(ConversationDeliveryEffect::ReportRefusal {
                        error: ProjectionError::ThreadMismatch,
                    });
                    return Handled;
                }
                match self.projection.install_snapshot(snapshot) {
                    Ok(disposition) => {
                        if disposition == SnapshotDisposition::Applied {
                            context.push(ConversationDeliveryEffect::Invalidate);
                        } else {
                            // Unchanged equal-cursor identical snapshot clears recovery and
                            // remains visible; no invalidation is required because rows are
                            // identical, but the transition back to ready is already handled
                            // by staying ready.
                        }
                        Handled
                    }
                    Err(error) => {
                        context.push(ConversationDeliveryEffect::ReportRefusal { error });
                        Handled
                    }
                }
            }
            ConversationDeliveryEvent::BatchReceived(batch) => {
                if batch.thread_id() != &self.thread_id {
                    context.push(ConversationDeliveryEffect::ReportRefusal {
                        error: ProjectionError::ThreadMismatch,
                    });
                    return Handled;
                }
                match self.projection.apply_batch(batch) {
                    Ok(_) => {
                        context.push(ConversationDeliveryEffect::Invalidate);
                        Handled
                    }
                    Err(error) => {
                        let is_recovery =
                            self.projection.status() == ProjectionStatus::ResnapshotRequired;
                        context.push(ConversationDeliveryEffect::ReportRefusal { error });
                        if is_recovery {
                            Transition(State::recovering())
                        } else {
                            Handled
                        }
                    }
                }
            }
            ConversationDeliveryEvent::RetryRequested => Handled,
        }
    }

    #[statig::state(superstate = "delivery", entry_action = "enter_recovering")]
    fn recovering(
        &mut self,
        context: &mut DeliveryContext,
        event: &ConversationDeliveryEvent,
    ) -> Outcome<State> {
        match event {
            ConversationDeliveryEvent::Closed => Super,
            ConversationDeliveryEvent::SnapshotReceived(snapshot) => {
                if snapshot.thread_id() != &self.thread_id {
                    context.push(ConversationDeliveryEffect::ReportRefusal {
                        error: ProjectionError::ThreadMismatch,
                    });
                    return Handled;
                }
                match self.projection.install_snapshot(snapshot) {
                    Ok(_) => {
                        context.push(ConversationDeliveryEffect::Invalidate);
                        Transition(State::ready())
                    }
                    Err(error) => {
                        context.push(ConversationDeliveryEffect::ReportRefusal { error });
                        Handled
                    }
                }
            }
            ConversationDeliveryEvent::BatchReceived(batch) => {
                if batch.thread_id() != &self.thread_id {
                    context.push(ConversationDeliveryEffect::ReportRefusal {
                        error: ProjectionError::ThreadMismatch,
                    });
                    return Handled;
                }
                match self.projection.apply_batch(batch) {
                    Ok(_) => {
                        // Should not successfully apply while recovering, but if it did,
                        // it would have cleared recovery; keep batch atomicity invariant
                        // by not emitting invalidation during recovery.
                        Handled
                    }
                    Err(error) => {
                        context.push(ConversationDeliveryEffect::ReportRefusal { error });
                        Handled
                    }
                }
            }
            ConversationDeliveryEvent::RetryRequested => {
                self.emit_snapshot_request(context);
                Handled
            }
        }
    }

    #[statig::superstate]
    fn delivery(
        &mut self,
        _context: &mut DeliveryContext,
        event: &ConversationDeliveryEvent,
    ) -> Outcome<State> {
        match event {
            ConversationDeliveryEvent::Closed => Transition(State::closed()),
            _ => Super,
        }
    }

    #[statig::state(entry_action = "enter_closed")]
    fn closed(
        &mut self,
        _context: &mut DeliveryContext,
        _event: &ConversationDeliveryEvent,
    ) -> Outcome<State> {
        Handled
    }

    #[statig::action]
    fn enter_awaiting_snapshot(&mut self, context: &mut DeliveryContext) {
        self.emit_snapshot_request(context);
    }

    #[statig::action]
    fn enter_recovering(&mut self, context: &mut DeliveryContext) {
        self.emit_snapshot_request(context);
    }

    #[statig::action]
    fn enter_closed(&mut self, context: &mut DeliveryContext) {
        context.push(ConversationDeliveryEffect::OwnerClosed {
            thread_id: self.thread_id.clone(),
        });
    }
}

/// Small public controller that hides Statig's generated enum/wrapper details.
///
/// All async work remains outside the machine. Callers dispatch typed events
/// and inspect a read-only view; they never mutate projection state directly.
/// Effects live in controller-owned context and are drained without `unsafe`.
pub struct ConversationDeliveryController {
    machine: StateMachine<Delivery>,
    outbox: DeliveryContext,
}

impl ConversationDeliveryController {
    /// Creates an owner for `thread_id` and enters `AwaitingSnapshot`,
    /// emitting exactly one baseline snapshot request with generation one.
    #[must_use]
    pub fn new(thread_id: ThreadId) -> Self {
        let delivery = Delivery::new(thread_id);
        let mut machine = delivery.state_machine();
        let mut outbox = Vec::new();
        machine.init_with_context(&mut outbox);
        Self { machine, outbox }
    }

    /// Test-only constructor that starts the generation counter at `generation`.
    ///
    /// The next emitted request will be `generation + 1` and is checked, so
    /// starting at `u64::MAX` makes the very first entry exhaust, and starting
    /// at `u64::MAX - 1` leaves exactly one successful retry.
    #[must_use]
    pub fn with_initial_generation(thread_id: ThreadId, generation: u64) -> Self {
        let delivery = Delivery::with_generation(thread_id, generation);
        let mut machine = delivery.state_machine();
        let mut outbox = Vec::new();
        // Initial entry still runs; it will exhaust immediately if already at
        // `u64::MAX`. That is intentional for exhaustion tests.
        machine.init_with_context(&mut outbox);
        Self { machine, outbox }
    }

    /// Dispatches one typed event.
    ///
    /// Closing is idempotent and later events cannot reopen the owner. If a
    /// generation allocation fails during this dispatch, the corresponding
    /// [`ConversationDeliveryEffect::GenerationExhausted`] is queued and this
    /// returns [`ConversationDeliveryError::GenerationExhausted`]. An already
    /// pending exhaustion effect from a prior dispatch does not cause a new
    /// error on an unrelated later dispatch.
    ///
    /// # Errors
    ///
    /// Returns [`ConversationDeliveryError::GenerationExhausted`] when the
    /// finite generation space is exhausted and a new request id cannot be
    /// allocated during this dispatch.
    pub fn dispatch(
        &mut self,
        event: ConversationDeliveryEvent,
    ) -> Result<(), ConversationDeliveryError> {
        let is_closed = matches!(
            Delivery::current_phase(self.machine.state()),
            DeliveryPhase::Closed
        );
        if is_closed && matches!(event, ConversationDeliveryEvent::Closed) {
            // Idempotent: handling Closed while already closed stays closed
            // without emitting a second OwnerClosed.
            return Ok(());
        }

        let before_len = self.outbox.len();
        self.machine.handle_with_context(&event, &mut self.outbox);

        // Per-dispatch exhaustion: only effects produced by this dispatch count.
        let exhausted_this_dispatch = self.outbox[before_len..]
            .iter()
            .any(|effect| matches!(effect, ConversationDeliveryEffect::GenerationExhausted));

        if exhausted_this_dispatch {
            return Err(ConversationDeliveryError::GenerationExhausted);
        }

        // If we closed during this dispatch, outbox now contains OwnerClosed.
        // No extra handling needed; later dispatches remain inert via the
        // closed state's Handled response.

        Ok(())
    }

    /// Convenience: dispatch an authoritative snapshot.
    pub fn on_snapshot(
        &mut self,
        snapshot: ConversationSnapshot,
    ) -> Result<(), ConversationDeliveryError> {
        self.dispatch(ConversationDeliveryEvent::SnapshotReceived(snapshot))
    }

    /// Convenience: dispatch an authoritative patch batch.
    pub fn on_batch(&mut self, batch: PatchBatch) -> Result<(), ConversationDeliveryError> {
        self.dispatch(ConversationDeliveryEvent::BatchReceived(batch))
    }

    /// Convenience: request an explicit resnapshot retry.
    pub fn retry(&mut self) -> Result<(), ConversationDeliveryError> {
        self.dispatch(ConversationDeliveryEvent::RetryRequested)
    }

    /// Convenience: close the owner. Idempotent.
    pub fn close(&mut self) -> Result<(), ConversationDeliveryError> {
        self.dispatch(ConversationDeliveryEvent::Closed)
    }

    /// Drains the ordered outbox.
    #[must_use]
    pub fn drain_effects(&mut self) -> Vec<ConversationDeliveryEffect> {
        std::mem::take(&mut self.outbox)
    }

    /// Peeks at pending effects without draining.
    #[must_use]
    pub fn pending_effects(&self) -> &[ConversationDeliveryEffect] {
        &self.outbox
    }

    /// Returns the private outbox length without exposing mutable state.
    #[must_use]
    pub fn pending_effect_count(&self) -> usize {
        self.outbox.len()
    }

    /// Read-only view reporting phase, fixed thread, projection status,
    /// last-good snapshot/cursor, and pending-effect count.
    #[must_use]
    pub fn view(&self) -> ConversationDeliveryView {
        let inner = self.machine.inner();
        let phase = Delivery::current_phase(self.machine.state());
        ConversationDeliveryView {
            phase,
            thread_id: inner.thread_id.clone(),
            projection_status: inner.projection.status(),
            cursor: inner
                .projection
                .snapshot()
                .map(|snapshot| snapshot.cursor()),
            has_snapshot: inner.projection.snapshot().is_some(),
            pending_effects: self.outbox.len(),
        }
    }

    /// Returns the fixed thread this owner serves.
    #[must_use]
    pub fn thread_id(&self) -> &ThreadId {
        &self.machine.inner().thread_id
    }

    /// Returns the current delivery phase.
    #[must_use]
    pub fn phase(&self) -> DeliveryPhase {
        Delivery::current_phase(self.machine.state())
    }

    /// Returns the underlying projection status.
    #[must_use]
    pub fn projection_status(&self) -> ProjectionStatus {
        self.machine.inner().projection.status()
    }

    /// Returns the last-good snapshot when present.
    #[must_use]
    pub fn snapshot(&self) -> Option<&ConversationSnapshot> {
        self.machine.inner().projection.snapshot()
    }

    /// Returns the last-good cursor when present.
    #[must_use]
    pub fn cursor(&self) -> Option<ConversationCursor> {
        self.machine
            .inner()
            .projection
            .snapshot()
            .map(|s| s.cursor())
    }

    /// Returns the last allocated generation.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.machine.inner().generation
    }

    /// Returns whether the owner is terminally closed.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        matches!(
            Delivery::current_phase(self.machine.state()),
            DeliveryPhase::Closed
        )
    }
}
