//! Bounded live-run cancellation intent shared by Forge request handling and
//! native run dispatch.
//!
//! This registry is deliberately only a routing table. It owns one fresh
//! [`CancelHandle`] per live `(ThreadId, RunId)` pair and records whether the
//! cancellation intent for that pair has already been published. It does not
//! own an engine, a process, a repository transaction, a protocol receipt, or
//! any terminal state. The existing run owner observes the handle and remains
//! responsible for actual cancellation and terminal settlement.
//!
//! A [`RunCancellationLease`] is the sole public lifetime capability for one
//! registration. Keep it alive through the existing run owner's terminal
//! settlement, then drop it (or call [`RunCancellationLease::unregister`]).
//! Registry clones share state; cloning the handle returned by
//! [`RunCancellationLease::cancel_handle`] does not clone or release the
//! registration.

#![forbid(unsafe_code)]
#![allow(clippy::module_name_repetitions)]

use std::{
    collections::HashMap,
    fmt,
    num::NonZeroU64,
    sync::{Arc, Mutex, MutexGuard},
};

use artisan_domain::{RunId, ThreadId};
use artisan_transport::CancelHandle;
use thiserror::Error;

/// Failure while constructing or mutating the live-run cancellation table.
///
/// The error intentionally carries no thread, run, source, message, provider,
/// or transport payload. A poisoned lock is never recovered: callers receive
/// this typed failure and the table fails closed without making a best-effort
/// mutation through potentially inconsistent state.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum RunCancellationError {
    /// A zero capacity could not admit any live run.
    #[error("run cancellation registry capacity must be nonzero")]
    ZeroCapacity,
    /// The exact `(ThreadId, RunId)` pair already has a live registration.
    #[error("run cancellation is already registered for this thread and run")]
    Duplicate,
    /// Every live registration slot is occupied.
    #[error("run cancellation registry reached its capacity of {capacity} live runs")]
    AtCapacity {
        /// The fixed maximum number of live registrations.
        capacity: usize,
    },
    /// Allocating the next registration generation would wrap or reuse zero.
    #[error("run cancellation registration generation is exhausted")]
    GenerationExhausted,
    /// The registry mutex was poisoned by a prior panic while holding it.
    #[error("run cancellation registry lock is poisoned")]
    Poisoned,
    /// More than one live run belongs to the queried thread.
    #[error("run cancellation has multiple active runs for the queried thread")]
    AmbiguousActiveRuns,
}

/// Result of one exact cancellation request.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum CancelRequestOutcome {
    /// The registry accepted and published the first cancellation intent for
    /// the live exact run.
    Signalled,
    /// The exact live run had already received a cancellation intent.
    AlreadySignalled,
    /// No live registration matched both the supplied thread and run ids.
    NotActive,
}

type RunKey = (ThreadId, RunId);

struct ActiveRun {
    generation: NonZeroU64,
    cancel: Arc<CancelHandle>,
    signalled: bool,
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
    fn lock(&self) -> Result<MutexGuard<'_, RegistryState>, RunCancellationError> {
        self.state
            .lock()
            .map_err(|_| RunCancellationError::Poisoned)
    }
}

impl RegistryState {
    fn allocate_generation(&mut self) -> Result<NonZeroU64, RunCancellationError> {
        let generation = NonZeroU64::new(self.next_generation)
            .ok_or(RunCancellationError::GenerationExhausted)?;
        let next_generation = self
            .next_generation
            .checked_add(1)
            .ok_or(RunCancellationError::GenerationExhausted)?;
        self.next_generation = next_generation;
        Ok(generation)
    }
}

/// Cloneable, bounded registry of currently live native runs.
///
/// The registry stores only live registrations. Dropping or explicitly
/// unregistering a lease removes its entry and returns its slot; no tombstone
/// or historical run id is retained. The generation counter is one bounded
/// scalar and never wraps, so a stale lease cannot remove a later registration
/// that happens to use the same exact ids.
#[derive(Clone)]
pub struct RunCancellationRegistry {
    inner: Arc<RegistryInner>,
}

impl fmt::Debug for RunCancellationRegistry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RunCancellationRegistry")
            .field("capacity", &self.capacity())
            .finish_non_exhaustive()
    }
}

