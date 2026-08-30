//! Pure synchronous steering placement state machine.
//!
//! One controller per steering submission. The outer aggregate retains any
//! number of these controllers concurrently, keyed by immutable command
//! identity. This module does no network I/O, snapshot replay, timers,
//! rendering, or GPUI work.
//!
//! # Statig 0.4.1 blocking hierarchical chart
//!
//! The chart is implemented with `statig::blocking`:
//!
//! ```text
//! active-submission (superstate)
//!   ├── PendingLip — optimistic lip at composer edge
//!   ├── Dispatching — outer dispatch effect in flight
//!   ├── AwaitingProjection — accepted, durable user item not yet visible
//!   └── AwaitingAcknowledgement — exact ItemId anchor known, label after it
//! settled (superstate, sealed)
//!   ├── Acknowledged
//!   ├── Failed
//!   └── Cancelled
//! ```
//!
//! The public controller owns the generated `StateMachine<SteeringInner>` and
//! dispatches every accepted event through real handlers returning
//! `Outcome::Transition(State::...)`. `phase` and `placement` are derived from
//! `machine.state()`; there is no parallel `phase` field that can diverge.
//! Immutable submission data and anchor/timestamps live in the shared storage
//! (`SteeringInner`) and mutate only inside handlers. Effects are produced via
//! safe external context (`&mut VecDeque<SteeringEffect>`).
//!
//! # Invariant — lip retention while awaiting projection
//!
//! The composer pending lip remains visible through `PendingLip`,
//! `Dispatching`, and `AwaitingProjection`. It is released only when the
//! exact `ItemId` anchor is observed (`AwaitingAcknowledgement`) or when the
//! controller settles. The renderer therefore shows:
//!
//! - `ComposerPendingLip` while awaiting projection (no label gap),
//! - `AnchoredAfter` once anchored,
//! - `SettledHidden` / `Failed` when settled.
//!
//! This retains optimism until the durable echo proves projection, mirroring
//! `steering-stages.ts` where `TakeUp` releases the lip only when the durable
//! item appears.
//!
//! # Fencing
//!
//! Every asynchronous completion event carries `(command_id, generation)`.
//! Mismatch is a typed refusal that leaves state and outbox unchanged.
//! Anchoring is immutable: a second different `ItemId` is `AnchorConflict`.
//! Duplicate equal anchor/completion is idempotent. All caller-supplied signed
//! millisecond timestamps must be monotonically non-decreasing; regression is
//! refused atomically. No clocks, sleeps, tasks, channels, callbacks, `unsafe`,
//! or GPUI types are used.

#![allow(clippy::module_name_repetitions)]

use std::collections::VecDeque;
use std::fmt;

use artisan_domain::{ItemId, RequestId};
use statig::blocking::{IntoStateMachineExt, StateMachine};
use statig::prelude::*;

// ---------------------------------------------------------------------------
// Label kind (redacted, never raw prompt)
// ---------------------------------------------------------------------------

/// Redacted display label kind. Never carries raw prompt text.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SteeringLabelKind {
    /// Generic steering label.
    Steering,
}

impl fmt::Display for SteeringLabelKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Steering => formatter.write_str("steering"),
        }
    }
}

// ---------------------------------------------------------------------------
// Bounded validated source reference (UTF-8 bytes, nonempty, no whitespace/control)
// ---------------------------------------------------------------------------

/// Validated source reference sent to Forge.
///
/// Bounds: nonempty, no Unicode whitespace/control, at most
/// `SOURCE_REFERENCE_MAX_BYTES` UTF-8 bytes. The ceiling matches
/// `IDENTIFIER_MAX_BYTES` (128) and keeps the routing value bounded while
/// preserving compatibility with opaque Forge-minted references. The check is
/// documented here so callers can reason about the byte ceiling without
/// reading the validator body.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct SourceReference(String);

/// Maximum UTF-8 bytes for a source reference.
pub const SOURCE_REFERENCE_MAX_BYTES: usize = 128;

impl SourceReference {
    /// Parses and validates a source reference.
    ///
    /// # Errors
    ///
    /// Returns `SteeringControllerError::EmptySourceReference` when empty, or
    /// `SteeringControllerError::InvalidSourceReference` when the value
    /// contains whitespace/control or exceeds the byte ceiling.
    pub fn parse(value: impl Into<String>) -> Result<Self, SteeringControllerError> {
        let value = value.into();
        if value.is_empty() {
            return Err(SteeringControllerError::EmptySourceReference);
        }
        if value
            .chars()
            .any(|ch| ch.is_whitespace() || ch.is_control())
        {
            return Err(SteeringControllerError::InvalidSourceReference(
                value.clone(),
            ));
        }
        if value.len() > SOURCE_REFERENCE_MAX_BYTES {
            return Err(SteeringControllerError::InvalidSourceReference(value));
        }
        Ok(Self(value))
    }

