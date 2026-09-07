//! Thread-isolated client delivery state for conversation subscriptions.
//!
//! [`SubscriptionProjectionRegistry`] is the frontend boundary between an
//! already-decoded delivery stream and the per-thread
//! [`ConversationProjection`] values that a renderer can observe. It is a
//! deterministic, multi-thread-safe state machine: it performs no I/O,
//! scheduling, retry, clock, logging, serialization, or payload decoding.
//!
//! Each registered thread has one opaque [`SubscriptionHandle`] generation
//! and exactly one [`SubscriptionStatus`]. A handle is required when a start,
//! stop, or patch arrives so a frame left over from an older subscription
//! cannot mutate a replacement generation. Pending generations retain at most
//! [`MAX_PENDING_BATCHES`] batches in arrival order. Activation drains only a
//! contiguous prefix; the first mismatch or projection refusal fails closed
//! to [`SubscriptionStatus::ResnapshotRequired`].

use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard};

use artisan_domain::{ConversationCursor, ConversationSnapshot, PatchBatch, ThreadId};
use artisan_protocol::{ConversationSubscriptionStarted, ConversationSubscriptionStopped};

use crate::conversation_projection::{ConversationProjection, ProjectionError, ProjectionStatus};

/// Maximum number of early patch batches retained while activation is
/// pending.
pub const MAX_PENDING_BATCHES: usize = 64;

/// A synonym for [`MAX_PENDING_BATCHES`] for callers that describe the queue
/// as a pending-delivery limit.
pub const PENDING_BATCH_LIMIT: usize = MAX_PENDING_BATCHES;

/// Opaque identity for one registration of one conversation thread.
///
/// Re-registering the same thread creates a new generation. The handle is
/// deliberately required for delivery operations, so a late frame from an
/// earlier generation is ignored instead of being applied to the new one.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct SubscriptionHandle {
    thread_id: ThreadId,
    generation: u64,
}

impl SubscriptionHandle {
    /// Returns the thread owned by this registration generation.
    #[must_use]
    pub const fn thread_id(&self) -> &ThreadId {
        &self.thread_id
    }

    /// Returns the monotonically allocated registration generation.
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }
}

/// Alias for callers that name the opaque registration token by its
/// generation rather than its handle.
pub type SubscriptionGeneration = SubscriptionHandle;

/// Alias for callers that use token terminology for a registration handle.
pub type SubscriptionToken = SubscriptionHandle;

/// The complete observable state of one registered conversation thread.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SubscriptionStatus {
    /// A registration exists, but its server activation has not been applied.
    Pending,
    /// Activation succeeded and contiguous delivery may be applied.
    Active,
    /// Continuity or projection application failed; a fresh recovery is
    /// required before delivery can resume.
    ResnapshotRequired,
    /// Stop was acknowledged or the registration was retired locally.
    Unsubscribed,
}

/// Why an active subscription lost its delivery continuity.
///
/// This type intentionally carries no snapshot, message body, or patch
/// value. A nested [`ProjectionError`] is already payload-free and preserves
/// the existing projection seam's typed diagnosis.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DeliveryLostReason {
    /// A frame named a thread other than the generation it was delivered to.
    ThreadMismatch {
        /// Thread the registration serves.
        expected_thread_id: ThreadId,
        /// Thread named by the received frame.
        actual_thread_id: ThreadId,
    },
    /// More than the bounded number of early batches arrived before start.
    QueueOverflow {
        /// Maximum retained queue length.
        limit: usize,
    },
    /// A resumed start did not match the caller-owned projection baseline.
    ResumeBaselineMismatch {
        /// Cursor declared by the resume acknowledgement.
        expected_cursor: ConversationCursor,
        /// Cursor present in the caller-owned projection, if any.
        actual_cursor: Option<ConversationCursor>,
        /// Existing projection health at the time of validation.
        projection_status: ProjectionStatus,
    },
    /// The existing projection rejected a snapshot or batch.
    Projection(ProjectionError),
}

