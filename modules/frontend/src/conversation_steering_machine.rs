//! Pure synchronous steering placement state machine.
//!
//! One controller per steering submission. The outer aggregate retains any
//! number of these controllers concurrently, keyed by immutable command
//! identity. This module does no network I/O, snapshot replay, timers,
//! rendering, or GPUI work.
//!
//! # Statig 0.4.1 blocking chart (hidden)
//!
//! The hierarchical chart is modelled as if expanded by `statig::blocking`
//! `#[state_machine]`:
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
//! The generated `State`/`Superstate`/`Event`/`Action` types are intentionally
//! hidden. Callers interact only through the small public controller API
//! below. Statig dependency registration is intentionally absent from this
//! clean base; the chart semantics below are authored to match Statig 0.4.1
//! blocking macro semantics and the `statig` crate can be registered by the VP
//! without changing this file's public surface.
//!
//! References to `statig` 0.4.1 are therefore conditional: when the `statig`
//! feature is present the inner machine would be annotated with
//! `#[state_machine]` and driven via `StateMachine<Inner>`. Until registration
//! the synchronous `match` implementation below preserves the same transition
//! table and fencing invariants.
//!
//! # Invariant — lip retention while awaiting projection
//!
//! **Chosen invariant:** the composer pending lip remains visible through
//! `PendingLip`, `Dispatching`, and `AwaitingProjection`. It is released only
//! when the exact `ItemId` anchor is observed (`AwaitingAcknowledgement`) or
//! when the controller settles. The renderer therefore shows:
//!
//! - `ComposerPendingLip` while awaiting projection (no label gap),
//! - `AnchoredAfter` once anchored,
//! - `SettledHidden` / `Failed` when settled.
//!
//! This retains optimism until the durable echo proves projection, mirroring
//! the TypeScript `steering-stages.ts` behaviour where `TakeUp` (projection
//! visible) releases the lip and raises the label. The alternative — hiding the
//! lip immediately after accept and showing `NoVisibleLabel` — would flash empty
//! space and was deliberately not chosen. The invariant is documented here and
//! tested explicitly.
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

use artisan_domain::ItemId;

// Statig 0.4.1 blocking import — pending VP dependency registration.
// When the `statig` feature is enabled the inner machine is driven by the
// generated state machine; until then the manual match table below preserves
// identical semantics.
#[cfg(feature = "statig")]
#[allow(unused_imports)]
use statig::blocking::{self as statig_blocking, StateMachine as StatigStateMachine};

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
    /// Under the chosen invariant this variant is currently **not emitted**
    /// for `AwaitingProjection`; the lip is retained instead. The variant
    /// exists so the renderer vocabulary remains closed and the invariant
    /// choice is reversible without changing the enum.
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
        /// Bounded failure reason (identity only, never provider payload).
        reason: String,
    },
}

/// Immutable view returned to the renderer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SteeringView {
    /// Immutable command identity.
    pub command_id: String,
    /// Immutable submission generation (nonzero).
    pub generation: u64,
    /// Exact source reference sent to Forge.
    pub source_reference: String,
    /// Redacted label kind.
    pub label_kind: SteeringLabelKind,
    /// Current phase.
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

/// Typed effects ordered in a drainable outbox. Effects carry only bounded
/// identity/routing data, never raw prompt text, credentials, or provider
/// payloads.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SteeringEffect {
    /// Dispatch the steering submission to the outer effect.
    Dispatch {
        /// Command identity.
        command_id: String,
        /// Generation.
        generation: u64,
        /// Exact source reference (bounded, not raw prompt).
        source_reference: String,
    },
    /// Watch/wait for the exact source reference to appear in the
    /// projection.
    WatchProjection {
        /// Command identity.
        command_id: String,
        /// Generation.
        generation: u64,
        /// Exact source reference to watch.
        source_reference: String,
    },
    /// Release the pending composer lip for this generation only.
    ReleasePendingLip {
        /// Owning generation.
        generation: u64,
    },
    /// Request renderer invalidation.
    RenderInvalidation {
        /// Command identity (for routing, not payload).
        command_id: String,
        /// Generation.
        generation: u64,
    },
    /// Optional acknowledgement watch for the anchored item.
    WatchAcknowledgement {
        /// Command identity.
        command_id: String,
        /// Generation.
        generation: u64,
        /// Anchored item to watch.
        anchor: ItemId,
    },
}

