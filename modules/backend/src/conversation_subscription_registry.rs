//! Per-connection conversation subscription registry.
//!
//! Synchronous, safe-Rust state table that tracks one entry per [`ThreadId`]
//! for the lifetime of a single QUIC connection. The table is later owned by
//! one `SessionDelivery`; it never touches storage, transport, clocks, or
//! async runtimes. Each registration mints a strictly monotonic, nonzero
//! connection-local generation carried inside an opaque [`SubscriptionLease`].
//! The lease fences every later mutation: any operation through a stale lease
//! fails without mutating the replacement entry.
//!
//! The table distinguishes `Pending` (registered but not yet activated) from
//! `Active` (activated after the successful Subscribe response write). A
//! `Pending` entry cannot accept patch batches; activation consumes the pending
//! state exactly once. Unsubscribe removes the current entry and renders its
//! lease stale forever. Successful batch publication requires an active lease
//! with exact thread equality and `from_cursor == current_cursor`, advancing
//! to `to_cursor` using the accepted [`PatchBatch`] contiguity guarantees.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::num::NonZeroU64;

use artisan_domain::{ConversationCursor, PatchBatch, ThreadId};
use thiserror::Error;

// ---------------------------------------------------------------------------
// Lease + state
// ---------------------------------------------------------------------------

/// Opaque, cloneable, equality-testable lease fencing one subscription entry.
///
/// The lease carries the owning thread identity plus a strictly monotonic,
/// nonzero, connection-local generation. The entry's generation is the only
/// source of truth; a copy of a lease never grants new authority after the
/// entry was replaced or removed.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct SubscriptionLease {
    thread_id: ThreadId,
    generation: NonZeroU64,
}

impl SubscriptionLease {
    /// Returns the thread this lease fences.
    #[must_use]
    pub fn thread_id(&self) -> &ThreadId {
        &self.thread_id
    }

    /// Returns the strictly monotonic generation of this lease.
    #[must_use]
    pub fn generation(&self) -> NonZeroU64 {
        self.generation
    }
}

/// Current lifecycle of a subscription entry.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SubscriptionState {
    /// Registered but not yet activated; the driver has not yet proven the
    /// Subscribe response was written.
    Pending,
    /// Activated after the driver proved the Subscribe response write.
    Active,
}

/// Read-only view of one subscription entry.
///
/// The view is an owned snapshot; mutating the registry never mutates a
/// previously returned view. The backing map is never exposed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SubscriptionView {
    lease: SubscriptionLease,
    state: SubscriptionState,
    cursor: ConversationCursor,
}

impl SubscriptionView {
    /// Returns the fencing lease for this entry.
    #[must_use]
    pub fn lease(&self) -> &SubscriptionLease {
        &self.lease
    }

    /// Returns the lifecycle of the entry.
    #[must_use]
    pub fn state(&self) -> SubscriptionState {
        self.state
    }

    /// Returns the last published cursor for this entry.
    #[must_use]
    pub fn cursor(&self) -> ConversationCursor {
        self.cursor
    }
}

/// Owned description of an entry removed by [`ConversationSubscriptionRegistry::unsubscribe`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemovedSubscription {
    lease: SubscriptionLease,
    state: SubscriptionState,
    cursor: ConversationCursor,
}

impl RemovedSubscription {
    /// Returns the lease that was removed; it is stale forever.
    #[must_use]
    pub fn lease(&self) -> &SubscriptionLease {
        &self.lease
    }

    /// Returns the lifecycle the entry had at removal time.
    #[must_use]
    pub fn state(&self) -> SubscriptionState {
        self.state
    }

    /// Returns the cursor the entry had at removal time.
    #[must_use]
    pub fn cursor(&self) -> ConversationCursor {
        self.cursor
    }
}

// ---------------------------------------------------------------------------
// Typed bounded errors / outcomes
// ---------------------------------------------------------------------------

/// Failure while registering a pending subscription.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Error)]
pub enum RegisterError {
    /// The connection-local generation counter is exhausted; allocation would
    /// wrap or reuse zero.
    #[error("subscription generation exhausted")]
    GenerationExhausted,
}