impl fmt::Display for DeliveryLostReason {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ThreadMismatch { .. } => {
                formatter.write_str("subscription delivery named the wrong thread")
            }
            Self::QueueOverflow { limit } => {
                write!(
                    formatter,
                    "subscription early-delivery queue exceeded {limit} batches"
                )
            }
            Self::ResumeBaselineMismatch { .. } => {
                formatter.write_str("resumed subscription did not match its projection baseline")
            }
            Self::Projection(error) => {
                write!(
                    formatter,
                    "conversation projection rejected delivery: {error}"
                )
            }
        }
    }
}

impl std::error::Error for DeliveryLostReason {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Projection(error) => Some(error),
            _ => None,
        }
    }
}

/// Failure from registration, activation, stop, or patch delivery.
///
/// Every variant is payload-free. In particular, failed operations never
/// retain or format a [`ConversationSnapshot`] or [`PatchBatch`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SubscriptionProjectionError {
    /// The finite generation namespace cannot allocate another generation.
    GenerationExhausted,
    /// Delivery continuity was lost for this live generation.
    DeliveryLost {
        /// Thread whose live generation entered recovery.
        thread_id: ThreadId,
        /// Registration generation that lost continuity.
        generation: u64,
        /// Typed, payload-free cause.
        reason: DeliveryLostReason,
    },
    /// The operation is not valid for the current live state.
    InvalidState {
        /// Thread whose registration was addressed.
        thread_id: ThreadId,
        /// Registration generation that was addressed.
        generation: u64,
        /// Current state that rejected the operation.
        status: SubscriptionStatus,
    },
}

impl fmt::Display for SubscriptionProjectionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::GenerationExhausted => {
                formatter.write_str("conversation subscription generation space is exhausted")
            }
            Self::DeliveryLost {
                thread_id,
                generation,
                reason,
            } => write!(
                formatter,
                "conversation subscription generation {generation} for thread {thread_id} lost delivery: {reason}"
            ),
            Self::InvalidState {
                thread_id,
                generation,
                status,
            } => write!(
                formatter,
                "conversation subscription generation {generation} for thread {thread_id} is {status:?}"
            ),
        }
    }
}

impl std::error::Error for SubscriptionProjectionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::DeliveryLost { reason, .. } => Some(reason),
            _ => None,
        }
    }
}

/// Result of applying a subscription start response.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ActivationOutcome {
    /// A fresh snapshot activated the generation.
    Fresh {
        /// Cursor visible after the snapshot and any queued drain.
        cursor: ConversationCursor,
        /// Number of queued batches applied during activation.
        drained_batches: usize,
    },
    /// A matching existing baseline activated the generation.
    Resumed {
        /// Cursor visible after the baseline and any queued drain.
        cursor: ConversationCursor,
        /// Number of queued batches applied during activation.
        drained_batches: usize,
    },
    /// The addressed generation was stale or already unsubscribed.
    Ignored,
}

impl ActivationOutcome {
    /// Returns the resulting cursor for an activation, or `None` when the
    /// addressed start was ignored.
    #[must_use]
    pub const fn cursor(self) -> Option<ConversationCursor> {
        match self {
            Self::Fresh { cursor, .. } | Self::Resumed { cursor, .. } => Some(cursor),
            Self::Ignored => None,
        }
    }

    /// Returns how many pending batches activation drained.
    #[must_use]
    pub const fn drained_batches(self) -> usize {
        match self {
            Self::Fresh {
                drained_batches, ..
            }
            | Self::Resumed {
                drained_batches, ..
            } => drained_batches,
            Self::Ignored => 0,
        }
    }
}

/// Result of accepting one delivery batch.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum DeliveryOutcome {
    /// The batch was retained while activation was pending.
    Queued {
        /// Number of pending batches retained after this delivery.
        queued_batches: usize,
    },
    /// The active projection applied the batch.
    Applied {
        /// Cursor published by the applied batch.
        to_cursor: ConversationCursor,
    },
    /// The addressed generation was stale or already unsubscribed.
    Ignored,
}