    /// Borrows the validated reference text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SourceReference {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

// ---------------------------------------------------------------------------
// Closed redacted failure kind (no prompt/provider/tool text)
// ---------------------------------------------------------------------------

/// Closed redacted failure kind. No raw prompt, provider, or tool text enters
/// state, effects, `Debug`, or the renderer view.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SteeringFailureKind {
    /// Dispatch transport/outer effect failed.
    Dispatch,
    /// Projection wait failed or timed out.
    Projection,
    /// Engine rejected the steering.
    Rejected,
}

impl fmt::Display for SteeringFailureKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Dispatch => formatter.write_str("dispatch"),
            Self::Projection => formatter.write_str("projection"),
            Self::Rejected => formatter.write_str("rejected"),
        }
    }
}

// ---------------------------------------------------------------------------
// Phase / placement / view
// ---------------------------------------------------------------------------

/// Renderer-visible phase, including both superstates.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SteeringPhase {
    /// Optimistic submission still at composer edge.
    PendingLip,
    /// Outer dispatch effect in flight.
    Dispatching,
    /// Accepted but durable user item not yet visible.
    AwaitingProjection,
    /// Exact durable anchor known; label renders after that item.
    AwaitingAcknowledgement,
    /// Engine acknowledged — settled.
    Acknowledged,
    /// Terminal failure — settled.
    Failed,
    /// Cancelled — settled.
    Cancelled,
}

impl SteeringPhase {
    /// Whether this phase belongs to the settled superstate (sealed).
    #[must_use]
    pub const fn is_settled(self) -> bool {
        matches!(self, Self::Acknowledged | Self::Failed | Self::Cancelled)
    }

    /// Whether this phase belongs to the active-submission superstate.
    #[must_use]
    pub const fn is_active(self) -> bool {
        !self.is_settled()
    }
}

/// Closed placement enum consumed by the pure renderer. The renderer must not
/// recombine booleans to derive placement; this enum is the single source.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SteeringPlacement {
    /// Optimistic lip at the composer.
    ComposerPendingLip,
    /// Accepted but not yet anchored — no visible steering label.
    ///
    /// Under the chosen invariant this variant is currently not emitted for
    /// `AwaitingProjection`; the lip is retained instead. The variant exists
    /// so the renderer vocabulary remains closed and the invariant choice is
    /// reversible without changing the enum.
    NoVisibleLabel,
    /// Steering label renders immediately after the exact `ItemId`.
    AnchoredAfter {
        /// Durable anchor.
        anchor: ItemId,
    },
    /// Settled and hidden.
    SettledHidden,
    /// Bounded failed presentation retained for the renderer.
    Failed {
        /// Redacted failure kind.
        kind: SteeringFailureKind,
    },
}

/// Immutable view returned to the renderer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SteeringView {
    /// Immutable command identity.
    pub command_id: RequestId,
    /// Immutable submission generation (nonzero).
    pub generation: u64,
    /// Exact source reference sent to Forge.
    pub source_reference: SourceReference,
    /// Redacted label kind.
    pub label_kind: SteeringLabelKind,
    /// Current phase derived from the Statig state.
    pub phase: SteeringPhase,
    /// Closed placement.
    pub placement: SteeringPlacement,
    /// Exact anchor when known.
    pub anchor: Option<ItemId>,
    /// Caller-supplied start timestamp (signed ms).
    pub started_at_ms: i64,
    /// Settlement timestamp when settled.
    pub settled_at_ms: Option<i64>,
    /// Number of pending effects in the outbox.
    pub pending_effect_count: usize,
}

// ---------------------------------------------------------------------------
// Effects (bounded identity/routing only)
// ---------------------------------------------------------------------------

