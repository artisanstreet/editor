//! Process-wide, instance-owned conversation commit wake hints.
//!
//! A notifier is intended to be injected into the process-wide backend
//! composition. It carries no durable cursor or commit data: subscribers use
//! a wake only as a reason to perform their own durable re-read. Each live
//! thread has one bounded Tokio watch state, and every subscription receives
//! an independent view of that state.

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::num::NonZeroU64;
use std::sync::{Arc, Mutex, TryLockError, Weak};

use artisan_domain::ThreadId;
use tokio::sync::watch;

/// Process-wide notifier state owned by one injected notifier instance.
#[derive(Debug)]
struct Registry {
    state: Mutex<RegistryState>,
}

impl Registry {
    fn new() -> Self {
        Self {
            state: Mutex::new(RegistryState::new()),
        }
    }
}

/// Mutable state protected by the registry's short synchronous critical
/// section.
#[derive(Debug)]
struct RegistryState {
    entries: HashMap<ThreadId, Entry>,
    next_generation: u64,
}

impl RegistryState {
    fn new() -> Self {
        Self {
            entries: HashMap::new(),
            next_generation: 1,
        }
    }
}

/// One retained wake state for a thread with at least one live subscription.
#[derive(Debug)]
struct Entry {
    generation: NonZeroU64,
    sender: watch::Sender<()>,
    subscriber_count: usize,
}

/// A process-wide, instance-owned source of coalescing conversation commit
/// wake hints.
#[derive(Clone, Debug)]
pub struct ConversationCommitNotifier {
    registry: Arc<Registry>,
}

/// A subscription to coalescing conversation commit wake hints for one
/// thread.
///
/// The subscription does not own the notifier registry. Dropping the final
/// notifier owner therefore closes the bounded receiver and causes a pending
/// or subsequent wait to return [`ConversationCommitWaitError::Closed`].
#[must_use]
#[derive(Debug)]
pub struct ConversationCommitSubscription {
    registry: Weak<Registry>,
    thread_id: ThreadId,
    generation: NonZeroU64,
    receiver: watch::Receiver<()>,
}

/// The result of publishing one process-local wake hint.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConversationCommitPublish {
    /// A live matching entry retained the wake for its subscribers.
    Notified,
    /// No live subscription was registered for the thread.
    Unobserved,
    /// The registry was briefly occupied, so the hint was dropped.
    Coalesced,
}

/// Failure while registering a conversation commit subscription.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ConversationCommitSubscribeError {
    /// The monotonic entry-generation space cannot mint another entry.
    #[error("conversation commit subscription generation exhausted")]
    GenerationExhausted,
}

/// Failure while waiting for a conversation commit wake hint.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ConversationCommitWaitError {
    /// The final notifier owner was dropped and closed the receiver.
    #[error("conversation commit notifier closed")]
    Closed,
}

impl Default for ConversationCommitNotifier {
    fn default() -> Self {
        Self::new()
    }
}

impl ConversationCommitNotifier {
    /// Creates an independent notifier registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            registry: Arc::new(Registry::new()),
        }
    }

    /// Registers interest in exact `thread_id` before returning.
    ///
    /// A second live subscription for the same thread shares the existing
    /// bounded wake state while retaining an independent watch receiver. A
    /// new entry receives the next strictly monotonic, nonzero generation.
    ///
    /// # Errors
    ///
    /// Returns [`ConversationCommitSubscribeError::GenerationExhausted`]
    /// without mutating the registry when no new entry generation remains.
    #[must_use]
    pub fn subscribe(
        &self,
        thread_id: ThreadId,
    ) -> Result<ConversationCommitSubscription, ConversationCommitSubscribeError> {
        let mut state = self
            .registry
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        if let Some((generation, receiver)) = state.entries.get_mut(&thread_id).map(|entry| {
            entry.subscriber_count += 1;
            (entry.generation, entry.sender.subscribe())
        }) {
            Ok(ConversationCommitSubscription::new(
                &self.registry,
                thread_id.clone(),
                generation,
                receiver,
            ))
        } else {
            let generation_value = state.next_generation;
            let Some(generation) = NonZeroU64::new(generation_value) else {
                return Err(ConversationCommitSubscribeError::GenerationExhausted);
            };

            state.next_generation = generation_value.checked_add(1).unwrap_or_default();
            let (sender, receiver) = watch::channel(());
            state.entries.insert(
                thread_id.clone(),
                Entry {
                    generation,
                    sender,
                    subscriber_count: 1,
                },
            );
            Ok(ConversationCommitSubscription::new(
                &self.registry,
                thread_id,
                generation,
                receiver,
            ))
        }
    }

    /// Publishes a synchronous, bounded wake hint for exact `thread_id`.
    ///
    /// The registry lock is acquired with `try_lock`: an occupied critical
    /// section never makes the committer wait, and the hint is reported as
    /// [`ConversationCommitPublish::Coalesced`] instead. A Tokio watch state
    /// retains at most its current value, so repeated publishes do not build a
    /// per-commit queue.
    #[must_use]
    pub fn publish(&self, thread_id: &ThreadId) -> ConversationCommitPublish {
        let state = match self.registry.state.try_lock() {
            Ok(state) => state,
            Err(TryLockError::WouldBlock) => return ConversationCommitPublish::Coalesced,
            Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
        };

        let Some(entry) = state.entries.get(thread_id) else {
            return ConversationCommitPublish::Unobserved;
        };

        if entry.sender.send(()).is_ok() {
            ConversationCommitPublish::Notified
        } else {
            // The entry invariant normally makes this unreachable: its
            // sender is retained while every subscription receiver is live.
            ConversationCommitPublish::Unobserved
        }
    }
}

impl ConversationCommitSubscription {
    fn new(
        registry: &Arc<Registry>,
        thread_id: ThreadId,
        generation: NonZeroU64,
        receiver: watch::Receiver<()>,
    ) -> Self {
        Self {
            registry: Arc::downgrade(registry),
            thread_id,
            generation,
            receiver,
        }
    }

    /// Returns the exact thread for which this subscription receives hints.
    #[must_use]
    pub fn thread_id(&self) -> &ThreadId {
        &self.thread_id
    }

    /// Waits for the next unobserved coalesced wake generation.
    ///
    /// Tokio watch change detection is cancellation safe: cancelling this
    /// borrowed wait before it completes leaves the receiver's observed
    /// generation unchanged. A successful wait observes the current watch
    /// generation, so another wait remains pending until a later publish.
    ///
    /// # Errors
    ///
    /// Returns [`ConversationCommitWaitError::Closed`] after the final
    /// notifier owner is dropped.
    pub async fn wait(&mut self) -> Result<(), ConversationCommitWaitError> {
        self.receiver
            .changed()
            .await
            .map_err(|_| ConversationCommitWaitError::Closed)
    }
}

impl Drop for ConversationCommitSubscription {
    fn drop(&mut self) {
        let Some(registry) = self.registry.upgrade() else {
            return;
        };

        let mut state = registry
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let remove_entry = match state.entries.get_mut(&self.thread_id) {
            Some(entry) if entry.generation == self.generation => {
                if entry.subscriber_count <= 1 {
                    true
                } else {
                    entry.subscriber_count -= 1;
                    false
                }
            }
            _ => false,
        };

        if remove_entry {
            state.entries.remove(&self.thread_id);
        }
    }
}