/// Failure while activating a pending subscription.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Error)]
pub enum ActivateError {
    /// The supplied lease does not match the current entry's generation, or
    /// the thread has no entry at all.
    #[error("subscription lease is stale")]
    StaleLease,
    /// The matching entry is already active; activation is single-shot.
    #[error("subscription is already active")]
    AlreadyActive,
}

/// Outcome of [`ConversationSubscriptionRegistry::unsubscribe`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UnsubscribeOutcome {
    /// An existing entry was removed.
    Removed(RemovedSubscription),
    /// No entry existed for the thread; no mutation occurred.
    Absent,
}

/// Failure while applying a durable [`PatchBatch`] through an active lease.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Error)]
pub enum ApplyBatchError {
    /// The supplied lease does not match the current entry's generation, or
    /// the thread has no entry at all.
    #[error("subscription lease is stale")]
    StaleLease,
    /// The batch's thread differs from the lease's thread.
    #[error("patch batch thread mismatch")]
    ThreadMismatch,
    /// The entry is still pending; activation is required before publication.
    #[error("subscription is pending; activation required")]
    NotActive,
    /// The batch's `from_cursor` does not equal the registry's current cursor.
    ///
    /// This covers duplicate, regression, and gap cases. The contained values
    /// are the expected current cursor and the batch's `from_cursor`.
    #[error("patch batch cursor mismatch")]
    CursorMismatch {
        /// Cursor the registry currently holds.
        expected: ConversationCursor,
        /// Cursor the batch claims to follow.
        actual: ConversationCursor,
    },
}

// ---------------------------------------------------------------------------
// Entry (private)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Entry {
    generation: NonZeroU64,
    state: SubscriptionState,
    cursor: ConversationCursor,
}

// ---------------------------------------------------------------------------
// Registry
// ---------------------------------------------------------------------------

/// Synchronous per-connection subscription table with one entry per [`ThreadId`].
///
/// Equality compares both the entry map and the next generation counter, so
/// two registries that arrived at the same entries through the same generation
/// history compare equal, while differing generation histories do not. The map
/// is deterministic (`BTreeMap`) and contains no interior mutability, clocks,
/// or global state. The registry is intentionally not [`Clone`] — the
/// per-connection owner must remain a single authority whose leases cannot be
/// duplicated by copying the table.
#[derive(Debug, Eq, PartialEq)]
pub struct ConversationSubscriptionRegistry {
    entries: BTreeMap<ThreadId, Entry>,
    next_generation: u64,
}

impl Default for ConversationSubscriptionRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ConversationSubscriptionRegistry {
    /// Creates an empty registry with the first generation set to one.
    #[must_use]
    pub fn new() -> Self {
        Self {
            entries: BTreeMap::new(),
            next_generation: 1,
        }
    }

    /// Returns whether the registry holds no entries.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Returns the number of tracked threads.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Removes every connection-local entry.
    ///
    /// This is used only by the owning connection teardown path. Generation
    /// history is retained so a later accidental use of the same registry
    /// cannot mint a lease that was already issued.
    pub(crate) fn clear_all(&mut self) {
        self.entries.clear();
    }

    /// Returns a read-only snapshot view for `thread_id`, if any.
    #[must_use]
    pub fn view(&self, thread_id: &ThreadId) -> Option<SubscriptionView> {
        self.entries.get(thread_id).map(|entry| SubscriptionView {
            lease: SubscriptionLease {
                thread_id: thread_id.clone(),
                generation: entry.generation,
            },
            state: entry.state,
            cursor: entry.cursor,
        })
    }

    /// Registers a pending subscription for `thread_id` at `cursor`.
    ///
    /// If an entry already exists for the thread it is atomically replaced
    /// with the new pending entry and a strictly newer lease. The old lease
    /// is immediately stale. Generation allocation is strictly monotonic and
    /// never reuses zero; exhaustion fails without mutating the registry.
    ///
    /// # Errors
    ///
    /// Returns [`RegisterError::GenerationExhausted`] when the generation
    /// counter is exhausted without mutating state.
    pub fn register_pending(
        &mut self,
        thread_id: ThreadId,
        cursor: ConversationCursor,
    ) -> Result<SubscriptionLease, RegisterError> {
        let generation_value = self.next_generation;
        let Some(generation) = NonZeroU64::new(generation_value) else {
            return Err(RegisterError::GenerationExhausted);
        };

        let next_generation = generation_value.checked_add(1).unwrap_or_default();

        let lease = SubscriptionLease {
            thread_id: thread_id.clone(),
            generation,
        };
        let entry = Entry {
            generation,
            state: SubscriptionState::Pending,
            cursor,
        };

        self.entries.insert(thread_id, entry);
        self.next_generation = next_generation;
        Ok(lease)
    }