/// Typed effects ordered in a drainable outbox. Effects carry only bounded
/// identity/routing data, never raw prompt text, credentials, or provider
/// payloads.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SteeringEffect {
    /// Dispatch the steering submission to the outer effect.
    Dispatch {
        /// Command identity.
        command_id: RequestId,
        /// Generation.
        generation: u64,
        /// Exact source reference (bounded).
        source_reference: SourceReference,
    },
    /// Watch/wait for the exact source reference to appear in the projection.
    WatchProjection {
        /// Command identity.
        command_id: RequestId,
        /// Generation.
        generation: u64,
        /// Exact source reference to watch.
        source_reference: SourceReference,
    },
    /// Release the pending composer lip for this generation only.
    ReleasePendingLip {
        /// Owning generation.
        generation: u64,
    },
    /// Request renderer invalidation.
    RenderInvalidation {
        /// Command identity (for routing, not payload).
        command_id: RequestId,
        /// Generation.
        generation: u64,
    },
    /// Optional acknowledgement watch for the anchored item.
    WatchAcknowledgement {
        /// Command identity.
        command_id: RequestId,
        /// Generation.
        generation: u64,
        /// Anchored item to watch.
        anchor: ItemId,
    },
}

// ---------------------------------------------------------------------------
// Events (fenced by command_id + generation)
// ---------------------------------------------------------------------------

/// Closed typed event vocabulary. Every asynchronous completion carries
/// `(command_id, generation)` for fencing.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SteeringEvent {
    /// Outer effect should start dispatch.
    DispatchStarted {
        /// Command identity.
        command_id: RequestId,
        /// Generation.
        generation: u64,
        /// Caller timestamp (signed ms).
        at_ms: i64,
    },
    /// Outer effect accepted the submission.
    DispatchAccepted {
        /// Command identity.
        command_id: RequestId,
        /// Generation.
        generation: u64,
        /// Caller timestamp.
        at_ms: i64,
    },
    /// Dispatch failed.
    DispatchFailed {
        /// Command identity.
        command_id: RequestId,
        /// Generation.
        generation: u64,
        /// Caller timestamp.
        at_ms: i64,
        /// Redacted failure kind.
        kind: SteeringFailureKind,
    },
    /// Durable user item for this source reference became visible.
    DurableItemAnchored {
        /// Command identity.
        command_id: RequestId,
        /// Generation.
        generation: u64,
        /// Exact durable anchor.
        item_id: ItemId,
        /// Caller timestamp.
        at_ms: i64,
    },
    /// Engine acknowledged the steering.
    EngineAcknowledged {
        /// Command identity.
        command_id: RequestId,
        /// Generation.
        generation: u64,
        /// Caller timestamp.
        at_ms: i64,
    },
    /// Steering cancelled.
    Cancelled {
        /// Command identity.
        command_id: RequestId,
        /// Generation.
        generation: u64,
        /// Caller timestamp.
        at_ms: i64,
    },
}

impl SteeringEvent {
    fn command_id(&self) -> &RequestId {
        match self {
            Self::DispatchStarted { command_id, .. }
            | Self::DispatchAccepted { command_id, .. }
            | Self::DispatchFailed { command_id, .. }
            | Self::DurableItemAnchored { command_id, .. }
            | Self::EngineAcknowledged { command_id, .. }
            | Self::Cancelled { command_id, .. } => command_id,
        }
    }

    fn generation(&self) -> u64 {
        match self {
            Self::DispatchStarted { generation, .. }
            | Self::DispatchAccepted { generation, .. }
            | Self::DispatchFailed { generation, .. }
            | Self::DurableItemAnchored { generation, .. }
            | Self::EngineAcknowledged { generation, .. }
            | Self::Cancelled { generation, .. } => *generation,
        }
    }

    fn at_ms(&self) -> i64 {
        match self {
            Self::DispatchStarted { at_ms, .. }
            | Self::DispatchAccepted { at_ms, .. }
            | Self::DispatchFailed { at_ms, .. }
            | Self::DurableItemAnchored { at_ms, .. }
            | Self::EngineAcknowledged { at_ms, .. }
            | Self::Cancelled { at_ms, .. } => *at_ms,
        }
    }

    fn kind_str(&self) -> &'static str {
        match self {
            Self::DispatchStarted { .. } => "DispatchStarted",
            Self::DispatchAccepted { .. } => "DispatchAccepted",
            Self::DispatchFailed { .. } => "DispatchFailed",
            Self::DurableItemAnchored { .. } => "DurableItemAnchored",
            Self::EngineAcknowledged { .. } => "EngineAcknowledged",
            Self::Cancelled { .. } => "Cancelled",
        }
    }
}

// ---------------------------------------------------------------------------
// Rejections / construction errors
// ---------------------------------------------------------------------------

