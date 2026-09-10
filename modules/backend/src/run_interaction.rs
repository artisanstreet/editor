//! Bounded live-run interaction routing shared by Forge request handling and
//! native run dispatch.
//!
//! This registry mirrors [`crate::run_cancellation::RunCancellationRegistry`]
//! on purpose: it is only a routing table from one exact live
//! `(ThreadId, RunId)` pair to that run's bounded mid-turn inbox. It owns no
//! engine, no provider session, no repository transaction, and no terminal
//! state. The owning dispatch loop registers when its turn starts consuming,
//! drains the inbox alongside observations, and unregisters when the run
//! settles; the request handler routes authenticated responses into the
//! inbox and awaits the owning loop's acknowledgement.
//!
//! A [`RunInteractionLease`] is the sole public lifetime capability for one
//! registration. Drop it (after draining) to return its slot. Registry clones
//! share state; cloning an inbox sender does not clone or release the
//! registration.

#![forbid(unsafe_code)]
#![allow(clippy::module_name_repetitions)]

use std::{
    collections::HashMap,
    fmt,
    num::NonZeroU64,
    sync::{Arc, Mutex, MutexGuard},
};

use artisan_domain::{ObservationId, RequestId, RunId, ThreadId};
use thiserror::Error;
use tokio::sync::{mpsc, oneshot};

/// Bounded capacity of one live run's mid-turn inbox.
///
/// Responses queue while the owning loop finishes its current observation;
/// beyond this depth the route fails retryably without storing anything, so
/// a slow owner never loses or duplicates a response.
pub const INTERACTION_INBOX_CAPACITY: usize = 8;

/// Failure while constructing or mutating the live-run interaction table.
///
/// The error intentionally carries no thread, run, decision, or answer
/// payload. A poisoned lock is never recovered: callers receive this typed
/// failure and the table fails closed without a best-effort mutation through
/// potentially inconsistent state.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum RunInteractionRegistryError {
    /// A zero capacity could not admit any live run.
    #[error("run interaction registry capacity must be nonzero")]
    ZeroCapacity,
    /// The exact `(ThreadId, RunId)` pair already has a live registration.
    #[error("run interaction is already registered for this thread and run")]
    Duplicate,
    /// Every live registration slot is occupied.
    #[error("run interaction registry reached its capacity of {capacity} live runs")]
    AtCapacity {
        /// The fixed maximum number of live registrations.
        capacity: usize,
    },
    /// Allocating the next registration generation would wrap or reuse zero.
    #[error("run interaction registration generation is exhausted")]
    GenerationExhausted,
    /// The registry mutex was poisoned by a prior panic while holding it.
    #[error("run interaction registry lock is poisoned")]
    Poisoned,
}

/// One validated mid-turn response travelling to its owning run.
///
/// The envelope carries the domain-validated decision plus a single-shot
/// acknowledgement the owning loop answers exactly once. The loop resolves
/// the response transactionally and commits its resolution observation; the
/// route awaits that acknowledgement instead of guessing the outcome.
#[derive(Debug)]
pub struct RunInteractionEnvelope {
    /// Thread that must own the exact target run.
    pub thread_id: ThreadId,
    /// Exact native run identity owning the pending request.
    pub run_id: RunId,
    /// The validated response to deliver.
    pub command: OwnedInteractionCommand,
    /// Single-shot acknowledgement answered by the owning loop.
    pub respond: oneshot::Sender<RunInteractionAck>,
}

/// The validated response carried by one envelope.
#[derive(Clone, Debug, PartialEq)]
pub enum OwnedInteractionCommand {
    /// An explicit approval decision for one pending approval.
    RespondApproval {
        /// Client-minted stable request identity.
        request_id: RequestId,
        /// Provider approval identity under review.
        approval_id: ObservationId,
        /// The explicit decision: true allows, false denies.
        approved: bool,
    },
    /// Explicit answers for one pending question.
    RespondQuestion {
        /// Client-minted stable request identity.
        request_id: RequestId,
        /// Provider question identity under review.
        question_id: ObservationId,
        /// The explicit answers, possibly empty for a skipped question.
        answers: Vec<String>,
    },
}