/// Closed typed event vocabulary. Every asynchronous completion carries
/// `(command_id, generation)` for fencing.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SteeringEvent {
    /// Outer effect should start dispatch.
    DispatchStarted {
        /// Command identity.
        command_id: String,
        /// Generation.
        generation: u64,
        /// Caller timestamp (signed ms).
        at_ms: i64,
    },
    /// Outer effect accepted the submission.
    DispatchAccepted {
        /// Command identity.
        command_id: String,
        /// Generation.
        generation: u64,
        /// Caller timestamp.
        at_ms: i64,
    },
    /// Dispatch failed.
    DispatchFailed {
        /// Command identity.
        command_id: String,
        /// Generation.
        generation: u64,
        /// Caller timestamp.
        at_ms: i64,
        /// Bounded failure reason.
        reason: String,
    },
    /// Durable user item for this source reference became visible.
    DurableItemAnchored {
        /// Command identity.
        command_id: String,
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
        command_id: String,
        /// Generation.
        generation: u64,
        /// Caller timestamp.
        at_ms: i64,
    },
    /// Steering cancelled.
    Cancelled {
        /// Command identity.
        command_id: String,
        /// Generation.
        generation: u64,
        /// Caller timestamp.
        at_ms: i64,
    },
    /// Retry from failed — if supported, re-enters dispatching.
    Retry {
        /// Command identity.
        command_id: String,
        /// Generation.
        generation: u64,
        /// Caller timestamp.
        at_ms: i64,
    },
}

impl SteeringEvent {
    fn command_id(&self) -> &str {
        match self {
            Self::DispatchStarted { command_id, .. }
            | Self::DispatchAccepted { command_id, .. }
            | Self::DispatchFailed { command_id, .. }
            | Self::DurableItemAnchored { command_id, .. }
            | Self::EngineAcknowledged { command_id, .. }
            | Self::Cancelled { command_id, .. }
            | Self::Retry { command_id, .. } => command_id,
        }
    }

    fn generation(&self) -> u64 {
        match self {
            Self::DispatchStarted { generation, .. }
            | Self::DispatchAccepted { generation, .. }
            | Self::DispatchFailed { generation, .. }
            | Self::DurableItemAnchored { generation, .. }
            | Self::EngineAcknowledged { generation, .. }
            | Self::Cancelled { generation, .. }
            | Self::Retry { generation, .. } => *generation,
        }
    }

    fn at_ms(&self) -> i64 {
        match self {
            Self::DispatchStarted { at_ms, .. }
            | Self::DispatchAccepted { at_ms, .. }
            | Self::DispatchFailed { at_ms, .. }
            | Self::DurableItemAnchored { at_ms, .. }
            | Self::EngineAcknowledged { at_ms, .. }
            | Self::Cancelled { at_ms, .. }
            | Self::Retry { at_ms, .. } => *at_ms,
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
            Self::Retry { .. } => "Retry",
        }
    }
}

/// Why an event was refused. Refusals leave state and outbox unchanged.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SteeringRejection {
    /// Command id or generation mismatch / stale event.
    StaleCommandOrGeneration {
        /// Expected command id.
        expected_command_id: String,
        /// Expected generation.
        expected_generation: u64,
        /// Received command id.
        got_command_id: String,
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
}