/// Why an event was refused. Refusals leave state and outbox unchanged.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SteeringRejection {
    /// Command id or generation mismatch / stale event.
    StaleCommandOrGeneration {
        /// Expected command id.
        expected_command_id: RequestId,
        /// Expected generation.
        expected_generation: u64,
        /// Received command id.
        got_command_id: RequestId,
        /// Received generation.
        got_generation: u64,
    },
    /// Second anchor differs from the immutable first anchor.
    AnchorConflict {
        /// Existing anchor.
        existing: ItemId,
        /// Attempted anchor.
        attempted: ItemId,
    },
    /// Caller timestamp regressed.
    TimestampRegression {
        /// Last observed timestamp.
        last_observed_ms: i64,
        /// Attempted timestamp.
        attempted_ms: i64,
    },
    /// Transition is invalid from current phase.
    InvalidTransition {
        /// Current phase.
        from: SteeringPhase,
        /// Event kind.
        event: String,
    },
    /// Already settled — only exact duplicate completion is allowed.
    AlreadySettled {
        /// Settled phase.
        phase: SteeringPhase,
    },
}

impl fmt::Display for SteeringRejection {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::StaleCommandOrGeneration {
                expected_command_id,
                expected_generation,
                got_command_id,
                got_generation,
            } => write!(
                formatter,
                "stale command/generation: expected {expected_command_id}/{expected_generation}, got {got_command_id}/{got_generation}"
            ),
            Self::AnchorConflict {
                existing,
                attempted,
            } => write!(
                formatter,
                "anchor conflict: existing {existing}, attempted {attempted}"
            ),
            Self::TimestampRegression {
                last_observed_ms,
                attempted_ms,
            } => write!(
                formatter,
                "timestamp regression: last {last_observed_ms}, attempted {attempted_ms}"
            ),
            Self::InvalidTransition { from, event } => {
                write!(formatter, "invalid transition from {from:?} for {event}")
            }
            Self::AlreadySettled { phase } => write!(formatter, "already settled in {phase:?}"),
        }
    }
}

impl std::error::Error for SteeringRejection {}

/// Why controller construction failed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SteeringControllerError {
    /// Command id was empty, whitespace, control, or too long.
    InvalidCommandId(String),
    /// Generation was zero.
    InvalidGeneration,
    /// Source reference was empty.
    EmptySourceReference,
    /// Source reference contains whitespace/control or exceeds byte ceiling.
    InvalidSourceReference(String),
}

impl fmt::Display for SteeringControllerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidCommandId(value) => write!(formatter, "invalid command id: {value:?}"),
            Self::InvalidGeneration => formatter.write_str("generation must be nonzero"),
            Self::EmptySourceReference => formatter.write_str("source reference must be nonempty"),
            Self::InvalidSourceReference(value) => {
                write!(formatter, "invalid source reference: {value:?}")
            }
        }
    }
}

impl std::error::Error for SteeringControllerError {}

// ---------------------------------------------------------------------------
// Statig shared storage + hierarchical chart
// ---------------------------------------------------------------------------

/// Shared storage for one steering submission. Immutable command identity,
/// generation, source reference, and label kind never change after creation.
/// Anchor, settlement, timestamps, and failure kind mutate only inside real
/// Statig handlers.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SteeringInner {
    command_id: RequestId,
    generation: u64,
    source_reference: SourceReference,
    label_kind: SteeringLabelKind,
    started_at_ms: i64,
    last_observed_ms: i64,
    anchor: Option<ItemId>,
    settled_at_ms: Option<i64>,
    failure_kind: Option<SteeringFailureKind>,
}

#[state_machine(
    initial = "State::pending_lip()",
    state(derive(Debug, Clone, PartialEq)),
    superstate(derive(Debug))
)]
impl SteeringInner {
    #[state(superstate = "active_submission")]
    fn pending_lip(
        &mut self,
        event: &SteeringEvent,
        context: &mut VecDeque<SteeringEffect>,
    ) -> Outcome<State> {
        match event {
            SteeringEvent::DispatchStarted { at_ms, .. } => {
                self.last_observed_ms = *at_ms;
                context.push_back(SteeringEffect::Dispatch {
                    command_id: self.command_id.clone(),
                    generation: self.generation,
                    source_reference: self.source_reference.clone(),
                });
                context.push_back(SteeringEffect::WatchProjection {
                    command_id: self.command_id.clone(),
                    generation: self.generation,
                    source_reference: self.source_reference.clone(),
                });
                context.push_back(SteeringEffect::RenderInvalidation {
                    command_id: self.command_id.clone(),
                    generation: self.generation,
                });
                Transition(State::dispatching())
            }
            _ => Handled,
        }
    }