impl DeliveryOutcome {
    /// Returns the applied cursor, or `None` when the batch was queued or
    /// ignored.
    #[must_use]
    pub const fn to_cursor(self) -> Option<ConversationCursor> {
        match self {
            Self::Applied { to_cursor } => Some(to_cursor),
            Self::Queued { .. } | Self::Ignored => None,
        }
    }

    /// Returns the pending queue length, or `None` when the batch was applied
    /// or ignored.
    #[must_use]
    pub const fn queued_batches(self) -> Option<usize> {
        match self {
            Self::Queued { queued_batches } => Some(queued_batches),
            Self::Applied { .. } | Self::Ignored => None,
        }
    }
}

/// Result of retiring one subscription generation.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum UnsubscribeOutcome {
    /// The live generation is now unsubscribed.
    Unsubscribed,
    /// The addressed generation was stale or already unsubscribed.
    Ignored,
}

/// Cloneable, multi-thread-safe registry of per-thread conversation delivery.
///
/// Clones share the same registry. The mutex serializes each state-machine
/// transition, while a projection remains private to the registry except for
/// the read-only [`Self::with_projection`] callback.
#[derive(Clone)]
pub struct SubscriptionProjectionRegistry {
    state: Arc<Mutex<RegistryState>>,
}

impl SubscriptionProjectionRegistry {
    /// Creates an empty registry with no fabricated thread snapshot or
    /// cursor.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(RegistryState::new())),
        }
    }

    /// Registers a new pending generation with an empty projection for
    /// `thread_id`.
    ///
    /// Any earlier generation for the same thread is replaced. The returned
    /// handle must be used for its start, stop, and batch operations.
    ///
    /// # Errors
    ///
    /// Returns [`SubscriptionProjectionError::GenerationExhausted`] only
    /// after the finite generation counter reaches its representation limit.
    pub fn register(
        &self,
        thread_id: ThreadId,
    ) -> Result<SubscriptionHandle, SubscriptionProjectionError> {
        self.register_with_projection(ConversationProjection::new(thread_id))
    }

    /// Registers a new pending generation around an existing projection.
    ///
    /// This is the caller-provided baseline used by a valid resumed start.
    /// The projection is retained exactly as supplied; the registry does not
    /// invent a snapshot or cursor. Any earlier generation for the same
    /// thread is replaced.
    ///
    /// # Errors
    ///
    /// Returns [`SubscriptionProjectionError::GenerationExhausted`] only
    /// after the finite generation counter reaches its representation limit.
    pub fn register_with_projection(
        &self,
        projection: ConversationProjection,
    ) -> Result<SubscriptionHandle, SubscriptionProjectionError> {
        let thread_id = projection.thread_id().clone();
        let mut state = self.lock_state();
        let generation = match state.next_generation.checked_add(1) {
            Some(next) => {
                let current = state.next_generation;
                state.next_generation = next;
                current
            }
            None => return Err(SubscriptionProjectionError::GenerationExhausted),
        };
        let handle = SubscriptionHandle {
            thread_id: thread_id.clone(),
            generation,
        };
        insert_entry(
            &mut state,
            SubscriptionEntry::Pending {
                handle: handle.clone(),
                projection,
                early_batches: VecDeque::new(),
            },
        );
        Ok(handle)
    }

    /// Alias for [`Self::register_with_projection`] emphasizing that the
    /// projection is the resumed-delivery baseline.
    ///
    /// # Errors
    ///
    /// Returns [`SubscriptionProjectionError::GenerationExhausted`] when no
    /// new generation can be allocated.
    pub fn register_with_baseline(
        &self,
        projection: ConversationProjection,
    ) -> Result<SubscriptionHandle, SubscriptionProjectionError> {
        self.register_with_projection(projection)
    }

    /// Applies a fresh or resumed activation response to `handle`.
    ///
    /// Fresh starts install their authoritative snapshot through
    /// [`ConversationProjection::install_snapshot`]. Resumed starts require a
    /// ready caller-provided projection whose cursor exactly matches the
    /// response. Pending batches are then drained in arrival order until the
    /// first refusal; a refusal preserves the projection's last good visible
    /// state and enters recovery.
    ///
    /// A fresh start is also the explicit recovery path from
    /// [`SubscriptionStatus::ResnapshotRequired`]. Stale or unsubscribed
    /// handles return [`ActivationOutcome::Ignored`] and never mutate the
    /// current generation.
    ///
    /// # Errors
    ///
    /// Returns [`SubscriptionProjectionError::DeliveryLost`] when the start
    /// or queued drain cannot establish continuity, and
    /// [`SubscriptionProjectionError::InvalidState`] for a duplicate start on
    /// an active generation.
    pub fn start(
        &self,
        handle: &SubscriptionHandle,
        started: &ConversationSubscriptionStarted,
    ) -> Result<ActivationOutcome, SubscriptionProjectionError> {
        let mut state = self.lock_state();
        let Some(entry) = state.entries.remove(handle.thread_id()) else {
            return Ok(ActivationOutcome::Ignored);
        };
        if entry.generation() != handle.generation() {
            insert_entry(&mut state, entry);
            return Ok(ActivationOutcome::Ignored);
        }
        if entry.status() == SubscriptionStatus::Unsubscribed {
            insert_entry(&mut state, entry);
            return Ok(ActivationOutcome::Ignored);
        }

        match entry {
            SubscriptionEntry::Pending {
                handle: stored_handle,
                projection,
                early_batches,
            } => match started {
                ConversationSubscriptionStarted::Fresh(start) => activate_fresh(
                    &mut state,
                    stored_handle,
                    projection,
                    early_batches,
                    start.snapshot(),
                ),
                ConversationSubscriptionStarted::Resumed { thread_id, cursor } => activate_resumed(
                    &mut state,
                    stored_handle,
                    projection,
                    early_batches,
                    thread_id,
                    *cursor,
                ),
            },
            SubscriptionEntry::ResnapshotRequired {
                handle: stored_handle,
                projection,
            } => match started {
                ConversationSubscriptionStarted::Fresh(start) => activate_fresh(
                    &mut state,
                    stored_handle,
                    projection,
                    VecDeque::new(),
                    start.snapshot(),
                ),
                ConversationSubscriptionStarted::Resumed { thread_id, cursor } => activate_resumed(
                    &mut state,
                    stored_handle,
                    projection,
                    VecDeque::new(),
                    thread_id,
                    *cursor,
                ),
            },
            entry => {
                let status = entry.status();
                insert_entry(&mut state, entry);
                Err(SubscriptionProjectionError::InvalidState {
                    thread_id: handle.thread_id().clone(),
                    generation: handle.generation(),
                    status,
                })
            }
        }
    }

    /// Alias for [`Self::start`] using protocol-oriented naming.
    ///
    /// # Errors
    ///
    /// Propagates the errors documented by [`Self::start`].
    pub fn apply_started(
        &self,
        handle: &SubscriptionHandle,
        started: &ConversationSubscriptionStarted,
    ) -> Result<ActivationOutcome, SubscriptionProjectionError> {
        self.start(handle, started)
    }

    /// Alias for [`Self::start`] using response-oriented naming.
    ///
    /// # Errors
    ///
    /// Propagates the errors documented by [`Self::start`].
    pub fn apply_start(
        &self,
        handle: &SubscriptionHandle,
        started: &ConversationSubscriptionStarted,
    ) -> Result<ActivationOutcome, SubscriptionProjectionError> {
        self.start(handle, started)
    }

    /// Delivers one decoded patch batch to the addressed generation.
    ///
    /// Pending generations retain the batch in arrival order up to
    /// [`MAX_PENDING_BATCHES`]. Active generations delegate directly to the
    /// matching [`ConversationProjection::apply_batch`]. Any wrong-thread
    /// frame, cursor failure, projection refusal, or queue overflow affects
    /// only this live generation and enters
    /// [`SubscriptionStatus::ResnapshotRequired`]. Stale and unsubscribed
    /// handles consume and ignore the batch.
    ///
    /// # Errors
    ///
    /// Returns [`SubscriptionProjectionError::DeliveryLost`] for a continuity
    /// or projection failure. A generation already in recovery reports its
    /// existing typed [`ProjectionError::RecoveryRequired`] cause.
    pub fn deliver(
        &self,
        handle: &SubscriptionHandle,
        batch: PatchBatch,
    ) -> Result<DeliveryOutcome, SubscriptionProjectionError> {
        let mut state = self.lock_state();
        let Some(entry) = state.entries.remove(handle.thread_id()) else {
            return Ok(DeliveryOutcome::Ignored);
        };
        if entry.generation() != handle.generation() {
            insert_entry(&mut state, entry);
            return Ok(DeliveryOutcome::Ignored);
        }
        if entry.status() == SubscriptionStatus::Unsubscribed {
            insert_entry(&mut state, entry);
            return Ok(DeliveryOutcome::Ignored);
        }

        match entry {
            SubscriptionEntry::Pending {
                handle: stored_handle,
                projection,
                mut early_batches,
            } => {
                if batch.thread_id() != stored_handle.thread_id() {
                    let actual_thread_id = batch.thread_id().clone();
                    let error = delivery_lost(
                        &stored_handle,
                        DeliveryLostReason::ThreadMismatch {
                            expected_thread_id: stored_handle.thread_id().clone(),
                            actual_thread_id,
                        },
                    );
                    insert_recovery(&mut state, stored_handle, projection);
                    return Err(error);
                }
                if early_batches.len() >= MAX_PENDING_BATCHES {
                    let error = delivery_lost(
                        &stored_handle,
                        DeliveryLostReason::QueueOverflow {
                            limit: MAX_PENDING_BATCHES,
                        },
                    );
                    insert_recovery(&mut state, stored_handle, projection);
                    return Err(error);
                }
                early_batches.push_back(batch);
                let queued_batches = early_batches.len();
                insert_entry(
                    &mut state,
                    SubscriptionEntry::Pending {
                        handle: stored_handle,
                        projection,
                        early_batches,
                    },
                );
                Ok(DeliveryOutcome::Queued { queued_batches })
            }
            SubscriptionEntry::Active { handle, projection } => {
                deliver_active(&mut state, handle, projection, &batch)
            }
            SubscriptionEntry::ResnapshotRequired {
                handle: stored_handle,
                projection,
            } => {
                let error = delivery_lost(
                    &stored_handle,
                    DeliveryLostReason::Projection(ProjectionError::RecoveryRequired),
                );
                insert_recovery(&mut state, stored_handle, projection);
                Err(error)
            }
            SubscriptionEntry::Unsubscribed { .. } => {
                unreachable!("unsubscribed entries are returned before delivery dispatch")
            }
        }
    }

    /// Alias for [`Self::deliver`] using projection-oriented naming.
    ///
    /// # Errors
    ///
    /// Propagates the errors documented by [`Self::deliver`].
    pub fn apply_batch(
        &self,
        handle: &SubscriptionHandle,
        batch: PatchBatch,
    ) -> Result<DeliveryOutcome, SubscriptionProjectionError> {
        self.deliver(handle, batch)
    }

    /// Retires the addressed generation locally.
    ///
    /// The tombstone remains in the registry as `Unsubscribed`, so late
    /// starts and batches for this generation are deterministically ignored.
    /// A later explicit registration replaces that tombstone with a new
    /// pending generation.
    #[must_use]
    pub fn unsubscribe(&self, handle: &SubscriptionHandle) -> UnsubscribeOutcome {
        let mut state = self.lock_state();
        let Some(entry) = state.entries.remove(handle.thread_id()) else {
            return UnsubscribeOutcome::Ignored;
        };
        if entry.generation() != handle.generation()
            || entry.status() == SubscriptionStatus::Unsubscribed
        {
            insert_entry(&mut state, entry);
            return UnsubscribeOutcome::Ignored;
        }
        let (stored_handle, projection) = entry.into_projection();
        insert_entry(
            &mut state,
            SubscriptionEntry::Unsubscribed {
                handle: stored_handle,
                projection,
            },
        );
        UnsubscribeOutcome::Unsubscribed
    }

    /// Applies a protocol stop acknowledgement to the addressed generation.
    ///
    /// A stop for a stale or already-unsubscribed handle is ignored. A live
    /// handle whose stop payload names another thread fails closed to
    /// resnapshot-required.
    ///
    /// # Errors
    ///
    /// Returns [`SubscriptionProjectionError::DeliveryLost`] when a live stop
    /// acknowledgement names the wrong thread.
    pub fn apply_stopped(
        &self,
        handle: &SubscriptionHandle,
        stopped: &ConversationSubscriptionStopped,
    ) -> Result<UnsubscribeOutcome, SubscriptionProjectionError> {
        let mut state = self.lock_state();
        let Some(entry) = state.entries.remove(handle.thread_id()) else {
            return Ok(UnsubscribeOutcome::Ignored);
        };
        if entry.generation() != handle.generation()
            || entry.status() == SubscriptionStatus::Unsubscribed
        {
            insert_entry(&mut state, entry);
            return Ok(UnsubscribeOutcome::Ignored);
        }
        if stopped.thread_id.as_str() != handle.thread_id().as_str() {
            let actual_thread_id = stopped.thread_id.clone();
            let error = delivery_lost(
                handle,
                DeliveryLostReason::ThreadMismatch {
                    expected_thread_id: handle.thread_id().clone(),
                    actual_thread_id,
                },
            );
            let (stored_handle, projection) = entry.into_projection();
            insert_recovery(&mut state, stored_handle, projection);
            return Err(error);
        }
        let (stored_handle, projection) = entry.into_projection();
        insert_entry(
            &mut state,
            SubscriptionEntry::Unsubscribed {
                handle: stored_handle,
                projection,
            },
        );
        Ok(UnsubscribeOutcome::Unsubscribed)
    }

    /// Alias for [`Self::apply_stopped`] using stop-oriented naming.
    ///
    /// # Errors
    ///
    /// Propagates the errors documented by [`Self::apply_stopped`].
    pub fn stop(
        &self,
        handle: &SubscriptionHandle,
        stopped: &ConversationSubscriptionStopped,
    ) -> Result<UnsubscribeOutcome, SubscriptionProjectionError> {
        self.apply_stopped(handle, stopped)
    }

    /// Returns the state of `handle`, or `None` when it is stale or unknown.
    #[must_use]
    pub fn status(&self, handle: &SubscriptionHandle) -> Option<SubscriptionStatus> {
        let state = self.lock_state();
        let entry = state.entries.get(handle.thread_id())?;
        (entry.generation() == handle.generation()).then(|| entry.status())
    }

    /// Returns the current state for a thread, regardless of generation.
    #[must_use]
    pub fn status_for_thread(&self, thread_id: &ThreadId) -> Option<SubscriptionStatus> {
        self.lock_state()
            .entries
            .get(thread_id)
            .map(SubscriptionEntry::status)
    }

    /// Returns the current handle for `thread_id`, if it is registered.
    #[must_use]
    pub fn current_handle(&self, thread_id: &ThreadId) -> Option<SubscriptionHandle> {
        self.lock_state()
            .entries
            .get(thread_id)
            .map(SubscriptionEntry::handle)
    }

    /// Returns the projection's current cursor without exposing a second
    /// cursor owned by the registry.
    #[must_use]
    pub fn cursor(&self, handle: &SubscriptionHandle) -> Option<ConversationCursor> {
        self.with_projection(handle, |projection| {
            projection.snapshot().map(ConversationSnapshot::cursor)
        })
        .flatten()
    }

    /// Invokes a read-only callback with the existing projection for a live,
    /// stale-check-passing handle.
    ///
    /// The callback runs while the registry lock is held and must not call
    /// back into this registry. Returning a value is the safe way to copy a
    /// renderer-facing observation out of the critical section.
    #[must_use]
    pub fn with_projection<R>(
        &self,
        handle: &SubscriptionHandle,
        operation: impl FnOnce(&ConversationProjection) -> R,
    ) -> Option<R> {
        let state = self.lock_state();
        let entry = state.entries.get(handle.thread_id())?;
        if entry.generation() != handle.generation() {
            return None;
        }
        Some(operation(entry.projection()))
    }

    fn lock_state(&self) -> MutexGuard<'_, RegistryState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl Default for SubscriptionProjectionRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Name emphasizing that this registry belongs to conversation delivery.