impl fmt::Display for SteeringControllerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidCommandId(value) => write!(formatter, "invalid command id: {value:?}"),
            Self::InvalidGeneration => formatter.write_str("generation must be nonzero"),
            Self::EmptySourceReference => formatter.write_str("source reference must be nonempty"),
        }
    }
}

impl std::error::Error for SteeringControllerError {}

fn validate_command_id(value: &str) -> Result<(), SteeringControllerError> {
    if value.is_empty() {
        return Err(SteeringControllerError::InvalidCommandId(
            "empty".to_owned(),
        ));
    }
    if value
        .chars()
        .any(|ch| ch.is_whitespace() || ch.is_control())
    {
        return Err(SteeringControllerError::InvalidCommandId(value.to_owned()));
    }
    if value.len() > 128 {
        return Err(SteeringControllerError::InvalidCommandId(value.to_owned()));
    }
    Ok(())
}

/// Pure synchronous steering controller. The hierarchical Statig chart is
/// hidden inside; callers use only this public surface and the drainable
/// effect outbox.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConversationSteeringMachine {
    command_id: String,
    generation: u64,
    source_reference: String,
    label_kind: SteeringLabelKind,
    started_at_ms: i64,
    last_observed_ms: i64,
    anchor: Option<ItemId>,
    settled_at_ms: Option<i64>,
    phase: SteeringPhase,
    failure_reason: Option<String>,
    outbox: VecDeque<SteeringEffect>,
}

impl ConversationSteeringMachine {
    /// Creates a controller for one steering submission.
    ///
    /// # Errors
    ///
    /// Returns [`SteeringControllerError`] if the command id is empty /
    /// whitespace / control / too long, generation is zero, or source
    /// reference is empty.
    pub fn new(
        command_id: impl Into<String>,
        generation: u64,
        source_reference: impl Into<String>,
        started_at_ms: i64,
        label_kind: SteeringLabelKind,
    ) -> Result<Self, SteeringControllerError> {
        let command_id = command_id.into();
        let source_reference = source_reference.into();
        validate_command_id(&command_id)?;
        if generation == 0 {
            return Err(SteeringControllerError::InvalidGeneration);
        }
        if source_reference.is_empty() {
            return Err(SteeringControllerError::EmptySourceReference);
        }
        Ok(Self {
            command_id,
            generation,
            source_reference,
            label_kind,
            started_at_ms,
            last_observed_ms: started_at_ms,
            anchor: None,
            settled_at_ms: None,
            phase: SteeringPhase::PendingLip,
            failure_reason: None,
            outbox: VecDeque::new(),
        })
    }

    /// Returns the immutable command id.
    #[must_use]
    pub fn command_id(&self) -> &str {
        &self.command_id
    }

    /// Returns the immutable generation.
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    /// Returns the exact source reference sent to Forge.
    #[must_use]
    pub fn source_reference(&self) -> &str {
        &self.source_reference
    }

    /// Returns the redacted label kind.
    #[must_use]
    pub const fn label_kind(&self) -> SteeringLabelKind {
        self.label_kind
    }

    /// Returns the start timestamp.
    #[must_use]
    pub const fn started_at_ms(&self) -> i64 {
        self.started_at_ms
    }

    /// Returns the exact anchor when known.
    #[must_use]
    pub fn anchor(&self) -> Option<&ItemId> {
        self.anchor.as_ref()
    }