    #[state(superstate = "active_submission")]
    fn dispatching(
        &mut self,
        event: &SteeringEvent,
        context: &mut VecDeque<SteeringEffect>,
    ) -> Outcome<State> {
        match event {
            SteeringEvent::DispatchAccepted { at_ms, .. } => {
                self.last_observed_ms = *at_ms;
                context.push_back(SteeringEffect::RenderInvalidation {
                    command_id: self.command_id.clone(),
                    generation: self.generation,
                });
                Transition(State::awaiting_projection())
            }
            SteeringEvent::DispatchFailed { at_ms, kind, .. } => {
                self.last_observed_ms = *at_ms;
                self.settled_at_ms = Some(*at_ms);
                self.failure_kind = Some(*kind);
                context.push_back(SteeringEffect::ReleasePendingLip {
                    generation: self.generation,
                });
                context.push_back(SteeringEffect::RenderInvalidation {
                    command_id: self.command_id.clone(),
                    generation: self.generation,
                });
                Transition(State::failed())
            }
            SteeringEvent::Cancelled { at_ms, .. } => {
                self.last_observed_ms = *at_ms;
                self.settled_at_ms = Some(*at_ms);
                context.push_back(SteeringEffect::ReleasePendingLip {
                    generation: self.generation,
                });
                context.push_back(SteeringEffect::RenderInvalidation {
                    command_id: self.command_id.clone(),
                    generation: self.generation,
                });
                Transition(State::cancelled())
            }
            _ => Handled,
        }
    }

    #[state(superstate = "active_submission")]
    fn awaiting_projection(
        &mut self,
        event: &SteeringEvent,
        context: &mut VecDeque<SteeringEffect>,
    ) -> Outcome<State> {
        match event {
            SteeringEvent::DurableItemAnchored { item_id, at_ms, .. } => {
                // Anchoring is immutable — duplicate equal is idempotent,
                // different is handled as refusal before dispatch. This path
                // receives only the first valid anchor.
                self.last_observed_ms = *at_ms;
                self.anchor = Some(item_id.clone());
                context.push_back(SteeringEffect::RenderInvalidation {
                    command_id: self.command_id.clone(),
                    generation: self.generation,
                });
                context.push_back(SteeringEffect::WatchAcknowledgement {
                    command_id: self.command_id.clone(),
                    generation: self.generation,
                    anchor: item_id.clone(),
                });
                Transition(State::awaiting_acknowledgement())
            }
            SteeringEvent::EngineAcknowledged { at_ms, .. } => {
                // Acknowledgement before anchor — settle without fabricating anchor.
                self.last_observed_ms = *at_ms;
                self.settled_at_ms = Some(*at_ms);
                context.push_back(SteeringEffect::ReleasePendingLip {
                    generation: self.generation,
                });
                context.push_back(SteeringEffect::RenderInvalidation {
                    command_id: self.command_id.clone(),
                    generation: self.generation,
                });
                Transition(State::acknowledged())
            }
            SteeringEvent::DispatchFailed { at_ms, kind, .. } => {
                self.last_observed_ms = *at_ms;
                self.settled_at_ms = Some(*at_ms);
                self.failure_kind = Some(*kind);
                context.push_back(SteeringEffect::ReleasePendingLip {
                    generation: self.generation,
                });
                context.push_back(SteeringEffect::RenderInvalidation {
                    command_id: self.command_id.clone(),
                    generation: self.generation,
                });
                Transition(State::failed())
            }
            SteeringEvent::Cancelled { at_ms, .. } => {
                self.last_observed_ms = *at_ms;
                self.settled_at_ms = Some(*at_ms);
                context.push_back(SteeringEffect::ReleasePendingLip {
                    generation: self.generation,
                });
                context.push_back(SteeringEffect::RenderInvalidation {
                    command_id: self.command_id.clone(),
                    generation: self.generation,
                });
                Transition(State::cancelled())
            }
            _ => Handled,
        }
    }