pub type ConversationSubscriptionRegistry = SubscriptionProjectionRegistry;

/// Short alias for the conversation subscription registry.
pub type SubscriptionRegistry = SubscriptionProjectionRegistry;

struct RegistryState {
    next_generation: u64,
    entries: HashMap<ThreadId, SubscriptionEntry>,
}

impl RegistryState {
    fn new() -> Self {
        Self {
            next_generation: 1,
            entries: HashMap::new(),
        }
    }
}

enum SubscriptionEntry {
    Pending {
        handle: SubscriptionHandle,
        projection: ConversationProjection,
        early_batches: VecDeque<PatchBatch>,
    },
    Active {
        handle: SubscriptionHandle,
        projection: ConversationProjection,
    },
    ResnapshotRequired {
        handle: SubscriptionHandle,
        projection: ConversationProjection,
    },
    Unsubscribed {
        handle: SubscriptionHandle,
        projection: ConversationProjection,
    },
}

impl SubscriptionEntry {
    fn handle(&self) -> SubscriptionHandle {
        match self {
            Self::Pending { handle, .. }
            | Self::Active { handle, .. }
            | Self::ResnapshotRequired { handle, .. }
            | Self::Unsubscribed { handle, .. } => handle.clone(),
        }
    }

    fn generation(&self) -> u64 {
        match self {
            Self::Pending { handle, .. }
            | Self::Active { handle, .. }
            | Self::ResnapshotRequired { handle, .. }
            | Self::Unsubscribed { handle, .. } => handle.generation(),
        }
    }

