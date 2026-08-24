//! Durable facts emitted by the first native workflow.
//!
//! Each event records a fact that became true only after Forge durably
//! accepted its command: attachment minted a project, creation minted a
//! thread, queueing minted a message. Queuing is the terminal fact of this
//! milestone; no run lifecycle, streaming, or provider event exists yet.

use crate::model::{ProjectSummary, QueuedMessage, ThreadSummary};

/// One directory attach completed and its project identity was minted.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectAttached {
    /// Full summary of the attached project, including the minted identity.
    pub project: ProjectSummary,
}

/// One thread came into existence with its Forge-minted identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ThreadCreated {
    /// Summary of the created thread.
    pub thread: ThreadSummary,
}

/// A first message became durably queued on its thread.
///
/// Dispatch is intentionally absent: queued is an explicit durable state of
/// this milestone, not a pending transition into engine territory.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FirstMessageQueued {
    /// The queued message, including its minted message id.
    pub message: QueuedMessage,
}

/// Every durable fact of the first native workflow.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Event {
    /// See [`ProjectAttached`].
    ProjectAttached(ProjectAttached),
    /// See [`ThreadCreated`].
    ThreadCreated(ThreadCreated),
    /// See [`FirstMessageQueued`].
    FirstMessageQueued(FirstMessageQueued),
}