    #[state(superstate = "active_submission")]
    fn awaiting_acknowledgement(
        &mut self,
        event: &SteeringEvent,
        context: &mut VecDeque<SteeringEffect>,
    ) -> Outcome<State> {
        match event {
            SteeringEvent::EngineAcknowledged { at_ms, .. } => {
                self.last_observed_ms = *at_ms;
                self.settled_at_ms = Some(*at_ms);
                context.push_back(SteeringEffect::ReleasePendingLip {
                    generation: self.generation,
                });
                context.push_back(SteeringEffect::RenderInvalidation {
                    command_id: self.command_id.clone(),
                    generation: self.generation,
                });
                Transition(State::acknowledged())
            }
            SteeringEvent::DispatchFailed { at_ms, kind, .. } => {
                self.last_observed_ms = *at_ms;
                self.settled_at_ms = Some(*at_ms);
                self.failure_kind = Some(*kind);
                // Failure after anchoring must not move the label — anchor unchanged.
                context.push_back(SteeringEffect::ReleasePendingLip {
                    generation: self.generation,
                });
                context.push_back(SteeringEffect::RenderInvalidation {
                    command_id: self.command_id.clone(),
                    generation: self.generation,
                });
                Transition(State::failed())
            }
            SteeringEvent::Cancelled { at_ms, .. } => {
                self.last_observed_ms = *at_ms;
                self.settled_at_ms = Some(*at_ms);
                context.push_back(SteeringEffect::ReleasePendingLip {
                    generation: self.generation,
                });
                context.push_back(SteeringEffect::RenderInvalidation {
                    command_id: self.command_id.clone(),
                    generation: self.generation,
                });
                Transition(State::cancelled())
            }
            SteeringEvent::DurableItemAnchored { at_ms, .. } => {
                // Duplicate equal anchor is idempotent — advance timestamp without new effects.
                self.last_observed_ms = (*at_ms).max(self.last_observed_ms);
                Handled
            }
            _ => Handled,
        }
    }

    #[state(superstate = "settled")]
    fn acknowledged(&mut self, event: &SteeringEvent) -> Outcome<State> {
        match event {
            SteeringEvent::EngineAcknowledged { at_ms, .. } => {
                self.last_observed_ms = (*at_ms).max(self.last_observed_ms);
                Handled
            }
            SteeringEvent::DurableItemAnchored { item_id, at_ms, .. } => {
                if self.anchor.as_ref() == Some(item_id) {
                    self.last_observed_ms = (*at_ms).max(self.last_observed_ms);
                    Handled
                } else {
                    // Anchor conflict after settlement is refused externally; handler
                    // stays handled without transition.
                    Handled
                }
            }
            _ => Handled,
        }
    }

    #[state(superstate = "settled")]
    fn failed(&mut self, event: &SteeringEvent) -> Outcome<State> {
        match event {
            SteeringEvent::DispatchFailed { at_ms, .. } => {
                self.last_observed_ms = (*at_ms).max(self.last_observed_ms);
                Handled
            }
            SteeringEvent::DurableItemAnchored { item_id, at_ms, .. } => {
                if self.anchor.as_ref() == Some(item_id) {
                    self.last_observed_ms = (*at_ms).max(self.last_observed_ms);
                    Handled
                } else {
                    Handled
                }
            }
            _ => Handled,
        }
    }

    #[state(superstate = "settled")]
    fn cancelled(&mut self, event: &SteeringEvent) -> Outcome<State> {
        match event {
            SteeringEvent::Cancelled { at_ms, .. } => {
                self.last_observed_ms = (*at_ms).max(self.last_observed_ms);
                Handled
            }
            SteeringEvent::DurableItemAnchored { item_id, at_ms, .. } => {
                if self.anchor.as_ref() == Some(item_id) {
                    self.last_observed_ms = (*at_ms).max(self.last_observed_ms);
                    Handled
                } else {
                    Handled
                }
            }
            _ => Handled,
        }
    }

    #[superstate]
    fn active_submission() -> Outcome<State> {
        Handled
    }

    #[superstate]
    fn settled() -> Outcome<State> {
        Handled
    }
}

// ---------------------------------------------------------------------------
// Public controller — owns the generated StateMachine + external outbox
// ---------------------------------------------------------------------------

/// Pure synchronous steering controller. The hierarchical Statig chart is
/// hidden inside; callers use only this public surface and the drainable
/// effect outbox.
pub struct ConversationSteeringMachine {
    machine: StateMachine<SteeringInner>,
    outbox: VecDeque<SteeringEffect>,
}

impl ConversationSteeringMachine {
    /// Creates a controller for one steering submission.
    ///
    /// # Errors
    ///
    /// Returns [`SteeringControllerError`] if the command id is invalid per
    /// `RequestId`, generation is zero, or source reference is empty/invalid.
    pub fn new(
        command_id: impl Into<String>,
        generation: u64,
        source_reference: impl Into<String>,
        started_at_ms: i64,
        label_kind: SteeringLabelKind,
    ) -> Result<Self, SteeringControllerError> {
        let command_id_str = command_id.into();
        let command_id = RequestId::parse(command_id_str.clone())
            .map_err(|_| SteeringControllerError::InvalidCommandId(command_id_str))?;
        if generation == 0 {
            return Err(SteeringControllerError::InvalidGeneration);
        }
        let source_reference = SourceReference::parse(source_reference)?;
        let inner = SteeringInner {
            command_id,
            generation,
            source_reference,
            label_kind,
            started_at_ms,
            last_observed_ms: started_at_ms,
            anchor: None,
            settled_at_ms: None,
            failure_kind: None,
        };
        let machine = inner.state_machine();
        Ok(Self {
            machine,
            outbox: VecDeque::new(),
        })
    }