impl OwnedInteractionCommand {
    /// Returns the stable client request identity.
    #[must_use]
    pub const fn request_id(&self) -> &RequestId {
        match self {
            Self::RespondApproval { request_id, .. } | Self::RespondQuestion { request_id, .. } => {
                request_id
            }
        }
    }
}

/// The owning loop's exactly-once answer to one routed response.
#[derive(Debug)]
pub enum RunInteractionAck {
    /// The loop settled the response; the receipt is the stored durable
    /// outcome with its accepted-or-duplicate disposition.
    Settled(artisan_database::StoredInteractionReceipt),
    /// The request id was already accepted for a different intent. The
    /// originally accepted outcome stands.
    Conflict,
    /// The named run is not live, not running, or rebound. Never stored:
    /// the client may retry once the owning run is live.
    WrongRun,
    /// The loop could not settle the response transiently (for example its
    /// clock was unavailable). Nothing was stored; retry may succeed.
    Unavailable,
}

type RunKey = (ThreadId, RunId);

struct ActiveRun {
    generation: NonZeroU64,
    inbox: mpsc::Sender<RunInteractionEnvelope>,
}

struct RegistryState {
    next_generation: u64,
    active: HashMap<RunKey, ActiveRun>,
}

struct RegistryInner {
    capacity: usize,
    state: Mutex<RegistryState>,
}

impl RegistryInner {
    fn lock(&self) -> Result<MutexGuard<'_, RegistryState>, RunInteractionRegistryError> {
        self.state
            .lock()
            .map_err(|_| RunInteractionRegistryError::Poisoned)
    }
}

impl RegistryState {
    fn allocate_generation(&mut self) -> Result<NonZeroU64, RunInteractionRegistryError> {
        let generation = NonZeroU64::new(self.next_generation)
            .ok_or(RunInteractionRegistryError::GenerationExhausted)?;
        let next_generation = self
            .next_generation
            .checked_add(1)
            .ok_or(RunInteractionRegistryError::GenerationExhausted)?;
        self.next_generation = next_generation;
        Ok(generation)
    }
}

/// Cloneable, bounded registry of currently live native runs accepting
/// mid-turn interaction input.
///
/// The registry stores only live registrations. Dropping a lease removes its
/// entry and returns its slot; no tombstone or historical run id is
/// retained. The generation counter is one bounded scalar and never wraps,
/// so a stale lease cannot remove a later registration that happens to use
/// the same exact ids.
#[derive(Clone)]
pub struct RunInteractionRegistry {
    inner: Arc<RegistryInner>,
}

impl fmt::Debug for RunInteractionRegistry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RunInteractionRegistry")
            .field("capacity", &self.capacity())
            .finish_non_exhaustive()
    }
}

impl RunInteractionRegistry {
    /// Creates an empty registry with the fixed finite live-run capacity.
    ///
    /// # Errors
    ///
    /// Returns [`RunInteractionRegistryError::ZeroCapacity`] when `capacity`
    /// is zero. The capacity cannot change after construction.
    pub fn new(capacity: usize) -> Result<Self, RunInteractionRegistryError> {
        if capacity == 0 {
            return Err(RunInteractionRegistryError::ZeroCapacity);
        }

        Ok(Self {
            inner: Arc::new(RegistryInner {
                capacity,
                state: Mutex::new(RegistryState {
                    next_generation: 1,
                    active: HashMap::new(),
                }),
            }),
        })
    }