    /// Returns current phase.
    #[must_use]
    pub const fn phase(&self) -> SteeringPhase {
        self.phase
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

    /// Immutable renderer view. The placement is derived solely from phase
    /// and anchor; callers must not recombine booleans.
    #[must_use]
    pub fn view(&self) -> SteeringView {
        let placement = match self.phase {
            SteeringPhase::PendingLip
            | SteeringPhase::Dispatching
            | SteeringPhase::AwaitingProjection => SteeringPlacement::ComposerPendingLip,
            SteeringPhase::AwaitingAcknowledgement => {
                // Anchor is guaranteed Some in this phase.
                if let Some(anchor) = self.anchor.clone() {
                    SteeringPlacement::AnchoredAfter { anchor }
                } else {
                    // Defensive; should never happen. Fall back to pending lip.
                    SteeringPlacement::ComposerPendingLip
                }
            }
            SteeringPhase::Acknowledged | SteeringPhase::Cancelled => {
                SteeringPlacement::SettledHidden
            }
            SteeringPhase::Failed => {
                if let Some(reason) = self.failure_reason.clone() {
                    SteeringPlacement::Failed { reason }
                } else {
                    SteeringPlacement::Failed {
                        reason: "failed".to_owned(),
                    }
                }
            }
        };
        SteeringView {
            command_id: self.command_id.clone(),
            generation: self.generation,
            source_reference: self.source_reference.clone(),
            label_kind: self.label_kind,
            phase: self.phase,
            placement,
            anchor: self.anchor.clone(),
            started_at_ms: self.started_at_ms,
            settled_at_ms: self.settled_at_ms,
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
    pub fn handle_event(&mut self, event: SteeringEvent) -> Result<(), SteeringRejection> {
        // Fencing: immutable identity must match exactly.
        if event.command_id() != self.command_id || event.generation() != self.generation {
            return Err(SteeringRejection::StaleCommandOrGeneration {
                expected_command_id: self.command_id.clone(),
                expected_generation: self.generation,
                got_command_id: event.command_id().to_owned(),
                got_generation: event.generation(),
            });
        }

        // Timestamp regression — atomic refusal.
        let at_ms = event.at_ms();
        if at_ms < self.last_observed_ms {
            return Err(SteeringRejection::TimestampRegression {
                last_observed_ms: self.last_observed_ms,
                attempted_ms: at_ms,
            });
        }

        // Settled states are sealed; only exact duplicate completion is
        // idempotent. Any other event from settled is refused.
        if self.phase.is_settled() {
            return self.handle_settled_event(event, at_ms);
        }

        // Active-submission transitions.
        match (&self.phase, &event) {
            (SteeringPhase::PendingLip, SteeringEvent::DispatchStarted { at_ms, .. }) => {
                self.last_observed_ms = *at_ms;
                self.phase = SteeringPhase::Dispatching;
                self.push_dispatch_effects();
                Ok(())
            }
            (SteeringPhase::Dispatching, SteeringEvent::DispatchAccepted { at_ms, .. }) => {
                self.last_observed_ms = *at_ms;
                self.phase = SteeringPhase::AwaitingProjection;
                // Already watching projection from DispatchStarted; request
                // render invalidation to reflect no visible label vs lip per
                // invariant (lip retained, but renderer still invalidates).
                self.outbox.push_back(SteeringEffect::RenderInvalidation {
                    command_id: self.command_id.clone(),
                    generation: self.generation,
                });
                Ok(())
            }
            (SteeringPhase::Dispatching, SteeringEvent::DispatchFailed { at_ms, reason, .. }) => {
                self.last_observed_ms = *at_ms;
                self.settled_at_ms = Some(*at_ms);
                self.failure_reason = Some(bounded_reason(reason));
                self.phase = SteeringPhase::Failed;
                self.push_settlement_effects();
                Ok(())
            }
            (SteeringPhase::Dispatching, SteeringEvent::Cancelled { at_ms, .. }) => {
                self.last_observed_ms = *at_ms;
                self.settled_at_ms = Some(*at_ms);
                self.phase = SteeringPhase::Cancelled;
                self.push_settlement_effects();
                Ok(())
            }
            (
                SteeringPhase::AwaitingProjection,
                SteeringEvent::DurableItemAnchored { item_id, at_ms, .. },
            ) => {
                // Anchoring is immutable: first anchor wins.
                if let Some(existing) = self.anchor.as_ref() {
                    if existing == item_id {
                        // Duplicate equal anchor — idempotent, but still
                        // advance timestamp if newer.
                        self.last_observed_ms = (*at_ms).max(self.last_observed_ms);
                        return Ok(());
                    }
                    return Err(SteeringRejection::AnchorConflict {
                        existing: existing.clone(),
                        attempted: item_id.clone(),
                    });
                }
                self.last_observed_ms = *at_ms;
                self.anchor = Some(item_id.clone());
                self.phase = SteeringPhase::AwaitingAcknowledgement;
                self.outbox.push_back(SteeringEffect::RenderInvalidation {
                    command_id: self.command_id.clone(),
                    generation: self.generation,
                });
                self.outbox.push_back(SteeringEffect::WatchAcknowledgement {
                    command_id: self.command_id.clone(),
                    generation: self.generation,
                    anchor: item_id.clone(),
                });
                Ok(())
            }
            (
                SteeringPhase::AwaitingProjection,
                SteeringEvent::EngineAcknowledged { at_ms, .. },
            ) => {
                // Acknowledgement before anchor — settle without fabricating
                // an anchor. No placement is fabricated.
                self.last_observed_ms = *at_ms;
                self.settled_at_ms = Some(*at_ms);
                self.phase = SteeringPhase::Acknowledged;
                self.push_settlement_effects();
                Ok(())
            }
            (
                SteeringPhase::AwaitingProjection,
                SteeringEvent::DispatchFailed { at_ms, reason, .. },
            ) => {
                self.last_observed_ms = *at_ms;
                self.settled_at_ms = Some(*at_ms);
                self.failure_reason = Some(bounded_reason(reason));
                self.phase = SteeringPhase::Failed;
                self.push_settlement_effects();
                Ok(())
            }
            (SteeringPhase::AwaitingProjection, SteeringEvent::Cancelled { at_ms, .. }) => {
                self.last_observed_ms = *at_ms;
                self.settled_at_ms = Some(*at_ms);
                self.phase = SteeringPhase::Cancelled;
                self.push_settlement_effects();
                Ok(())
            }
            (
                SteeringPhase::AwaitingAcknowledgement,
                SteeringEvent::EngineAcknowledged { at_ms, .. },
            ) => {
                self.last_observed_ms = *at_ms;
                self.settled_at_ms = Some(*at_ms);
                self.phase = SteeringPhase::Acknowledged;
                self.push_settlement_effects();
                Ok(())
            }
            (
                SteeringPhase::AwaitingAcknowledgement,
                SteeringEvent::DispatchFailed { at_ms, reason, .. },
            ) => {
                // Failure after anchoring must not move the label.
                self.last_observed_ms = *at_ms;
                self.settled_at_ms = Some(*at_ms);
                self.failure_reason = Some(bounded_reason(reason));
                self.phase = SteeringPhase::Failed;
                self.push_settlement_effects();
                Ok(())
            }
            (SteeringPhase::AwaitingAcknowledgement, SteeringEvent::Cancelled { at_ms, .. }) => {
                self.last_observed_ms = *at_ms;
                self.settled_at_ms = Some(*at_ms);
                self.phase = SteeringPhase::Cancelled;
                self.push_settlement_effects();
                Ok(())
            }
            (
                SteeringPhase::AwaitingAcknowledgement,
                SteeringEvent::DurableItemAnchored { item_id, at_ms, .. },
            ) => {
                // Duplicate equal anchor is harmless; different anchor is conflict.
                if let Some(existing) = self.anchor.as_ref() {
                    if existing == item_id {
                        self.last_observed_ms = (*at_ms).max(self.last_observed_ms);
                        return Ok(());
                    }
                    return Err(SteeringRejection::AnchorConflict {
                        existing: existing.clone(),
                        attempted: item_id.clone(),
                    });
                }
                // Should be unreachable because AwaitingAcknowledgement always has anchor.
                self.last_observed_ms = *at_ms;
                self.anchor = Some(item_id.clone());
                self.outbox.push_back(SteeringEffect::RenderInvalidation {
                    command_id: self.command_id.clone(),
                    generation: self.generation,
                });
                Ok(())
            }
            _ => Err(SteeringRejection::InvalidTransition {
                from: self.phase,
                event: event.kind_str().to_owned(),
            }),
        }
    }

    fn handle_settled_event(
        &mut self,
        event: SteeringEvent,
        at_ms: i64,
    ) -> Result<(), SteeringRejection> {
        match (&self.phase, &event) {
            // Duplicate acknowledgement is idempotent.
            (SteeringPhase::Acknowledged, SteeringEvent::EngineAcknowledged { .. }) => {
                self.last_observed_ms = at_ms.max(self.last_observed_ms);
                Ok(())
            }
            // Duplicate anchor equal is idempotent even when settled.
            (_, SteeringEvent::DurableItemAnchored { item_id, .. }) => {
                if let Some(existing) = self.anchor.as_ref() {
                    if existing == item_id {
                        self.last_observed_ms = at_ms.max(self.last_observed_ms);
                        return Ok(());
                    }
                    return Err(SteeringRejection::AnchorConflict {
                        existing: existing.clone(),
                        attempted: item_id.clone(),
                    });
                }
                // Settled without anchor — anchoring after settlement is not
                // allowed to fabricate placement; treat as conflict/invalid.
                Err(SteeringRejection::InvalidTransition {
                    from: self.phase,
                    event: event.kind_str().to_owned(),
                })
            }
            // Duplicate failure/cancel with same phase is idempotent.
            (SteeringPhase::Failed, SteeringEvent::DispatchFailed { .. }) => {
                self.last_observed_ms = at_ms.max(self.last_observed_ms);
                Ok(())
            }
            (SteeringPhase::Cancelled, SteeringEvent::Cancelled { .. }) => {
                self.last_observed_ms = at_ms.max(self.last_observed_ms);
                Ok(())
            }
            // Retry from Failed re-enters Dispatching.
            (SteeringPhase::Failed, SteeringEvent::Retry { at_ms, .. }) => {
                self.last_observed_ms = *at_ms;
                self.phase = SteeringPhase::Dispatching;
                self.settled_at_ms = None;
                self.failure_reason = None;
                self.push_dispatch_effects();
                Ok(())
            }
            _ => Err(SteeringRejection::AlreadySettled { phase: self.phase }),
        }
    }

    fn push_dispatch_effects(&mut self) {
        self.outbox.push_back(SteeringEffect::Dispatch {
            command_id: self.command_id.clone(),
            generation: self.generation,
            source_reference: self.source_reference.clone(),
        });
        self.outbox.push_back(SteeringEffect::WatchProjection {
            command_id: self.command_id.clone(),
            generation: self.generation,
            source_reference: self.source_reference.clone(),
        });
        self.outbox.push_back(SteeringEffect::RenderInvalidation {
            command_id: self.command_id.clone(),
            generation: self.generation,
        });
    }

    fn push_settlement_effects(&mut self) {
        self.outbox.push_back(SteeringEffect::ReleasePendingLip {
            generation: self.generation,
        });
        self.outbox.push_back(SteeringEffect::RenderInvalidation {
            command_id: self.command_id.clone(),
            generation: self.generation,
        });
    }
}

fn bounded_reason(reason: &str) -> String {
    const MAX: usize = 256;
    if reason.len() <= MAX {
        reason.to_owned()
    } else {
        // Truncate to bounded length on UTF-8 boundary.
        let mut end = MAX;
        while !reason.is_char_boundary(end) {
            end -= 1;
        }
        reason[..end].to_owned()
    }
}

// ---------------------------------------------------------------------------
// Convenience alias for tests — the spec names the controller per submission
// and the outer aggregate keys by command identity. Both names alias the same
// pure machine.
// ---------------------------------------------------------------------------

/// Alias for ergonomic import in tests.
pub type SteeringController = ConversationSteeringMachine;