impl RunCancellationRegistry {
    /// Creates an empty registry with the fixed finite live-run capacity.
    ///
    /// # Errors
    ///
    /// Returns [`RunCancellationError::ZeroCapacity`] when `capacity` is
    /// zero. The capacity cannot change after construction.
    pub fn new(capacity: usize) -> Result<Self, RunCancellationError> {
        if capacity == 0 {
            return Err(RunCancellationError::ZeroCapacity);
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

    /// Registers one exact live `(thread_id, run_id)` pair.
    ///
    /// Success returns a non-cloneable RAII lease and a fresh independent
    /// [`CancelHandle`] held by that lease. A duplicate exact pair is rejected
    /// before capacity is considered; a different pair is rejected when all
    /// live slots are occupied. The generation is allocated only after those
    /// checks and is advanced with checked arithmetic, so a rejected attempt
    /// never changes the table.
    ///
    /// # Errors
    ///
    /// Returns [`RunCancellationError::Duplicate`],
    /// [`RunCancellationError::AtCapacity`],
    /// [`RunCancellationError::GenerationExhausted`], or
    /// [`RunCancellationError::Poisoned`].
    #[allow(clippy::map_entry)]
    pub fn register(
        &self,
        thread_id: ThreadId,
        run_id: RunId,
    ) -> Result<RunCancellationLease, RunCancellationError> {
        let key = (thread_id.clone(), run_id.clone());
        let mut state = self.inner.lock()?;

        if state.active.contains_key(&key) {
            return Err(RunCancellationError::Duplicate);
        }
        if state.active.len() >= self.inner.capacity {
            return Err(RunCancellationError::AtCapacity {
                capacity: self.inner.capacity,
            });
        }

        let generation = state.allocate_generation()?;
        let cancel = Arc::new(CancelHandle::new());
        state.active.insert(
            key,
            ActiveRun {
                generation,
                cancel: Arc::clone(&cancel),
                signalled: false,
            },
        );

        Ok(RunCancellationLease {
            inner: Arc::clone(&self.inner),
            thread_id,
            run_id,
            generation,
            cancel,
            registered: true,
        })
    }

    /// Requests cancellation of exactly `(thread_id, run_id)`.
    ///
    /// The lookup, one-shot state transition, and handle clone happen while
    /// holding the short synchronous lock. The actual handle notification is
    /// published after releasing it. Consequently, a concurrent lease drop
    /// either wins and returns [`CancelRequestOutcome::NotActive`], or the
    /// request owns the old handle and can never signal a later replacement's
    /// independent handle.
    ///
    /// # Errors
    ///
    /// Returns [`RunCancellationError::Poisoned`] without signaling when the
    /// registry lock is poisoned.
    pub fn request_cancel(
        &self,
        thread_id: &ThreadId,
        run_id: &RunId,
    ) -> Result<CancelRequestOutcome, RunCancellationError> {
        let key = (thread_id.clone(), run_id.clone());
        let cancel = {
            let mut state = self.inner.lock()?;
            let Some(active) = state.active.get_mut(&key) else {
                return Ok(CancelRequestOutcome::NotActive);
            };
            if active.signalled {
                return Ok(CancelRequestOutcome::AlreadySignalled);
            }
            active.signalled = true;
            Arc::clone(&active.cancel)
        };

        cancel.cancel();
        Ok(CancelRequestOutcome::Signalled)
    }

    /// Returns the sole live run for `thread_id`, if one exists.
    ///
    /// Multiple active runs are deliberately not ordered or guessed: callers
    /// receive [`RunCancellationError::AmbiguousActiveRuns`] and must fail
    /// closed until the registry returns to a singleton or empty state.
    ///
    /// # Errors
    ///
    /// Returns [`RunCancellationError::Poisoned`] when the registry lock is
    /// poisoned, and [`RunCancellationError::AmbiguousActiveRuns`] when more
    /// than one run is active for the thread.
    pub fn active_run(&self, thread_id: &ThreadId) -> Result<Option<RunId>, RunCancellationError> {
        let state = self.inner.lock()?;
        let mut active = state
            .active
            .keys()
            .filter(|(active_thread, _)| active_thread == thread_id)
            .map(|(_, run_id)| run_id.clone());
        let Some(run_id) = active.next() else {
            return Ok(None);
        };
        if active.next().is_some() {
            return Err(RunCancellationError::AmbiguousActiveRuns);
        }
        Ok(Some(run_id))
    }
}

/// RAII ownership of one live cancellation registration.
///
/// This type intentionally does not implement [`Clone`]. Cloning the
/// registry or any [`Arc<CancelHandle>`] obtained from this lease does not
/// release it; only this lease's drop or explicit unregister can evict the
/// exact generation it owns.
pub struct RunCancellationLease {
    inner: Arc<RegistryInner>,
    thread_id: ThreadId,
    run_id: RunId,
    generation: NonZeroU64,
    cancel: Arc<CancelHandle>,
    registered: bool,
}

impl fmt::Debug for RunCancellationLease {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RunCancellationLease")
            .field("generation", &self.generation)
            .field("registered", &self.registered)
            .finish_non_exhaustive()
    }
}

impl RunCancellationLease {
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