    fn status(&self) -> SubscriptionStatus {
        match self {
            Self::Pending { .. } => SubscriptionStatus::Pending,
            Self::Active { .. } => SubscriptionStatus::Active,
            Self::ResnapshotRequired { .. } => SubscriptionStatus::ResnapshotRequired,
            Self::Unsubscribed { .. } => SubscriptionStatus::Unsubscribed,
        }
    }

    fn projection(&self) -> &ConversationProjection {
        match self {
            Self::Pending { projection, .. }
            | Self::Active { projection, .. }
            | Self::ResnapshotRequired { projection, .. }
            | Self::Unsubscribed { projection, .. } => projection,
        }
    }

    fn into_projection(self) -> (SubscriptionHandle, ConversationProjection) {
        match self {
            Self::Pending {
                handle, projection, ..
            }
            | Self::Active { handle, projection }
            | Self::ResnapshotRequired { handle, projection }
            | Self::Unsubscribed { handle, projection } => (handle, projection),
        }
    }
}

fn insert_entry(state: &mut RegistryState, entry: SubscriptionEntry) {
    let key = entry.handle().thread_id().clone();
    let _replaced = state.entries.insert(key, entry);
}

fn insert_recovery(
    state: &mut RegistryState,
    handle: SubscriptionHandle,
    projection: ConversationProjection,
) {
    insert_entry(
        state,
        SubscriptionEntry::ResnapshotRequired { handle, projection },
    );
}