    /// Returns the immutable command id.
    #[must_use]
    pub fn command_id(&self) -> &RequestId {
        &self.machine.inner().command_id
    }

    /// Returns the immutable generation.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.machine.inner().generation
    }

    /// Returns the exact source reference sent to Forge.
    #[must_use]
    pub fn source_reference(&self) -> &SourceReference {
        &self.machine.inner().source_reference
    }

    /// Returns the redacted label kind.
    #[must_use]
    pub fn label_kind(&self) -> SteeringLabelKind {
        self.machine.inner().label_kind
    }

    /// Returns the start timestamp.
    #[must_use]
    pub fn started_at_ms(&self) -> i64 {
        self.machine.inner().started_at_ms
    }

    /// Returns the exact anchor when known.
    #[must_use]
    pub fn anchor(&self) -> Option<&ItemId> {
        self.machine.inner().anchor.as_ref()
    }

    /// Returns current phase derived from the Statig state.
    #[must_use]
    pub fn phase(&self) -> SteeringPhase {
        phase_from_state(self.machine.state())
    }

    /// Returns the number of pending effects.
    #[must_use]
    pub fn pending_effect_count(&self) -> usize {
        self.outbox.len()
    }

    /// Returns a snapshot of pending effects without draining.
    #[must_use]
    pub fn pending_effects(&self) -> Vec<SteeringEffect> {
        self.outbox.iter().cloned().collect()
    }

    /// Drains the ordered outbox.
    pub fn drain_effects(&mut self) -> Vec<SteeringEffect> {
        self.outbox.drain(..).collect()
    }

    /// Immutable renderer view. Placement is derived solely from the Statig
    /// state and anchor; callers must not recombine booleans.
    #[must_use]
    pub fn view(&self) -> SteeringView {
        let inner = self.machine.inner();
        let phase = phase_from_state(self.machine.state());
        let placement = match phase {
            SteeringPhase::PendingLip
            | SteeringPhase::Dispatching
            | SteeringPhase::AwaitingProjection => SteeringPlacement::ComposerPendingLip,
            SteeringPhase::AwaitingAcknowledgement => {
                if let Some(anchor) = inner.anchor.clone() {
                    SteeringPlacement::AnchoredAfter { anchor }
                } else {
                    SteeringPlacement::ComposerPendingLip
                }
            }
            SteeringPhase::Acknowledged | SteeringPhase::Cancelled => {
                SteeringPlacement::SettledHidden
            }
            SteeringPhase::Failed => {
                if let Some(kind) = inner.failure_kind {
                    SteeringPlacement::Failed { kind }
                } else {
                    SteeringPlacement::Failed {
                        kind: SteeringFailureKind::Dispatch,
                    }
                }
            }
        };
        SteeringView {
            command_id: inner.command_id.clone(),
            generation: inner.generation,
            source_reference: inner.source_reference.clone(),
            label_kind: inner.label_kind,
            phase,
            placement,
            anchor: inner.anchor.clone(),
            started_at_ms: inner.started_at_ms,
            settled_at_ms: inner.settled_at_ms,
            pending_effect_count: self.outbox.len(),
        }
    }

    /// Handles one typed event. Stale/mismatched, anchor-conflict, timestamp-
    /// regression, and invalid-transition refusals leave state and outbox
    /// unchanged.
    ///
    /// # Errors
    ///
    /// Returns [`SteeringRejection`] on refusal. Idempotent duplicate
    /// anchor/completion returns `Ok(())` with no additional effects.
    pub fn handle_event(&mut self, event: &SteeringEvent) -> Result<(), SteeringRejection> {
        let inner = self.machine.inner();
        // Fencing: immutable identity must match exactly.
        if event.command_id() != &inner.command_id || event.generation() != inner.generation {
            return Err(SteeringRejection::StaleCommandOrGeneration {
                expected_command_id: inner.command_id.clone(),
                expected_generation: inner.generation,
                got_command_id: event.command_id().clone(),
                got_generation: event.generation(),
            });
        }

        // Timestamp regression — atomic refusal.
        let at_ms = event.at_ms();
        if at_ms < inner.last_observed_ms {
            return Err(SteeringRejection::TimestampRegression {
                last_observed_ms: inner.last_observed_ms,
                attempted_ms: at_ms,
            });
        }

        // Anchor immutability: second different ItemId is a typed conflict.
        // Duplicate equal anchor is allowed — handler will advance timestamp.
        if let SteeringEvent::DurableItemAnchored { item_id, .. } = event
            && let Some(existing) = &inner.anchor
            && existing != item_id
        {
            return Err(SteeringRejection::AnchorConflict {
                existing: existing.clone(),
                attempted: item_id.clone(),
            });
        }

        // Sealed settled states: only exact duplicate completion is allowed.
        let phase = phase_from_state(self.machine.state());
        if phase.is_settled() {
            let allowed = match (phase, event) {
                (SteeringPhase::Acknowledged, SteeringEvent::EngineAcknowledged { .. })
                | (SteeringPhase::Failed, SteeringEvent::DispatchFailed { .. })
                | (SteeringPhase::Cancelled, SteeringEvent::Cancelled { .. }) => true,
                (_, SteeringEvent::DurableItemAnchored { item_id, .. }) => {
                    // Duplicate equal anchor is idempotent even when settled.
                    inner.anchor.as_ref() == Some(item_id)
                }
                _ => false,
            };
            if !allowed {
                // For anchor conflict already handled above; otherwise already settled.
                if let SteeringEvent::DurableItemAnchored { .. } = event {
                    // Equal case handled, different already returned AnchorConflict.
                    return Err(SteeringRejection::InvalidTransition {
                        from: phase,
                        event: event.kind_str().to_owned(),
                    });
                }
                return Err(SteeringRejection::AlreadySettled { phase });
            }
        }

        // Validate transition exists from current phase for this event.
        if !is_valid_transition(phase, event, inner.anchor.is_some()) {
            // Duplicate equal anchor already allowed; this is truly invalid.
            return Err(SteeringRejection::InvalidTransition {
                from: phase,
                event: event.kind_str().to_owned(),
            });
        }

        // All accepted state changes occur inside real Statig handlers.
        self.machine.handle_with_context(event, &mut self.outbox);
        Ok(())
    }
}