    /// Returns the fixed maximum number of live registrations.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.inner.capacity
    }

    /// Registers one exact live `(thread_id, run_id)` pair with a fresh
    /// bounded mid-turn inbox.
    ///
    /// Success returns a non-cloneable RAII lease and the inbox receiver the
    /// owning loop drains alongside observations. A duplicate exact pair is
    /// rejected before capacity is considered; a different pair is rejected
    /// when all live slots are occupied. The generation is allocated only
    /// after those checks and is advanced with checked arithmetic, so a
    /// rejected attempt never changes the table.
    ///
    /// # Errors
    ///
    /// Returns [`RunInteractionRegistryError::Duplicate`],
    /// [`RunInteractionRegistryError::AtCapacity`],
    /// [`RunInteractionRegistryError::GenerationExhausted`], or
    /// [`RunInteractionRegistryError::Poisoned`].
    #[allow(clippy::map_entry)]
    pub fn register(
        &self,
        thread_id: ThreadId,
        run_id: RunId,
    ) -> Result<
        (RunInteractionLease, mpsc::Receiver<RunInteractionEnvelope>),
        RunInteractionRegistryError,
    > {
        let key = (thread_id.clone(), run_id.clone());
        let mut state = self.inner.lock()?;

        if state.active.contains_key(&key) {
            return Err(RunInteractionRegistryError::Duplicate);
        }
        if state.active.len() >= self.inner.capacity {
            return Err(RunInteractionRegistryError::AtCapacity {
                capacity: self.inner.capacity,
            });
        }

        let generation = state.allocate_generation()?;
        let (inbox, receiver) = mpsc::channel(INTERACTION_INBOX_CAPACITY);
        state.active.insert(key, ActiveRun { generation, inbox });

        Ok((
            RunInteractionLease {
                inner: Arc::clone(&self.inner),
                thread_id,
                run_id,
                generation,
                registered: true,
            },
            receiver,
        ))
    }

    /// Returns a clone of the live inbox sender for exactly
    /// `(thread_id, run_id)`, or `None` when no live registration matches
    /// both identities.
    ///
    /// # Errors
    ///
    /// Returns [`RunInteractionRegistryError::Poisoned`] without routing when
    /// the registry lock is poisoned.
    pub fn route(
        &self,
        thread_id: &ThreadId,
        run_id: &RunId,
    ) -> Result<Option<mpsc::Sender<RunInteractionEnvelope>>, RunInteractionRegistryError> {
        let key = (thread_id.clone(), run_id.clone());
        let state = self.inner.lock()?;
        Ok(state.active.get(&key).map(|active| active.inbox.clone()))
    }
}

/// RAII ownership of one live interaction registration.
///
/// This type intentionally does not implement [`Clone`]. Cloning the
/// registry or any inbox sender obtained from this lease does not release
/// it; only this lease's drop can evict the exact generation it owns.
pub struct RunInteractionLease {
    inner: Arc<RegistryInner>,
    thread_id: ThreadId,
    run_id: RunId,
    generation: NonZeroU64,
    registered: bool,
}

impl fmt::Debug for RunInteractionLease {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RunInteractionLease")
            .field("generation", &self.generation)
            .field("registered", &self.registered)
            .finish_non_exhaustive()
    }
}

impl RunInteractionLease {
    /// Returns the exact thread identity owned by this lease.
    #[must_use]
    pub const fn thread_id(&self) -> &ThreadId {
        &self.thread_id
    }

    /// Returns the exact run identity owned by this lease.
    #[must_use]
    pub const fn run_id(&self) -> &RunId {
        &self.run_id
    }

    /// Returns this registration's strictly increasing, nonzero generation.
    #[must_use]
    pub const fn generation(&self) -> NonZeroU64 {
        self.generation
    }

    fn release(&mut self) -> Result<(), RunInteractionRegistryError> {
        if !self.registered {
            return Ok(());
        }

        let key = (self.thread_id.clone(), self.run_id.clone());
        let mut state = self.inner.lock()?;
        let owns_current_entry = state
            .active
            .get(&key)
            .is_some_and(|active| active.generation == self.generation);
        if owns_current_entry {
            state.active.remove(&key);
        }
        self.registered = false;
        Ok(())
    }
}

