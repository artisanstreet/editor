//! Bounded application state for queued-message lip rows and run usage.
//!
//! This module is deliberately independent of GPUI widgets and the transport
//! service. It owns the identities and fences that make those two surfaces
//! safe to compose: a queue page is byte-free, a withdrawal remains visible
//! until Forge confirms it, and a recalled payload is never lost when the
//! composer changes underneath an in-flight read.
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
    AuthoredText, DispatchError, EngineModelId, EngineRouteId, EngineVariantId,
    FailedMessageListing, FailedMessageSummary, ImageAttachmentRef, MessageId,
    QueuedMessageListOrder, QueuedMessageListing, QueuedMessageWithdrawalOutcome,
    QueuedMessageWithdrawalResult, ReadRecalledMessage, RecalledMessageResult, RequestId, RunId,
    RunUsageReport, RunUsageResult, ThreadId, UnixMillis, WithdrawQueuedMessageCommand,
};

use crate::conversation_scene::TurnEngineLabel;
use crate::native_composer::ComposerRecallTarget;

// Phase-1 split submodules (see composer_queue_state/).

#[path = "composer_queue_state/types.rs"]
mod types;

#[path = "composer_queue_state/state.rs"]
mod state;

#[cfg(test)]
#[path = "composer_queue_state/tests.rs"]
mod tests;

pub(crate) use state::ComposerQueueState;
pub(crate) use types::*;