fn phase_from_state(state: &State) -> SteeringPhase {
    match state {
        State::PendingLip { .. } => SteeringPhase::PendingLip,
        State::Dispatching { .. } => SteeringPhase::Dispatching,
        State::AwaitingProjection { .. } => SteeringPhase::AwaitingProjection,
        State::AwaitingAcknowledgement { .. } => SteeringPhase::AwaitingAcknowledgement,
        State::Acknowledged { .. } => SteeringPhase::Acknowledged,
        State::Failed { .. } => SteeringPhase::Failed,
        State::Cancelled { .. } => SteeringPhase::Cancelled,
    }
}

fn is_valid_transition(phase: SteeringPhase, event: &SteeringEvent, has_anchor: bool) -> bool {
    let is_anchor_event = matches!(event, SteeringEvent::DurableItemAnchored { .. });
    match phase {
        SteeringPhase::PendingLip => matches!(event, SteeringEvent::DispatchStarted { .. }),
        SteeringPhase::Dispatching => matches!(
            event,
            SteeringEvent::DispatchAccepted { .. }
                | SteeringEvent::DispatchFailed { .. }
                | SteeringEvent::Cancelled { .. }
        ),
        SteeringPhase::AwaitingProjection => matches!(
            event,
            SteeringEvent::DurableItemAnchored { .. }
                | SteeringEvent::EngineAcknowledged { .. }
                | SteeringEvent::DispatchFailed { .. }
                | SteeringEvent::Cancelled { .. }
        ),
        SteeringPhase::AwaitingAcknowledgement => {
            matches!(
                event,
                SteeringEvent::EngineAcknowledged { .. }
                    | SteeringEvent::DispatchFailed { .. }
                    | SteeringEvent::Cancelled { .. }
            ) || is_anchor_event && has_anchor
        }
        SteeringPhase::Acknowledged => {
            matches!(event, SteeringEvent::EngineAcknowledged { .. })
                || is_anchor_event && has_anchor
        }
        SteeringPhase::Failed => {
            matches!(event, SteeringEvent::DispatchFailed { .. }) || is_anchor_event && has_anchor
        }
        SteeringPhase::Cancelled => {
            matches!(event, SteeringEvent::Cancelled { .. }) || is_anchor_event && has_anchor
        }
    }
}

// ---------------------------------------------------------------------------
// Convenience alias
// ---------------------------------------------------------------------------

/// Alias for ergonomic import in tests.
pub type SteeringController = ConversationSteeringMachine;