impl Drop for RunInteractionLease {
    fn drop(&mut self) {
        // Drop cannot report an error. In particular, it must never panic on
        // a poisoned lock; retaining the entry is the fail-closed outcome.
        let _ = self.release();
    }
}

#[cfg(test)]
mod tests {
    use artisan_domain::{RunId, ThreadId};

    use super::{RunInteractionRegistry, RunInteractionRegistryError};

    fn thread_id(value: &str) -> ThreadId {
        ThreadId::parse(value).expect("fixture thread id should be valid")
    }

    fn run_id(value: &str) -> RunId {
        RunId::parse(value).expect("fixture run id should be valid")
    }

    #[test]
    fn zero_capacity_is_rejected() {
        assert!(matches!(
            RunInteractionRegistry::new(0),
            Err(RunInteractionRegistryError::ZeroCapacity)
        ));
    }

    #[test]
    fn exact_run_routes_its_inbox_and_wrong_pairs_do_not() {
        let registry = RunInteractionRegistry::new(2).expect("fixture capacity should be valid");
        let route_thread = thread_id("thread-route");
        let route_run = run_id("run-route");
        let (_lease, _receiver) = registry
            .register(route_thread.clone(), route_run.clone())
            .expect("exact run should register");

        assert!(
            registry
                .route(&route_thread, &route_run)
                .expect("routing should succeed")
                .is_some()
        );
        assert!(
            registry
                .route(&thread_id("thread-other"), &route_run)
                .expect("routing should succeed")
                .is_none()
        );
        assert!(
            registry
                .route(&route_thread, &run_id("run-stale"))
                .expect("routing should succeed")
                .is_none()
        );
    }

    #[test]
    fn duplicate_live_registration_is_rejected_before_capacity() {
        let registry = RunInteractionRegistry::new(1).expect("fixture capacity should be valid");
        let thread_id = thread_id("thread-duplicate");
        let run_id = run_id("run-duplicate");
        let _lease = registry
            .register(thread_id.clone(), run_id.clone())
            .expect("first run should register");

        assert!(matches!(
            registry.register(thread_id, run_id),
            Err(RunInteractionRegistryError::Duplicate)
        ));
    }

    #[test]
    fn dropping_a_lease_returns_its_slot_and_advances_generation() {
        let registry = RunInteractionRegistry::new(1).expect("fixture capacity should be valid");
        let thread_id = thread_id("thread-drop");
        let run_id = run_id("run-drop");
        let (first, _) = registry
            .register(thread_id.clone(), run_id.clone())
            .expect("first run should register");
        let first_generation = first.generation();
        drop(first);

        let (replacement, _) = registry
            .register(thread_id, run_id)
            .expect("dropped run should free its slot");
        assert!(replacement.generation() > first_generation);
    }

    #[test]
    fn stale_lease_cannot_remove_a_newer_generation() {
        let registry = RunInteractionRegistry::new(1).expect("fixture capacity should be valid");
        let thread_id = thread_id("thread-generation");
        let run_id = run_id("run-generation");
        let old = registry
            .register(thread_id.clone(), run_id.clone())
            .expect("old run should register");
        let old_generation = old.0.generation();

        {
            let mut state = registry.inner.state.lock().expect("test lock");
            let replacement_generation = old_generation
                .get()
                .checked_add(1)
                .and_then(std::num::NonZeroU64::new)
                .expect("fixture generation should be representable");
            let (inbox, _) = tokio::sync::mpsc::channel(super::INTERACTION_INBOX_CAPACITY);
            state.active.insert(
                (thread_id.clone(), run_id.clone()),
                super::ActiveRun {
                    generation: replacement_generation,
                    inbox,
                },
            );
        }

        drop(old);
        assert!(
            registry
                .route(&thread_id, &run_id)
                .expect("routing should succeed")
                .is_some()
        );
    }
}
