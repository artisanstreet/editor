//! Frontend presentation state paired from engine subscription events.
//!
//! [`EngineObservationState`] is the GPUI-side boundary between an
//! already-decoded [`EngineObservationEvent`](artisan_domain::EngineObservationEvent)
//! stream and the renderer-facing rows it owns. It performs no I/O,
//! scheduling, retry, clock, logging, serialization, or payload decoding: the
//! values it pairs are the validated, sanitized S1a domain vocabulary, so no
//! provider payload beyond that boundary can reach a renderer through this
//! module.
//!
//! Pairing rules:
//!
//! - Every event names its durable [`ObservationSequence`](artisan_domain::ObservationSequence)
//!   and arrives with the one-based [`EventCursor`](artisan_protocol::EventCursor)
//!   minted by the delivery connection. Application in cursor order with
//!   duplicate suppression makes reconnect replay idempotent.
//! - Approval and question rows are keyed by their provider `approval_id` and
//!   `question_id`. A resolution settles its requested row in place and never
//!   duplicates it, so a later answer packet can attach by request id.
//! - Message and reasoning deltas accumulate per item; a completion settles
//!   the accumulated text authoritatively.
//! - Usage reports fold into [`UsageTotals`] honoring their
//!   [`UsageBasis`](artisan_domain::UsageBasis): delta counts add,
//!   cumulative counts replace, and the context-window gauge always replaces.
//! - Unknown future arms degrade to a diagnostic timeline row through
//!   [`EngineObservationState::record_unknown`] and never panic. The protocol
//!   codec already rejects unknown wire discriminants with typed errors, so
//!   this seam covers future [`Observation`](artisan_domain::Observation)
//!   variants mapped by later packets.
//!
//! This module issues no commands and performs no RPCs. Answering an approval
//! or question (the A-approve packet) attaches to the request ids retained
//! here; no answer dispatch exists on this path.

#![allow(clippy::module_name_repetitions)]

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;

use artisan_domain::{
    ApprovalKind as DomainApprovalKind, ApprovalObservation, ApprovalState as DomainApprovalState,
    EngineObservationAttribution, EngineObservationEvent, MessagePhase, Observation,
    QuestionObservation, QuestionState as DomainQuestionState, RunId, RunState, RunTerminalState,
    TerminalActivityObservation, TerminalActivityState, ThreadId, ToolAction, ToolObservation,
    TurnId, TurnState, UnixMillis, UsageBasis, UsageObservation,
};

use crate::approval_presentation::{
    ApprovalItem, ApprovalKind as PresentationKind, ApprovalPresentation,
    ApprovalRequest as PresentationRequest, ApprovalState as PresentationState,
    get_approval_presentation,
};

// Phase-1 split submodules (see engine_observation_state/).

#[path = "engine_observation_state/presentation.rs"]
mod presentation;

#[path = "engine_observation_state/state.rs"]
mod state;

pub use presentation::*;
pub use state::*;
