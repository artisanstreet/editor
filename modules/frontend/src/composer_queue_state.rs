//! Bounded application state for the Forge message outbox and run usage.
//!
//! This module is deliberately independent of GPUI widgets and the transport
//! service. The queued and failed rows are exactly the outbox the Forge
//! pushes over the thread subscription: the Editor keeps no local copy of a
//! sent message, no retry payload, and no transcript echo matching. It owns
//! only the identities and fences that keep controls events and withdrawals
//! exact, plus the run-usage read fence.
//!
//! The parent application supplies the current thread and composer
//! generation. This module never invents either value, never invents queue
//! text, and never derives context capacity from the selected model catalog.

#![forbid(unsafe_code)]
#![allow(clippy::module_name_repetitions)]

use std::{
    collections::{HashSet, VecDeque},
    fmt,
};

use artisan_domain::{
    AuthoredText, DispatchError, EngineId, EngineModelId, EngineRouteId, EngineVariantId,
    FailedMessageSummary, FailedMessageTarget, ImageAttachmentRef, MessageId, MessageOutbox,
    QueuedMessageState, QueuedMessageWithdrawalOutcome, QueuedMessageWithdrawalResult, RequestId,
    RunId, RunUsageReport, RunUsageResult, ThreadId, WithdrawQueuedMessageCommand,
};

#[path = "composer_queue_state/types.rs"]
mod types;

#[path = "composer_queue_state/state.rs"]
mod state;

#[cfg(test)]
#[path = "composer_queue_state/tests.rs"]
mod tests;

pub(crate) use state::ComposerQueueState;
pub(crate) use types::*;