    /// Returns a clone of this registration's independent cancellation handle.
    ///
    /// The returned `Arc` keeps the signal alive for an observing run owner,
    /// but it does not keep the registry entry live and it does not unregister
    /// this lease when dropped.
    #[must_use]
    pub fn cancel_handle(&self) -> Arc<CancelHandle> {
        Arc::clone(&self.cancel)
    }

    /// Unregisters this exact generation and returns its live slot.
    ///
    /// A generation mismatch is treated as a successful no-op: an old lease
    /// is never allowed to remove a newer registration for the same ids. A
    /// poisoned lock returns [`RunCancellationError::Poisoned`] and leaves
    /// the lease armed for its best-effort, non-panicking `Drop` path.
    ///
    /// # Errors
    ///
    /// Returns [`RunCancellationError::Poisoned`] when the registry lock is
    /// poisoned.
    pub fn unregister(mut self) -> Result<(), RunCancellationError> {
        self.release()
    }

    fn release(&mut self) -> Result<(), RunCancellationError> {
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

impl Drop for RunCancellationLease {
    fn drop(&mut self) {
        // Drop cannot report an error. In particular, it must never panic on
        // a poisoned lock; retaining the entry is the fail-closed outcome.
        let _ = self.release();
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{Arc, Barrier},
        thread,
    };

    use artisan_domain::{RunId, ThreadId};

    use super::{CancelRequestOutcome, RunCancellationError, RunCancellationRegistry};

    fn thread_id(value: &str) -> ThreadId {
        ThreadId::parse(value).expect("fixture thread id should be valid")
    }

    fn run_id(value: &str) -> RunId {
        RunId::parse(value).expect("fixture run id should be valid")
    }

    fn registry(capacity: usize) -> RunCancellationRegistry {
        RunCancellationRegistry::new(capacity).expect("fixture capacity should be valid")
    }

    #[test]
    fn zero_capacity_is_rejected() {
        assert!(matches!(
            RunCancellationRegistry::new(0),
            Err(RunCancellationError::ZeroCapacity)
        ));
    }

    #[test]
    fn exact_run_signals_its_handle_and_second_request_is_already_signalled() {
        let registry = registry(2);
        let thread_id = thread_id("thread-exact");
        let run_id = run_id("run-exact");
        let lease = registry
            .register(thread_id.clone(), run_id.clone())
            .expect("exact run should register");
        let handle = lease.cancel_handle();

        assert_eq!(
            registry.request_cancel(&thread_id, &run_id),
            Ok(CancelRequestOutcome::Signalled)
        );
        assert!(handle.is_cancelled());
        assert_eq!(
            registry.request_cancel(&thread_id, &run_id),
            Ok(CancelRequestOutcome::AlreadySignalled)
        );
    }

    #[test]
    fn wrong_thread_and_stale_run_do_not_signal_the_exact_handle() {
        let registry = registry(2);
        let owner_thread = thread_id("thread-owner");
        let live_run = run_id("run-live");
        let lease = registry
            .register(owner_thread.clone(), live_run.clone())
            .expect("live run should register");
        let handle = lease.cancel_handle();

        assert_eq!(
            registry.request_cancel(&thread_id("thread-other"), &live_run),
            Ok(CancelRequestOutcome::NotActive)
        );
        assert_eq!(
            registry.request_cancel(&owner_thread, &run_id("run-stale")),
            Ok(CancelRequestOutcome::NotActive)
        );
        assert!(!handle.is_cancelled());
    }

    #[test]
    fn active_run_query_returns_empty_singleton_and_fails_closed_on_ambiguity() {
        let registry = registry(3);
        let thread_id = thread_id("thread-active-query");
        let active_run_id = run_id("run-active-query");

        assert_eq!(registry.active_run(&thread_id), Ok(None));
        let first = registry
            .register(thread_id.clone(), active_run_id.clone())
            .expect("active run should register");
        assert_eq!(registry.active_run(&thread_id), Ok(Some(active_run_id)));

        let second = registry
            .register(thread_id.clone(), run_id("run-active-query-2"))
            .expect("second fixture run should register");
        assert_eq!(
            registry.active_run(&thread_id),
            Err(RunCancellationError::AmbiguousActiveRuns)
        );
        drop(second);
        drop(first);
        assert_eq!(registry.active_run(&thread_id), Ok(None));
    }

    #[test]
    fn duplicate_live_registration_is_rejected_before_capacity() {
        let registry = registry(1);
        let thread_id = thread_id("thread-duplicate");
        let run_id = run_id("run-duplicate");
        let _lease = registry
            .register(thread_id.clone(), run_id.clone())
            .expect("first run should register");

        assert!(matches!(
            registry.register(thread_id, run_id),
            Err(RunCancellationError::Duplicate)
        ));
    }

    #[test]
    fn full_capacity_rejects_a_different_live_run() {
        let registry = registry(1);
        let _lease = registry
            .register(thread_id("thread-full-a"), run_id("run-full-a"))
            .expect("first run should register");

        assert!(matches!(
            registry.register(thread_id("thread-full-b"), run_id("run-full-b")),
            Err(RunCancellationError::AtCapacity { capacity: 1 })
        ));
    }

    #[test]
    fn dropping_a_guard_returns_capacity_and_advances_generation() {
        let registry = registry(1);
        let thread_id = thread_id("thread-drop");
        let run_id = run_id("run-drop");
        let first = registry
            .register(thread_id.clone(), run_id.clone())
            .expect("first run should register");
        let first_generation = first.generation();
        drop(first);

        let replacement = registry
            .register(thread_id, run_id)
            .expect("dropped run should free its slot");
        assert!(replacement.generation() > first_generation);
    }

    #[test]
    fn old_run_cancellation_cannot_signal_a_new_run() {
        let registry = registry(1);
        let thread_id = thread_id("thread-replacement");
        let old_run = run_id("run-old");
        let old = registry
            .register(thread_id.clone(), old_run.clone())
            .expect("old run should register");
        let old_handle = old.cancel_handle();
        drop(old);

        let new_run = run_id("run-new");
        let new = registry
            .register(thread_id.clone(), new_run.clone())
            .expect("new run should register");
        let new_handle = new.cancel_handle();
        assert!(!Arc::ptr_eq(&old_handle, &new_handle));

        assert_eq!(
            registry.request_cancel(&thread_id, &old_run),
            Ok(CancelRequestOutcome::NotActive)
        );
        assert!(!old_handle.is_cancelled());
        assert!(!new_handle.is_cancelled());
        assert_eq!(
            registry.request_cancel(&thread_id, &new_run),
            Ok(CancelRequestOutcome::Signalled)
        );
        assert!(new_handle.is_cancelled());
        assert!(!old_handle.is_cancelled());
    }

    #[test]
    fn stale_guard_cannot_remove_a_newer_generation() {
        let registry = registry(1);
        let thread_id = thread_id("thread-generation");
        let run_id = run_id("run-generation");
        let old = registry
            .register(thread_id.clone(), run_id.clone())
            .expect("old run should register");
        let old_generation = old.generation();
        let replacement_handle = Arc::new(artisan_transport::CancelHandle::new());

        // Public registration rejects a duplicate while `old` is live. This
        // installs the replacement directly only to exercise the private
        // generation fence at the exact stale-guard boundary.
        {
            let mut state = registry.inner.state.lock().expect("test lock");
            let replacement_generation = old_generation
                .get()
                .checked_add(1)
                .and_then(std::num::NonZeroU64::new)
                .expect("fixture generation should be representable");
            state.active.insert(
                (thread_id.clone(), run_id.clone()),
                super::ActiveRun {
                    generation: replacement_generation,
                    cancel: Arc::clone(&replacement_handle),
                    signalled: false,
                },
            );
        }

        old.unregister()
            .expect("stale release is a successful no-op");
        assert_eq!(
            registry.request_cancel(&thread_id, &run_id),
            Ok(CancelRequestOutcome::Signalled)
        );
        assert!(replacement_handle.is_cancelled());
    }

    #[test]
    fn generation_exhaustion_is_checked_without_wrapping() {
        let registry = registry(1);
        {
            let mut state = registry.inner.state.lock().expect("test lock");
            state.next_generation = u64::MAX;
        }

        assert!(matches!(
            registry.register(thread_id("thread-exhausted"), run_id("run-exhausted")),
            Err(RunCancellationError::GenerationExhausted)
        ));
    }

    #[test]
    fn registry_and_handle_clones_do_not_release_the_live_registration() {
        let registry = registry(1);
        let registry_clone = registry.clone();
        let thread_id = thread_id("thread-clone");
        let run_id = run_id("run-clone");
        let lease = registry
            .register(thread_id.clone(), run_id.clone())
            .expect("run should register");
        let handle = lease.cancel_handle();
        let handle_clone = Arc::clone(&handle);

        drop(registry_clone);
        drop(handle_clone);
        assert!(matches!(
            registry.register(thread_id, run_id),
            Err(RunCancellationError::Duplicate)
        ));
    }

    #[test]
    fn concurrent_request_and_drop_are_serialized_without_cross_cancellation() {
        let registry = registry(1);
        let race_thread = thread_id("thread-race");
        let race_run = run_id("run-race");
        let lease = registry
            .register(race_thread.clone(), race_run.clone())
            .expect("run should register");
        let old_handle = lease.cancel_handle();
        let barrier = Arc::new(Barrier::new(2));

        let request_registry = registry.clone();
        let request_barrier = Arc::clone(&barrier);
        let request_thread = thread::spawn(move || {
            request_barrier.wait();
            request_registry.request_cancel(&race_thread, &race_run)
        });

        let drop_barrier = Arc::clone(&barrier);
        let drop_thread = thread::spawn(move || {
            drop_barrier.wait();
            drop(lease);
        });

        let request_result = request_thread.join().expect("request thread should finish");
        drop_thread.join().expect("drop thread should finish");
        assert!(matches!(
            request_result,
            Ok(CancelRequestOutcome::Signalled | CancelRequestOutcome::NotActive)
        ));
        match request_result {
            Ok(CancelRequestOutcome::Signalled) => assert!(old_handle.is_cancelled()),
            Ok(CancelRequestOutcome::NotActive) => assert!(!old_handle.is_cancelled()),
            _ => unreachable!("the concurrent request has only two valid outcomes"),
        }

        let replacement = registry
            .register(thread_id("thread-race"), run_id("run-race"))
            .expect("drop should make a slot available");
        let replacement_handle = replacement.cancel_handle();
        assert!(!replacement_handle.is_cancelled());
    }

    #[test]
    fn poisoned_lock_fails_closed_without_signalling() {
        let registry = registry(1);
        let poison_thread = thread_id("thread-poison");
        let poison_run = run_id("run-poison");
        let lease = registry
            .register(poison_thread.clone(), poison_run.clone())
            .expect("run should register");
        let handle = lease.cancel_handle();
        let inner = Arc::clone(&registry.inner);

        let poisoner = thread::spawn(move || {
            let _state = inner.state.lock().expect("test lock");
            panic!("intentionally poison the test mutex");
        });
        assert!(poisoner.join().is_err());

        assert_eq!(
            registry.request_cancel(&poison_thread, &poison_run),
            Err(RunCancellationError::Poisoned)
        );
        assert!(!handle.is_cancelled());
        assert!(matches!(
            registry.register(thread_id("thread-poison-other"), run_id("run-poison-other")),
            Err(RunCancellationError::Poisoned)
        ));
    }
}