fn deliver_active(
    state: &mut RegistryState,
    handle: SubscriptionHandle,
    mut projection: ConversationProjection,
    batch: &PatchBatch,
) -> Result<DeliveryOutcome, SubscriptionProjectionError> {
    if batch.thread_id() != handle.thread_id() {
        let actual_thread_id = batch.thread_id().clone();
        let error = delivery_lost(
            &handle,
            DeliveryLostReason::ThreadMismatch {
                expected_thread_id: handle.thread_id().clone(),
                actual_thread_id,
            },
        );
        insert_recovery(state, handle, projection);
        return Err(error);
    }

    match projection.apply_batch(batch) {
        Ok(disposition) => {
            let outcome = DeliveryOutcome::Applied {
                to_cursor: disposition.to_cursor,
            };
            insert_entry(state, SubscriptionEntry::Active { handle, projection });
            Ok(outcome)
        }
        Err(error) => {
            let lost = delivery_lost(&handle, DeliveryLostReason::Projection(error));
            insert_recovery(state, handle, projection);
            Err(lost)
        }
    }
}

fn delivery_lost(
    handle: &SubscriptionHandle,
    reason: DeliveryLostReason,
) -> SubscriptionProjectionError {
    SubscriptionProjectionError::DeliveryLost {
        thread_id: handle.thread_id().clone(),
        generation: handle.generation(),
        reason,
    }
}