    /// Activates a pending subscription through its matching lease.
    ///
    /// The first matching activation transitions the entry to [`SubscriptionState::Active`]
    /// and returns the cursor declared at registration time. A stale or missing
    /// lease, or a second activation of the same lease, is a typed no-mutation
    /// error.
    ///
    /// # Errors
    ///
    /// Returns [`ActivateError::StaleLease`] when no entry exists for the
    /// lease's thread or the generation does not match the current entry and
    /// [`ActivateError::AlreadyActive`] when the matching entry is already
    /// active. Neither case mutates the registry.
    pub fn activate(
        &mut self,
        lease: &SubscriptionLease,
    ) -> Result<ConversationCursor, ActivateError> {
        let Some(entry) = self.entries.get_mut(&lease.thread_id) else {
            return Err(ActivateError::StaleLease);
        };
        if entry.generation != lease.generation {
            return Err(ActivateError::StaleLease);
        }
        match entry.state {
            SubscriptionState::Pending => {
                entry.state = SubscriptionState::Active;
                Ok(entry.cursor)
            }
            SubscriptionState::Active => Err(ActivateError::AlreadyActive),
        }
    }

    /// Removes the subscription for `thread_id` regardless of pending/active state.
    ///
    /// Returns the owned description of what was removed, including its lease,
    /// which is stale forever. An unknown thread returns [`UnsubscribeOutcome::Absent`]
    /// without fabricating a removal.
    #[must_use]
    pub fn unsubscribe(&mut self, thread_id: &ThreadId) -> UnsubscribeOutcome {
        match self.entries.remove(thread_id) {
            Some(entry) => {
                let lease = SubscriptionLease {
                    thread_id: thread_id.clone(),
                    generation: entry.generation,
                };
                UnsubscribeOutcome::Removed(RemovedSubscription {
                    lease,
                    state: entry.state,
                    cursor: entry.cursor,
                })
            }
            None => UnsubscribeOutcome::Absent,
        }
    }

    /// Accepts one durable [`PatchBatch`] through its matching active lease.
    ///
    /// Requires exact thread equality and `batch.from_cursor() == current_cursor`;
    /// on success advances the entry to exactly `batch.to_cursor()`. A pending
    /// entry, stale lease, thread mismatch, duplicate, regression, or gap is a
    /// typed failure that leaves all state unchanged. The accepted batch's
    /// internal non-empty and contiguous guarantees are trusted; this method
    /// does not rebuild a second patch-order vocabulary.
    ///
    /// The cursor advances only after the later writer reports a successful
    /// publication.
    ///
    /// # Errors
    ///
    /// Returns a typed [`ApplyBatchError`] without mutating state for any
    /// precondition violation.
    pub fn publish_batch(
        &mut self,
        lease: &SubscriptionLease,
        batch: &PatchBatch,
    ) -> Result<ConversationCursor, ApplyBatchError> {
        let Some(entry) = self.entries.get_mut(&lease.thread_id) else {
            return Err(ApplyBatchError::StaleLease);
        };
        if entry.generation != lease.generation {
            return Err(ApplyBatchError::StaleLease);
        }
        if batch.thread_id() != &lease.thread_id {
            return Err(ApplyBatchError::ThreadMismatch);
        }
        match entry.state {
            SubscriptionState::Pending => return Err(ApplyBatchError::NotActive),
            SubscriptionState::Active => {}
        }
        if batch.from_cursor() != entry.cursor {
            return Err(ApplyBatchError::CursorMismatch {
                expected: entry.cursor,
                actual: batch.from_cursor(),
            });
        }
        entry.cursor = batch.to_cursor();
        Ok(entry.cursor)
    }
}