fn activate_fresh(
    state: &mut RegistryState,
    handle: SubscriptionHandle,
    mut projection: ConversationProjection,
    early_batches: VecDeque<PatchBatch>,
    snapshot: &ConversationSnapshot,
) -> Result<ActivationOutcome, SubscriptionProjectionError> {
    let initial_cursor = snapshot.cursor();
    if let Err(error) = projection.install_snapshot(snapshot) {
        let lost = delivery_lost(&handle, DeliveryLostReason::Projection(error));
        insert_recovery(state, handle, projection);
        return Err(lost);
    }

    let (drained_batches, cursor) =
        match drain_queued(&mut projection, early_batches, initial_cursor) {
            Ok(result) => result,
            Err(error) => {
                let lost = delivery_lost(&handle, DeliveryLostReason::Projection(error));
                insert_recovery(state, handle, projection);
                return Err(lost);
            }
        };
    insert_entry(state, SubscriptionEntry::Active { handle, projection });
    Ok(ActivationOutcome::Fresh {
        cursor,
        drained_batches,
    })
}

fn activate_resumed(
    state: &mut RegistryState,
    handle: SubscriptionHandle,
    mut projection: ConversationProjection,
    early_batches: VecDeque<PatchBatch>,
    thread_id: &ThreadId,
    cursor: ConversationCursor,
) -> Result<ActivationOutcome, SubscriptionProjectionError> {
    if thread_id != handle.thread_id() {
        let lost = delivery_lost(
            &handle,
            DeliveryLostReason::ThreadMismatch {
                expected_thread_id: handle.thread_id().clone(),
                actual_thread_id: thread_id.clone(),
            },
        );
        insert_recovery(state, handle, projection);
        return Err(lost);
    }

    let actual_cursor = projection.snapshot().map(ConversationSnapshot::cursor);
    let projection_status = projection.status();
    if projection_status != ProjectionStatus::Ready || actual_cursor != Some(cursor) {
        let lost = delivery_lost(
            &handle,
            DeliveryLostReason::ResumeBaselineMismatch {
                expected_cursor: cursor,
                actual_cursor,
                projection_status,
            },
        );
        insert_recovery(state, handle, projection);
        return Err(lost);
    }

    let (drained_batches, cursor) = match drain_queued(&mut projection, early_batches, cursor) {
        Ok(result) => result,
        Err(error) => {
            let lost = delivery_lost(&handle, DeliveryLostReason::Projection(error));
            insert_recovery(state, handle, projection);
            return Err(lost);
        }
    };
    insert_entry(state, SubscriptionEntry::Active { handle, projection });
    Ok(ActivationOutcome::Resumed {
        cursor,
        drained_batches,
    })
}

fn drain_queued(
    projection: &mut ConversationProjection,
    mut early_batches: VecDeque<PatchBatch>,
    initial_cursor: ConversationCursor,
) -> Result<(usize, ConversationCursor), ProjectionError> {
    let mut applied = 0usize;
    let mut cursor = initial_cursor;
    while let Some(batch) = early_batches.pop_front() {
        let disposition = projection.apply_batch(&batch)?;
        cursor = disposition.to_cursor;
        applied += 1;
    }
    Ok((applied, cursor))
}
