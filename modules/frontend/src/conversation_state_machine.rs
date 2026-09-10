//! Bounded synchronous composition of the conversation state machines.
//!
//! [`ConversationStateController`] is the one composition owner for a rendered
//! thread.  It owns the durable delivery controller, the registered turn and
//! steering machines, disclosure machines, one viewport machine, and a small
//! typed set of non-durable scene facts.  It does not perform I/O or execute
//! any effect: callers drain [`ConversationStateEffect`] and decide how to
//! execute those effects at a boundary outside this module.
//!
//! The aggregate deliberately keeps registries and its effect outbox bounded.
//! It preflights the relevant ceiling before dispatching a child event, so a
//! full outbox or registry cannot leave a half-applied aggregate mutation.

#![allow(clippy::large_enum_variant)]
#![allow(clippy::module_name_repetitions)]

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use artisan_domain::{
    AssistantMessagePhase, ConversationItem, ConversationLifecycle, ConversationPatch,
    ConversationSnapshot, ConversationTurn, ItemId, RequestId, ThreadId, TurnId,
};
use thiserror::Error;

use crate::conversation_delivery_machine::{
    ConversationDeliveryController, ConversationDeliveryEffect, ConversationDeliveryError,
    ConversationDeliveryEvent, ConversationDeliveryView, DeliveryPhase,
};
use crate::conversation_scene::{
    AssistantPhase, ConversationScene, SceneBuildError, SceneDisclosure, SceneId, SceneIdError,
    SceneItem, SceneItemKind, SceneTurn, SteeringPlacement as SceneSteeringPlacement,
    TurnFooterSettlement, TurnNarration as SceneTurnNarration, TurnNarrationEntry,
};
use crate::conversation_steering_machine::{
    ConversationSteeringMachine, SteeringControllerError, SteeringEvent, SteeringLabelKind,
    SteeringPlacement as ChildSteeringPlacement, SteeringRejection, SteeringView,
};
use crate::conversation_turn_machine::{
    ConversationTurnController, StateKind, TurnError, TurnEvent, TurnNarration,
    TurnView as ChildTurnView,
};
use crate::conversation_view_machine::{
    Disclosure, DisclosureController, DisclosureEffect, DisclosureEvent, DisclosureState,
    ViewportController, ViewportEffect, ViewportEvent, ViewportGeneration, ViewportState,
};

/// Maximum registered turn controllers retained by one conversation owner.
pub const MAX_TURN_CONTROLLERS: usize = crate::conversation_scene::SCENE_MAX_TURNS;

/// Maximum steering controllers retained by one conversation owner.
pub const MAX_STEERING_CONTROLLERS: usize =
    crate::conversation_scene::SCENE_MAX_STEERING_PLACEMENTS;

/// Maximum disclosure controllers retained by one conversation owner.
pub const MAX_DISCLOSURE_CONTROLLERS: usize = crate::conversation_scene::SCENE_MAX_ITEMS;

/// Maximum typed non-durable facts retained by one conversation owner.
pub const MAX_SCENE_FACTS: usize = crate::conversation_scene::SCENE_MAX_ITEMS;

/// Maximum aggregate effects that may wait to be drained.
pub const MAX_PENDING_EFFECTS: usize = 4_096;

const MAX_DELIVERY_EFFECTS_PER_EVENT: usize = 2;
const MAX_STEERING_EFFECTS_PER_EVENT: usize = 3;
const MAX_VIEWPORT_EFFECTS_PER_EVENT: usize = 4;
const MAX_DISCLOSURE_EFFECTS_PER_EVENT: usize = 2;
const MAX_CLOSE_EFFECTS: usize = 2;

/// A typed bounded fact that can be projected into a scene item until its
/// durable domain vocabulary has a corresponding item kind.
///
/// Fact text is renderer-facing and bounded by the existing scene builder.
/// It is never copied into an aggregate effect or aggregate error.
#[derive(Clone, Eq, PartialEq)]
pub struct SceneFact {
    /// Stable render identity for this fact.
    pub id: SceneId,
    /// Durable turn that owns this fact.
    pub turn_id: TurnId,
    /// Global scene ordering ordinal.
    pub ordinal: u64,
    /// Closed non-durable fact kind.
    pub kind: SceneFactKind,
    /// Narrow typed event timing in signed Unix millis, when known.
    ///
    /// This is the persisted engine commit time for activity projected from
    /// retained observations, never a sampled clock or a global watermark.
    /// It orders activity without fabricating time; elapsed evidence still
    /// derives from the canonical turn's own creation/update times.
    pub observed_at_ms: Option<i64>,
    /// Whether this fact is derived from retained engine observations.
    ///
    /// Derived activity facts are rebased atomically when canonical delivery
    /// growth reuses their ordinal; manually registered facts keep
    /// conflict-on-collision semantics.
    pub derived: bool,
}

impl fmt::Debug for SceneFact {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SceneFact")
            .field("id", &self.id)
            .field("turn_id", &self.turn_id)
            .field("ordinal", &self.ordinal)
            .field("kind", &self.kind)
            .field("observed_at_ms", &self.observed_at_ms)
            .field("derived", &self.derived)
            .finish()
    }
}

impl SceneFact {
    /// Creates and validates one bounded non-durable scene fact.
    ///
    /// # Errors
    ///
    /// Returns the same bounded scene validation error used by the pure scene
    /// builder. The aggregate validates again when accepting public struct
    /// values, so invalid direct literals cannot enter owned state.
    pub fn new(
        id: SceneId,
        turn_id: TurnId,
        ordinal: u64,
        kind: SceneFactKind,
    ) -> Result<Self, SceneBuildError> {
        let fact = Self {
            id,
            turn_id,
            ordinal,
            kind,
            observed_at_ms: None,
            derived: false,
        };
        fact.as_scene_item(None)?;
        Ok(fact)
    }

    /// Attaches narrow typed event timing to this fact.
    ///
    /// The timestamp is a persisted commit time, never a sampled clock. It
    /// does not change the fact identity, owning turn, ordinal, or kind.
    #[must_use]
    pub fn with_observed_at_ms(self, observed_at_ms: i64) -> Self {
        Self {
            observed_at_ms: Some(observed_at_ms),
            ..self
        }
    }

    /// Returns the narrow typed event timing, when present.
    #[must_use]
    pub const fn observed_at_ms(&self) -> Option<i64> {
        self.observed_at_ms
    }

    /// Marks this fact as derived from retained engine observations.
    ///
    /// Derived activity facts are rebased atomically when canonical delivery
    /// growth reuses their ordinal. It does not change the fact identity,
    /// owning turn, ordinal, kind, or timing.
    #[must_use]
    pub fn with_derived(self) -> Self {
        Self {
            derived: true,
            ..self
        }
    }

    /// Returns whether this fact is derived from retained engine observations.
    #[must_use]
    pub const fn derived(&self) -> bool {
        self.derived
    }

    fn as_scene_item(
        &self,
        disclosure: Option<SceneDisclosure>,
    ) -> Result<SceneItem, SceneBuildError> {
        SceneItem::new(
            self.id.clone(),
            self.turn_id.clone(),
            self.ordinal,
            self.kind.as_scene_item_kind(),
            disclosure,
        )
    }
}

/// Closed non-durable scene fact vocabulary.
#[derive(Clone, Eq, PartialEq)]
pub enum SceneFactKind {
    /// Compaction summary card.
    Compaction { summary: String },
    /// Settled reasoning summary.
    Reasoning { body: String },
    /// Activity or tool-result summary.
    Activity { body: String },
    /// Work-session title.
    WorkSession { title: String },
    /// One changed-file set card.
    ChangedFiles {
        /// Bounded changed-file facts.
        files: Vec<crate::conversation_scene::SceneFileChange>,
    },
    /// Plan/checklist card.
    Plan {
        /// Bounded plan title.
        title: String,
        /// Bounded checklist entries.
        entries: Vec<String>,
    },
    /// Approval request card.
    Approval { prompt: String },
    /// Question card.
    Question { prompt: String },
    /// Redacted or otherwise renderer-safe error card.
    Error { message: String },
    /// Usage/provider interruption card.
    UsageInterruption { detail: String },
    /// Model transition card.
    ModelTransition {
        /// Previous model label.
        from_model: String,
        /// New model label.
        to_model: String,
    },
    /// Bounded native event/fallback fact.
    NativeFact { text: String },
}

impl fmt::Debug for SceneFactKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut structure = formatter.debug_struct(match self {
            Self::Compaction { .. } => "Compaction",
            Self::Reasoning { .. } => "Reasoning",
            Self::Activity { .. } => "Activity",
            Self::WorkSession { .. } => "WorkSession",
            Self::ChangedFiles { .. } => "ChangedFiles",
            Self::Plan { .. } => "Plan",
            Self::Approval { .. } => "Approval",
            Self::Question { .. } => "Question",
            Self::Error { .. } => "Error",
            Self::UsageInterruption { .. } => "UsageInterruption",
            Self::ModelTransition { .. } => "ModelTransition",
            Self::NativeFact { .. } => "NativeFact",
        });

        match self {
            Self::Compaction { summary }
            | Self::Reasoning { body: summary }
            | Self::Activity { body: summary }
            | Self::WorkSession { title: summary }
            | Self::Approval { prompt: summary }
            | Self::Question { prompt: summary }
            | Self::Error { message: summary }
            | Self::UsageInterruption { detail: summary }
            | Self::NativeFact { text: summary } => {
                structure.field("text_bytes", &summary.len());
            }
            Self::ChangedFiles { files } => {
                structure.field("file_count", &files.len());
            }
            Self::Plan { title, entries } => {
                structure
                    .field("title_bytes", &title.len())
                    .field("entry_count", &entries.len());
            }
            Self::ModelTransition {
                from_model,
                to_model,
            } => {
                structure
                    .field("from_model_bytes", &from_model.len())
                    .field("to_model_bytes", &to_model.len());
            }
        }
        structure.finish()
    }
}

impl SceneFactKind {
    fn as_scene_item_kind(&self) -> SceneItemKind {
        match self {
            Self::Compaction { summary } => SceneItemKind::Compaction {
                summary: summary.clone(),
            },
            Self::Reasoning { body } => SceneItemKind::ReasoningSummary { body: body.clone() },
            Self::Activity { body } => SceneItemKind::Activity { body: body.clone() },
            Self::WorkSession { title } => SceneItemKind::WorkSession {
                title: title.clone(),
            },
            Self::ChangedFiles { files } => SceneItemKind::ChangeSet {
                files: files.clone(),
            },
            Self::Plan { title, entries } => SceneItemKind::Plan {
                title: title.clone(),
                entries: entries.clone(),
            },
            Self::Approval { prompt } => SceneItemKind::Approval {
                prompt: prompt.clone(),
            },
            Self::Question { prompt } => SceneItemKind::Question {
                prompt: prompt.clone(),
            },
            Self::Error { message } => SceneItemKind::Error {
                message: message.clone(),
            },
            Self::UsageInterruption { detail } => SceneItemKind::UsageInterruption {
                detail: detail.clone(),
            },
            Self::ModelTransition {
                from_model,
                to_model,
            } => SceneItemKind::ModelTransition {
                from_model: from_model.clone(),
                to_model: to_model.clone(),
            },
            Self::NativeFact { text } => SceneItemKind::NativeFact { text: text.clone() },
        }
    }
}

/// Fact operation routed through the aggregate's bounded fact registry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SceneFactCommand {
    /// Add one new fact under its stable identity.
    Register(SceneFact),
    /// Atomically insert or update one fact under its stable identity.
    ///
    /// A repeated projection of the same retained row upserts the same id:
    /// an identical fact is a no-op, a changed fact for the same turn
    /// updates in place without a transient removal, and reassignment to a
    /// different turn is refused. The registry keeps the first accepted
    /// ordinal so repeated projections never shift scene order.
    Upsert(SceneFact),
    /// Remove one fact under its stable identity.
    Remove { id: SceneId },
}

/// Closed aggregate event vocabulary.
///
/// Child events retain their child identity and are never converted into
/// untyped maps or arbitrary payloads.
#[derive(Clone)]
pub enum ConversationStateEvent {
    /// Route one delivery event to the fixed-thread delivery controller.
    Delivery(ConversationDeliveryEvent),
    /// Register one turn state machine.
    RegisterTurn { turn_id: TurnId },
    /// Route one event to an already registered turn.
    Turn { turn_id: TurnId, event: TurnEvent },
    /// Register one exact `(RequestId, generation)` steering machine.
    RegisterSteering {
        /// Client command identity.
        command_id: RequestId,
        /// Command generation.
        generation: u64,
        /// Validated bounded Forge source reference.
        source_reference: crate::conversation_steering_machine::SourceReference,
        /// Caller-supplied start timestamp.
        started_at_ms: i64,
        /// Redacted label kind.
        label_kind: SteeringLabelKind,
    },
    /// Route one fenced event to a steering machine.
    Steering(SteeringEvent),
    /// Register one disclosure controller keyed by stable scene identity.
    RegisterDisclosure {
        /// Stable scene identity.
        scene_id: SceneId,
        /// Whether work is active at initialization time.
        initially_working: bool,
    },
    /// Route one disclosure event to a registered disclosure controller.
    Disclosure {
        /// Stable scene identity.
        scene_id: SceneId,
        /// Disclosure lifecycle or user event.
        event: DisclosureEvent,
    },
    /// Route one viewport event to the sole viewport controller.
    Viewport(ViewportEvent),
    /// Register or remove one bounded non-durable fact.
    Fact(SceneFactCommand),
    /// Close the aggregate owner and its delivery/viewport children.
    Close,
}

impl fmt::Debug for ConversationStateEvent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Delivery(ConversationDeliveryEvent::SnapshotReceived(snapshot)) => formatter
                .debug_struct("DeliverySnapshotReceived")
                .field("thread_id", snapshot.thread_id())
                .field("cursor", &snapshot.cursor())
                .field("turn_count", &snapshot.turns().len())
                .field("item_count", &snapshot.items().len())
                .finish(),
            Self::Delivery(ConversationDeliveryEvent::BatchReceived(batch)) => formatter
                .debug_struct("DeliveryBatchReceived")
                .field("thread_id", batch.thread_id())
                .field("from_cursor", &batch.from_cursor())
                .field("to_cursor", &batch.to_cursor())
                .field("patch_count", &batch.patches().len())
                .finish(),
            Self::Delivery(ConversationDeliveryEvent::SubscriptionResumed {
                thread_id,
                cursor,
            }) => formatter
                .debug_struct("DeliverySubscriptionResumed")
                .field("thread_id", thread_id)
                .field("cursor", cursor)
                .finish(),
            Self::Delivery(ConversationDeliveryEvent::RetryRequested) => {
                formatter.write_str("DeliveryRetryRequested")
            }
            Self::Delivery(ConversationDeliveryEvent::Closed) => {
                formatter.write_str("DeliveryClosed")
            }
            Self::RegisterTurn { turn_id } => formatter
                .debug_struct("RegisterTurn")
                .field("turn_id", turn_id)
                .finish(),
            Self::Turn { turn_id, event } => formatter
                .debug_struct("Turn")
                .field("turn_id", turn_id)
                .field("event", event)
                .finish(),
            Self::RegisterSteering {
                command_id,
                generation,
                source_reference,
                started_at_ms,
                label_kind,
            } => formatter
                .debug_struct("RegisterSteering")
                .field("command_id", command_id)
                .field("generation", generation)
                .field("source_reference", source_reference)
                .field("started_at_ms", started_at_ms)
                .field("label_kind", label_kind)
                .finish(),
            Self::Steering(event) => formatter.debug_tuple("Steering").field(event).finish(),
            Self::RegisterDisclosure {
                scene_id,
                initially_working,
            } => formatter
                .debug_struct("RegisterDisclosure")
                .field("scene_id", scene_id)
                .field("initially_working", initially_working)
                .finish(),
            Self::Disclosure { scene_id, event } => formatter
                .debug_struct("Disclosure")
                .field("scene_id", scene_id)
                .field("event", event)
                .finish(),
            Self::Viewport(event) => formatter.debug_tuple("Viewport").field(event).finish(),
            Self::Fact(command) => formatter.debug_tuple("Fact").field(command).finish(),
            Self::Close => formatter.write_str("Close"),
        }
    }
}

impl PartialEq for ConversationStateEvent {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Delivery(left), Self::Delivery(right)) => delivery_event_eq(left, right),
            (Self::RegisterTurn { turn_id: left }, Self::RegisterTurn { turn_id: right }) => {
                left == right
            }
            (
                Self::Turn {
                    turn_id: left_id,
                    event: left_event,
                },
                Self::Turn {
                    turn_id: right_id,
                    event: right_event,
                },
            ) => left_id == right_id && left_event == right_event,
            (
                Self::RegisterSteering {
                    command_id: left_command,
                    generation: left_generation,
                    source_reference: left_source,
                    started_at_ms: left_started,
                    label_kind: left_label,
                },
                Self::RegisterSteering {
                    command_id: right_command,
                    generation: right_generation,
                    source_reference: right_source,
                    started_at_ms: right_started,
                    label_kind: right_label,
                },
            ) => {
                left_command == right_command
                    && left_generation == right_generation
                    && left_source == right_source
                    && left_started == right_started
                    && left_label == right_label
            }
            (Self::Steering(left), Self::Steering(right)) => left == right,
            (
                Self::RegisterDisclosure {
                    scene_id: left_id,
                    initially_working: left_working,
                },
                Self::RegisterDisclosure {
                    scene_id: right_id,
                    initially_working: right_working,
                },
            ) => left_id == right_id && left_working == right_working,
            (
                Self::Disclosure {
                    scene_id: left_id,
                    event: left_event,
                },
                Self::Disclosure {
                    scene_id: right_id,
                    event: right_event,
                },
            ) => left_id == right_id && left_event == right_event,
            (Self::Viewport(left), Self::Viewport(right)) => left == right,
            (Self::Fact(left), Self::Fact(right)) => left == right,
            (Self::Close, Self::Close) => true,
            _ => false,
        }
    }
}

impl Eq for ConversationStateEvent {}

fn delivery_event_eq(left: &ConversationDeliveryEvent, right: &ConversationDeliveryEvent) -> bool {
    match (left, right) {
        (
            ConversationDeliveryEvent::SnapshotReceived(left),
            ConversationDeliveryEvent::SnapshotReceived(right),
        ) => left == right,
        (
            ConversationDeliveryEvent::BatchReceived(left),
            ConversationDeliveryEvent::BatchReceived(right),
        ) => left == right,
        (
            ConversationDeliveryEvent::SubscriptionResumed {
                thread_id: left_thread,
                cursor: left_cursor,
            },
            ConversationDeliveryEvent::SubscriptionResumed {
                thread_id: right_thread,
                cursor: right_cursor,
            },
        ) => left_thread == right_thread && left_cursor == right_cursor,
        (ConversationDeliveryEvent::RetryRequested, ConversationDeliveryEvent::RetryRequested)
        | (ConversationDeliveryEvent::Closed, ConversationDeliveryEvent::Closed) => true,
        _ => false,
    }
}

/// Command spelling for the closed aggregate event vocabulary.
pub type ConversationStateCommand = ConversationStateEvent;

/// Ergonomic alias for callers that call the aggregate an event stream.
pub type ConversationEvent = ConversationStateEvent;

/// Why a bounded aggregate registry or outbox could not accept a mutation.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum CapacityResource {
    /// Turn-controller registry.
    Turns,
    /// Steering-controller registry.
    Steerings,
    /// Disclosure-controller registry.
    Disclosures,
    /// Non-durable fact registry.
    SceneFacts,
    /// Aggregate pending-effect outbox.
    PendingEffects,
}

/// Redacted steering-construction failure exposed by the aggregate.
///
/// The child constructor has defensive error variants that may carry rejected
/// input text. The aggregate accepts already validated IDs and source
/// references, so it deliberately maps those details to this closed, payload-
/// free error before they can cross the aggregate boundary.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum SteeringConstructionError {
    /// The generation is reserved for an unregistered controller.
    #[error("steering generation must be nonzero")]
    InvalidGeneration,
    /// A defensive child validation rejected an already typed input.
    #[error("steering input was rejected")]
    InvalidInput,
}

/// Typed aggregate refusal and composition error.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum ConversationStateError {
    /// The fixed owner has already been closed.
    #[error("conversation state owner is closed")]
    OwnerClosed,
    /// A bounded aggregate resource would overflow.
    #[error("conversation {resource:?} capacity exhausted at {count}; maximum is {maximum}")]
    CapacityExhausted {
        /// Resource whose ceiling was reached.
        resource: CapacityResource,
        /// Current or prospective count.
        count: usize,
        /// Configured ceiling.
        maximum: usize,
    },
    /// A turn was registered twice.
    #[error("turn {turn_id} is already registered")]
    DuplicateTurn { turn_id: TurnId },
    /// A turn event targeted no registered turn.
    #[error("turn {turn_id} is not registered")]
    UnknownTurn { turn_id: TurnId },
    /// A steering key was registered twice.
    #[error("steering {command_id}/{generation} is already registered")]
    DuplicateSteering {
        /// Command identity.
        command_id: RequestId,
        /// Command generation.
        generation: u64,
    },
    /// A steering event targeted no registered exact key.
    #[error("steering {command_id}/{generation} is not registered")]
    UnknownSteering {
        /// Command identity.
        command_id: RequestId,
        /// Command generation.
        generation: u64,
    },
    /// A disclosure key was registered twice.
    #[error("disclosure {scene_id} is already registered")]
    DuplicateDisclosure { scene_id: SceneId },
    /// A disclosure event targeted no registered key.
    #[error("disclosure {scene_id} is not registered")]
    UnknownDisclosure { scene_id: SceneId },
    /// A fact identity was registered twice.
    #[error("scene fact {id} is already registered")]
    DuplicateFact { id: SceneId },
    /// A fact removal targeted no registered fact.
    #[error("scene fact {id} is not registered")]
    UnknownFact { id: SceneId },
    /// A fact upsert targeted a different turn than the registered fact.
    #[error("scene fact {id} belongs to turn {turn_id} and cannot move to another turn")]
    FactTurnMismatch { id: SceneId, turn_id: TurnId },
    /// A fact would collide with durable identity or global ordinal.
    #[error("scene fact {id} conflicts with durable scene state")]
    SceneConflict { id: SceneId },
    /// A steering anchor is not present in the last-good durable snapshot.
    #[error("steering anchor {anchor} is unknown")]
    UnknownSteeringAnchor { anchor: ItemId },
    /// A steering anchor is durable but is not a user-message item.
    #[error("steering anchor {anchor} is not a user message")]
    NonUserSteeringAnchor { anchor: ItemId },
    /// A previously anchored label would disappear from an accepted snapshot.
    #[error("anchored steering item {anchor} is unavailable in the snapshot")]
    SteeringAnchorUnavailable { anchor: ItemId },
    /// A synthetic steering scene identity exceeded the scene identity bound.
    #[error("steering scene identity is invalid: {error}")]
    InvalidSceneIdentity { error: SceneIdError },
    /// A known turn received an event that its child chart deliberately does
    /// not accept.
    #[error("turn {turn_id} in {state:?} cannot accept this event")]
    InvalidTurnEvent { turn_id: TurnId, state: StateKind },
    /// The delivery child rejected a request-generation allocation.
    #[error("delivery child refused the event: {0}")]
    Delivery(#[source] ConversationDeliveryError),
    /// A registered turn child rejected the event.
    #[error("turn {turn_id} refused its event: {error}")]
    Turn {
        /// Turn identity.
        turn_id: TurnId,
        /// Child refusal.
        error: TurnError,
    },
    /// A steering child rejected the event.
    #[error("steering {command_id}/{generation} refused its event: {error}")]
    Steering {
        /// Command identity.
        command_id: RequestId,
        /// Command generation.
        generation: u64,
        /// Child refusal.
        error: SteeringRejection,
    },
    /// Steering construction failed before registry mutation.
    #[error("steering construction failed: {0}")]
    SteeringConstruction(#[source] SteeringConstructionError),
    /// The pure scene builder rejected the combined bounded inputs.
    #[error("conversation scene projection failed: {0}")]
    Scene(#[source] SceneBuildError),
    /// A viewport event was sent after the viewport child was closed.
    #[error("conversation viewport is closed")]
    ViewportClosed,
}

/// One immutable turn view with the key needed by a renderer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConversationTurnView {
    /// Registered turn identity.
    pub turn_id: TurnId,
    /// Child-derived turn view.
    pub view: ChildTurnView,
}

/// One immutable steering view with its aggregate scene identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConversationSteeringView {
    /// Synthetic stable scene identity for this steering generation.
    pub scene_id: SceneId,
    /// Child-derived steering view.
    pub view: SteeringView,
}

/// One immutable disclosure view with its stable scene key.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConversationDisclosureView {
    /// Stable scene identity.
    pub scene_id: SceneId,
    /// Exact Statig disclosure state.
    pub state: DisclosureState,
    /// Closed renderer disclosure value derived from `state`.
    pub disclosure: Disclosure,
}

/// Renderer-facing immutable aggregate view.
///
/// All child views and counts are freshly derived from the owned controllers;
/// none of these fields is used as mutable aggregate state.
#[derive(Clone, Debug)]
pub struct ConversationStateView {
    /// Fixed durable delivery view.
    pub delivery: ConversationDeliveryView,
    /// Convenience copy of the current delivery phase.
    pub delivery_phase: DeliveryPhase,
    /// Convenience copy of the current delivery health.
    pub delivery_status: crate::conversation_projection::ProjectionStatus,
    /// Registered turn views in deterministic turn-id order.
    pub turn_views: Vec<ConversationTurnView>,
    /// Registered steering views in deterministic command/generation order.
    pub steering_views: Vec<ConversationSteeringView>,
    /// Active composer-lip steering views in deterministic order.
    pub pending_lip_steering_views: Vec<SteeringView>,
    /// Registered disclosure views in deterministic scene-id order.
    pub disclosure_views: Vec<ConversationDisclosureView>,
    /// Exact sole viewport state.
    pub viewport_state: ViewportState,
    /// Exact sole viewport generation.
    pub viewport_generation: ViewportGeneration,
    /// Number of aggregate effects waiting to be drained.
    pub pending_effect_count: usize,
    /// Number of registered typed non-durable facts.
    pub scene_fact_count: usize,
    /// Whether the fixed owner is closed, derived from delivery state.
    pub closed: bool,
}

impl PartialEq for ConversationStateView {
    fn eq(&self, other: &Self) -> bool {
        self.delivery.phase == other.delivery.phase
            && self.delivery.thread_id == other.delivery.thread_id
            && self.delivery.projection_status == other.delivery.projection_status
            && self.delivery.cursor == other.delivery.cursor
            && self.delivery.has_snapshot == other.delivery.has_snapshot
            && self.delivery.pending_effects == other.delivery.pending_effects
            && self.delivery_phase == other.delivery_phase
            && self.delivery_status == other.delivery_status
            && self.turn_views == other.turn_views
            && self.steering_views == other.steering_views
            && self.pending_lip_steering_views == other.pending_lip_steering_views
            && self.disclosure_views == other.disclosure_views
            && self.viewport_state == other.viewport_state
            && self.viewport_generation == other.viewport_generation
            && self.pending_effect_count == other.pending_effect_count
            && self.scene_fact_count == other.scene_fact_count
            && self.closed == other.closed
    }
}

impl Eq for ConversationStateView {}

/// One keyed aggregate effect. No variant executes I/O.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConversationStateEffect {
    /// Effect drained from the delivery child.
    Delivery(ConversationDeliveryEffect),
    /// Effect drained from one exact steering child.
    Steering {
        /// Command identity of the child.
        command_id: RequestId,
        /// Generation of the child.
        generation: u64,
        /// Child effect.
        effect: crate::conversation_steering_machine::SteeringEffect,
    },
    /// A disclosure child reached its explicit retired leaf.
    Disclosure {
        /// Stable scene key.
        scene_id: SceneId,
        /// Child effect.
        effect: DisclosureEffect,
    },
    /// Effect drained from the sole viewport child.
    Viewport(ViewportEffect),
    /// A derived aggregate view or scene changed.
    SceneInvalidated,
}

/// Ergonomic alias for the aggregate effect vocabulary.
pub type ConversationAggregateEffect = ConversationStateEffect;

/// Ergonomic alias for the aggregate immutable view.
pub type ConversationAggregateView = ConversationStateView;

/// Ergonomic alias for the aggregate error.
pub type ConversationAggregateError = ConversationStateError;

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct SteeringKey {
    command_id: RequestId,
    generation: u64,
}

struct SteeringRecord {
    scene_id: SceneId,
    controller: ConversationSteeringMachine,
}

/// Sole synchronous conversation composition authority for one fixed thread.
pub struct ConversationStateController {
    delivery: ConversationDeliveryController,
    turns: BTreeMap<TurnId, ConversationTurnController>,
    /// Turns under explicit caller ownership.
    ///
    /// Delivery synchronizes only turns absent from this set: an explicit
    /// [`Self::register_turn`] replaces any delivery-derived controller with
    /// a fresh one and takes over that turn permanently, so manual drive
    /// semantics never change under a live subscription. Bounded by
    /// [`MAX_TURN_CONTROLLERS`] together with [`Self::turns`].
    explicit_turns: BTreeSet<TurnId>,
    steerings: BTreeMap<SteeringKey, SteeringRecord>,
    disclosures: BTreeMap<SceneId, DisclosureController>,
    facts: BTreeMap<SceneId, SceneFact>,
    viewport: ViewportController,
    effects: Vec<ConversationStateEffect>,
}

impl fmt::Debug for ConversationStateController {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ConversationStateController")
            .field("thread_id", self.delivery.thread_id())
            .field("delivery_phase", &self.delivery.phase())
            .field("turn_count", &self.turns.len())
            .field("explicit_turn_count", &self.explicit_turns.len())
            .field("steering_count", &self.steerings.len())
            .field("disclosure_count", &self.disclosures.len())
            .field("scene_fact_count", &self.facts.len())
            .field("viewport_state", &self.viewport.state())
            .field("pending_effect_count", &self.effects.len())
            .finish()
    }
}

impl ConversationStateController {
    /// Creates the fixed-thread aggregate and exposes its initial snapshot
    /// request as the first aggregate effect.
    #[must_use]
    pub fn new(thread_id: ThreadId) -> Self {
        let mut delivery = ConversationDeliveryController::new(thread_id);
        let initial_effects = delivery.drain_effects();
        let mut effects = Vec::with_capacity(MAX_PENDING_EFFECTS);
        effects.extend(
            initial_effects
                .into_iter()
                .map(ConversationStateEffect::Delivery),
        );
        Self {
            delivery,
            turns: BTreeMap::new(),
            explicit_turns: BTreeSet::new(),
            steerings: BTreeMap::new(),
            disclosures: BTreeMap::new(),
            facts: BTreeMap::new(),
            viewport: ViewportController::new(),
            effects,
        }
    }

    /// Returns the fixed thread identity.
    #[must_use]
    pub fn thread_id(&self) -> &ThreadId {
        self.delivery.thread_id()
    }

    /// Dispatches one closed aggregate event.
    ///
    /// All registry checks and relevant effect-capacity checks happen before
    /// child mutation. A child refusal is returned in a typed error and does
    /// not reorder or remove already pending aggregate effects.
    ///
    /// # Errors
    ///
    /// Returns [`ConversationStateError`] when the owner is closed, an event
    /// targets an unknown or conflicting identity, a bounded capacity is
    /// exhausted, or a child rejects the event.
    pub fn dispatch(
        &mut self,
        event: ConversationStateEvent,
    ) -> Result<(), ConversationStateError> {
        if self.delivery.is_closed() {
            return Err(ConversationStateError::OwnerClosed);
        }

        match event {
            ConversationStateEvent::Close
            | ConversationStateEvent::Delivery(ConversationDeliveryEvent::Closed) => {
                self.close_owner()
            }
            ConversationStateEvent::Delivery(event) => self.dispatch_delivery(&event),
            ConversationStateEvent::RegisterTurn { turn_id } => self.register_turn(turn_id),
            ConversationStateEvent::Turn { turn_id, event } => self.dispatch_turn(turn_id, event),
            ConversationStateEvent::RegisterSteering {
                command_id,
                generation,
                source_reference,
                started_at_ms,
                label_kind,
            } => self.register_steering(
                command_id,
                generation,
                &source_reference,
                started_at_ms,
                label_kind,
            ),
            ConversationStateEvent::Steering(event) => self.dispatch_steering(&event),
            ConversationStateEvent::RegisterDisclosure {
                scene_id,
                initially_working,
            } => self.register_disclosure(scene_id, initially_working),
            ConversationStateEvent::Disclosure { scene_id, event } => {
                self.dispatch_disclosure(scene_id, event)
            }
            ConversationStateEvent::Viewport(event) => self.dispatch_viewport(event),
            ConversationStateEvent::Fact(command) => self.dispatch_fact(command),
        }
    }

    /// Alias for [`Self::dispatch`].
    ///
    /// # Errors
    ///
    /// Returns the [`ConversationStateError`] produced by [`Self::dispatch`].
    pub fn handle_event(
        &mut self,
        event: ConversationStateEvent,
    ) -> Result<(), ConversationStateError> {
        self.dispatch(event)
    }

    /// Routes one delivery event.
    ///
    /// # Errors
    ///
    /// Returns [`ConversationStateError`] when the owner is closed, the
    /// delivery event fails scene validation, capacity is exhausted, or the
    /// delivery child rejects the event.
    pub fn on_delivery(
        &mut self,
        event: ConversationDeliveryEvent,
    ) -> Result<(), ConversationStateError> {
        self.dispatch(ConversationStateEvent::Delivery(event))
    }

    /// Registers one turn controller.
    ///
    /// Explicit registration takes permanent ownership of the turn: it
    /// replaces any delivery-derived controller with a fresh one, and later
    /// delivery synchronization skips explicitly owned turns. Registering an
    /// already explicit turn is still a typed duplicate.
    ///
    /// # Errors
    ///
    /// Returns [`ConversationStateError::OwnerClosed`] for a closed owner,
    /// [`ConversationStateError::DuplicateTurn`] for an already explicit
    /// identity, or [`ConversationStateError::CapacityExhausted`] when a bound
    /// is full.
    pub fn register_turn(&mut self, turn_id: TurnId) -> Result<(), ConversationStateError> {
        if self.delivery.is_closed() {
            return Err(ConversationStateError::OwnerClosed);
        }
        if self.explicit_turns.contains(&turn_id) {
            return Err(ConversationStateError::DuplicateTurn { turn_id });
        }
        let fresh = !self.turns.contains_key(&turn_id);
        Self::ensure_capacity(
            CapacityResource::Turns,
            self.turns.len().saturating_add(usize::from(fresh)),
            MAX_TURN_CONTROLLERS,
        )?;
        self.ensure_effect_capacity(1)?;
        self.turns
            .insert(turn_id.clone(), ConversationTurnController::new());
        self.explicit_turns.insert(turn_id);
        self.push_effect(ConversationStateEffect::SceneInvalidated);
        Ok(())
    }

    /// Routes one event to a registered turn controller.
    ///
    /// # Errors
    ///
    /// Returns [`ConversationStateError`] when the owner or turn rejects the
    /// event, including unknown turns, invalid transitions, or exhausted
    /// effect capacity.
    pub fn on_turn(
        &mut self,
        turn_id: TurnId,
        event: TurnEvent,
    ) -> Result<(), ConversationStateError> {
        self.dispatch(ConversationStateEvent::Turn { turn_id, event })
    }

    /// Registers one exact steering command generation.
    ///
    /// # Errors
    ///
    /// Returns [`ConversationStateError`] for a closed owner, duplicate or
    /// conflicting identity, exhausted capacity, invalid scene identity, or
    /// rejected steering construction.
    pub fn register_steering(
        &mut self,
        command_id: RequestId,
        generation: u64,
        source_reference: &crate::conversation_steering_machine::SourceReference,
        started_at_ms: i64,
        label_kind: SteeringLabelKind,
    ) -> Result<(), ConversationStateError> {
        if self.delivery.is_closed() {
            return Err(ConversationStateError::OwnerClosed);
        }
        let key = SteeringKey {
            command_id: command_id.clone(),
            generation,
        };
        if self.steerings.contains_key(&key) {
            return Err(ConversationStateError::DuplicateSteering {
                command_id,
                generation,
            });
        }
        Self::ensure_capacity(
            CapacityResource::Steerings,
            self.steerings.len().saturating_add(1),
            MAX_STEERING_CONTROLLERS,
        )?;
        self.ensure_effect_capacity(1)?;

        let scene_id = steering_scene_id(&key)
            .map_err(|error| ConversationStateError::InvalidSceneIdentity { error })?;
        if self
            .steerings
            .values()
            .any(|record| record.scene_id == scene_id)
        {
            return Err(ConversationStateError::SceneConflict { id: scene_id });
        }
        if self.facts.contains_key(&scene_id)
            || self.delivery.snapshot().is_some_and(|snapshot| {
                snapshot
                    .items()
                    .iter()
                    .any(|item| item.item_id().as_str() == scene_id.as_str())
            })
        {
            return Err(ConversationStateError::SceneConflict { id: scene_id });
        }

        let controller = ConversationSteeringMachine::new(
            command_id.as_str(),
            generation,
            source_reference.as_str(),
            started_at_ms,
            label_kind,
        )
        .map_err(|error| {
            let error = match error {
                SteeringControllerError::InvalidGeneration => {
                    SteeringConstructionError::InvalidGeneration
                }
                SteeringControllerError::InvalidCommandId(_)
                | SteeringControllerError::EmptySourceReference
                | SteeringControllerError::InvalidSourceReference(_) => {
                    SteeringConstructionError::InvalidInput
                }
            };
            ConversationStateError::SteeringConstruction(error)
        })?;
        self.steerings.insert(
            key,
            SteeringRecord {
                scene_id,
                controller,
            },
        );
        self.push_effect(ConversationStateEffect::SceneInvalidated);
        Ok(())
    }

    /// Routes one fenced event to its exact steering controller.
    ///
    /// # Errors
    ///
    /// Returns [`ConversationStateError`] when the owner or steering child
    /// rejects the event, its anchor is invalid, or effect capacity is full.
    pub fn on_steering(&mut self, event: SteeringEvent) -> Result<(), ConversationStateError> {
        self.dispatch(ConversationStateEvent::Steering(event))
    }

    /// Registers one disclosure controller keyed by stable scene identity.
    ///
    /// # Errors
    ///
    /// Returns [`ConversationStateError::OwnerClosed`] for a closed owner,
    /// [`ConversationStateError::DuplicateDisclosure`] for a duplicate key, or
    /// [`ConversationStateError::CapacityExhausted`] when a bound is full.
    pub fn register_disclosure(
        &mut self,
        scene_id: SceneId,
        initially_working: bool,
    ) -> Result<(), ConversationStateError> {
        if self.delivery.is_closed() {
            return Err(ConversationStateError::OwnerClosed);
        }
        if self.disclosures.contains_key(&scene_id) {
            return Err(ConversationStateError::DuplicateDisclosure { scene_id });
        }
        Self::ensure_capacity(
            CapacityResource::Disclosures,
            self.disclosures.len().saturating_add(1),
            MAX_DISCLOSURE_CONTROLLERS,
        )?;
        self.ensure_effect_capacity(1)?;
        self.disclosures
            .insert(scene_id, DisclosureController::new(initially_working));
        self.push_effect(ConversationStateEffect::SceneInvalidated);
        Ok(())
    }

    /// Routes one lifecycle or user disclosure event.
    ///
    /// # Errors
    ///
    /// Returns [`ConversationStateError`] when the owner or disclosure key is
    /// invalid, effect capacity is exhausted, or the child rejects the event.
    pub fn on_disclosure(
        &mut self,
        scene_id: SceneId,
        event: DisclosureEvent,
    ) -> Result<(), ConversationStateError> {
        self.dispatch(ConversationStateEvent::Disclosure { scene_id, event })
    }

    /// Routes one event to the sole viewport controller.
    ///
    /// # Errors
    ///
    /// Returns [`ConversationStateError`] when the owner or viewport is closed
    /// for the event, or when effect capacity is exhausted.
    pub fn on_viewport(&mut self, event: ViewportEvent) -> Result<(), ConversationStateError> {
        self.dispatch(ConversationStateEvent::Viewport(event))
    }

    /// Registers one bounded non-durable fact.
    ///
    /// # Errors
    ///
    /// Returns [`ConversationStateError`] for a closed owner, duplicate or
    /// conflicting fact identity, invalid scene data, or exhausted capacity.
    pub fn register_fact(&mut self, fact: SceneFact) -> Result<(), ConversationStateError> {
        self.dispatch(ConversationStateEvent::Fact(SceneFactCommand::Register(
            fact,
        )))
    }

    /// Removes one bounded non-durable fact.
    ///
    /// # Errors
    ///
    /// Returns [`ConversationStateError::OwnerClosed`] for a closed owner,
    /// [`ConversationStateError::UnknownFact`] for an absent fact, or
    /// [`ConversationStateError::CapacityExhausted`] when effects cannot wait.
    pub fn remove_fact(&mut self, id: SceneId) -> Result<(), ConversationStateError> {
        self.dispatch(ConversationStateEvent::Fact(SceneFactCommand::Remove {
            id,
        }))
    }

    /// Atomically inserts or updates one bounded non-durable fact.
    ///
    /// An identical fact is a no-op without effects; a changed fact for the
    /// same turn updates in place while keeping its first accepted ordinal.
    ///
    /// # Errors
    ///
    /// Returns [`ConversationStateError`] for a closed owner, cross-turn
    /// reassignment, conflicting fact identity, invalid scene data, or
    /// exhausted capacity.
    pub fn upsert_fact(&mut self, fact: SceneFact) -> Result<(), ConversationStateError> {
        self.dispatch(ConversationStateEvent::Fact(SceneFactCommand::Upsert(fact)))
    }

    /// Closes delivery and the sole viewport owner. The close operation is
    /// idempotent at the child boundary but a second aggregate close is a
    /// typed closed-owner refusal.
    ///
    /// # Errors
    ///
    /// Returns [`ConversationStateError::OwnerClosed`] when already closed,
    /// [`ConversationStateError::CapacityExhausted`] when close effects cannot
    /// wait, or a child delivery error.
    pub fn close(&mut self) -> Result<(), ConversationStateError> {
        self.dispatch(ConversationStateEvent::Close)
    }

    /// Drains aggregate effects in their original order.
    #[must_use]
    pub fn drain_effects(&mut self) -> Vec<ConversationStateEffect> {
        std::mem::take(&mut self.effects)
    }

    /// Borrows aggregate effects without exposing mutable state.
    #[must_use]
    pub fn pending_effects(&self) -> &[ConversationStateEffect] {
        &self.effects
    }

    /// Returns the number of pending aggregate effects.
    #[must_use]
    pub fn pending_effect_count(&self) -> usize {
        self.effects.len()
    }

    /// Returns the sole delivery view.
    #[must_use]
    pub fn delivery_view(&self) -> ConversationDeliveryView {
        self.delivery.view()
    }

    /// Returns the last-good canonical snapshot, when one has arrived.
    ///
    /// Activity projection reads this snapshot to resolve attributed turns
    /// without duplicating delivery internals.
    #[must_use]
    pub fn snapshot(&self) -> Option<&ConversationSnapshot> {
        self.delivery.snapshot()
    }

    /// Returns the sole viewport state.
    #[must_use]
    pub fn viewport_state(&self) -> ViewportState {
        self.viewport.state()
    }

    /// Returns the sole viewport generation.
    #[must_use]
    pub fn viewport_generation(&self) -> ViewportGeneration {
        self.viewport.generation()
    }

    /// Returns whether the aggregate owner is closed.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.delivery.is_closed()
    }

    /// Derives a renderer-facing immutable aggregate view.
    #[must_use]
    pub fn view(&self) -> ConversationStateView {
        let delivery = self.delivery.view();
        let mut turn_views = Vec::with_capacity(self.turns.len());
        for (turn_id, controller) in &self.turns {
            turn_views.push(ConversationTurnView {
                turn_id: turn_id.clone(),
                view: controller.view(),
            });
        }

        let mut steering_views = Vec::with_capacity(self.steerings.len());
        let mut pending_lip_steering_views = Vec::new();
        for record in self.steerings.values() {
            let view = record.controller.view();
            if matches!(&view.placement, ChildSteeringPlacement::ComposerPendingLip) {
                pending_lip_steering_views.push(view.clone());
            }
            steering_views.push(ConversationSteeringView {
                scene_id: record.scene_id.clone(),
                view,
            });
        }

        let mut disclosure_views = Vec::with_capacity(self.disclosures.len());
        for (scene_id, controller) in &self.disclosures {
            disclosure_views.push(ConversationDisclosureView {
                scene_id: scene_id.clone(),
                state: controller.state(),
                disclosure: controller.disclosure(),
            });
        }

        ConversationStateView {
            delivery_phase: delivery.phase,
            delivery_status: delivery.projection_status,
            delivery,
            turn_views,
            steering_views,
            pending_lip_steering_views,
            disclosure_views,
            viewport_state: self.viewport.state(),
            viewport_generation: self.viewport.generation(),
            pending_effect_count: self.effects.len(),
            scene_fact_count: self.facts.len(),
            closed: self.delivery.is_closed(),
        }
    }

    /// Purely projects the last-good durable snapshot, typed facts, child
    /// narrations, anchored steering labels, and child disclosure values.
    ///
    /// While delivery is recovering, its child retains the last-good snapshot;
    /// this method therefore projects that same durable scene rather than a
    /// partially applied batch. Pending-lip, hidden, and failed steerings are
    /// intentionally absent from the scene and remain available through
    /// [`Self::view`] and aggregate effects.
    ///
    /// Active turn controllers contribute their authoritative clock basis to
    /// the turn status, and eligible completed turns receive their settled
    /// footer facts; anything else keeps an unsettled footer with no
    /// fabricated content.
    ///
    /// # Errors
    ///
    /// Returns [`ConversationStateError::Scene`] when bounded durable or
    /// non-durable inputs cannot form a valid scene.
    pub fn scene(&self) -> Result<ConversationScene, ConversationStateError> {
        let mut turns = Vec::new();
        let mut items = Vec::new();
        let durable_turn_ids = if let Some(snapshot) = self.delivery.snapshot() {
            turns.reserve(snapshot.turns().len());
            for turn in snapshot.turns() {
                turns.push(SceneTurn::new(
                    turn.turn_id.clone(),
                    turn.ordinal.get(),
                    turn.lifecycle,
                ));
            }

            items.reserve(snapshot.items().len().saturating_add(self.facts.len()));
            for item in snapshot.items() {
                items.push(self.scene_item_from_durable(item)?);
            }
            snapshot
                .turns()
                .iter()
                .map(|turn| turn.turn_id.clone())
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };

        for fact in self.facts.values() {
            items.push(
                fact.as_scene_item(self.scene_disclosure(&fact.id))
                    .map_err(ConversationStateError::Scene)?,
            );
        }

        let mut narrations = Vec::new();
        for (turn_id, controller) in &self.turns {
            if durable_turn_ids.iter().any(|durable| durable == turn_id) {
                let view = controller.view();
                let mut entry = TurnNarrationEntry::new(
                    turn_id.clone(),
                    scene_narration(&view.narration),
                );
                // The basis is the turn's own first active-entry event time,
                // never a sampled clock, so it stays fixed across rerenders,
                // resumes, and snapshot refreshes.
                if view.state.is_active()
                    && let Some(started_at) = view.started_at
                {
                    entry = entry.with_active_started_at_ms(started_at);
                }
                narrations.push(entry);
            }
        }

        let mut steerings = Vec::new();
        for record in self.steerings.values() {
            let view = record.controller.view();
            let ChildSteeringPlacement::AnchoredAfter { anchor } = view.placement else {
                continue;
            };
            let label = steering_label(view.label_kind);
            steerings.push(
                SceneSteeringPlacement::new(record.scene_id.clone(), anchor, label)
                    .map_err(ConversationStateError::Scene)?,
            );
        }

        ConversationScene::build(turns, items, narrations, steerings)
            .map_err(ConversationStateError::Scene)
            .and_then(|mut scene| {
                self.annotate_turn_footer_settlements(&mut scene)?;
                Ok(scene)
            })
    }

    /// Alias for [`Self::scene`] for renderer adapters.
    ///
    /// # Errors
    ///
    /// Returns the [`ConversationStateError`] produced by [`Self::scene`].
    pub fn render_scene(&self) -> Result<ConversationScene, ConversationStateError> {
        self.scene()
    }

    fn close_owner(&mut self) -> Result<(), ConversationStateError> {
        self.ensure_effect_capacity(MAX_CLOSE_EFFECTS)?;

        let delivery_result = self.delivery.close();
        self.push_delivery_effects();
        delivery_result.map_err(ConversationStateError::Delivery)?;

        let viewport_effects = self.viewport.handle(ViewportEvent::OwnerClosed);
        for effect in viewport_effects {
            self.push_effect(ConversationStateEffect::Viewport(effect));
        }
        Ok(())
    }

    fn dispatch_delivery(
        &mut self,
        event: &ConversationDeliveryEvent,
    ) -> Result<(), ConversationStateError> {
        // Derived activity facts own ordinals above the durable watermark at
        // project time, but later canonical growth can reuse those ordinals.
        // Rebase colliding derived facts atomically with acceptance: plan
        // without mutating, reserve ALL effect room (rebase invalidation plus
        // the delivery child's worst case) before touching the registry, then
        // validate with rollback. Every refusal below — validation, capacity,
        // or a refused delivery dispatch — leaves ordinals and effects
        // untouched. Stable ids, turns, and timing never move, and manually
        // registered facts keep conflict-on-collision semantics.
        let rebase = self.plan_derived_rebase(event)?;
        let rebase_effects = usize::from(!rebase.is_empty());
        self.ensure_effect_capacity(rebase_effects.saturating_add(MAX_DELIVERY_EFFECTS_PER_EVENT))?;
        let saved: Vec<(SceneId, u64)> = rebase
            .iter()
            .filter_map(|(id, _)| self.facts.get(id).map(|fact| (id.clone(), fact.ordinal)))
            .collect();
        for (id, ordinal) in &rebase {
            if let Some(fact) = self.facts.get_mut(id) {
                fact.ordinal = *ordinal;
            }
        }
        if let Err(error) = match event {
            ConversationDeliveryEvent::SnapshotReceived(snapshot) => {
                self.validate_snapshot_for_scene(snapshot)
            }
            ConversationDeliveryEvent::BatchReceived(batch) => self.validate_batch_for_scene(batch),
            ConversationDeliveryEvent::SubscriptionResumed { .. }
            | ConversationDeliveryEvent::RetryRequested
            | ConversationDeliveryEvent::Closed => Ok(()),
        } {
            self.restore_fact_ordinals(&saved);
            return Err(error);
        }
        let rebase_effect_index = if rebase.is_empty() {
            None
        } else {
            let index = self.effects.len();
            self.push_effect(ConversationStateEffect::SceneInvalidated);
            Some(index)
        };
        // The delivery child reports projection refusals (stale cursor,
        // rejected snapshot, thread mismatch) as `Ok` plus a `ReportRefusal`
        // effect, so dispatch success alone does not prove acceptance. Only
        // an advanced canonical snapshot keeps the speculative ordinals.
        let before = if rebase.is_empty() {
            None
        } else {
            self.delivery.snapshot().cloned()
        };
        match self.delivery.dispatch(event) {
            Ok(()) => {
                self.push_delivery_effects();
                if !rebase.is_empty() && self.delivery.snapshot() != before.as_ref() {
                    self.restore_fact_ordinals(&saved);
                    if let Some(index) = rebase_effect_index {
                        self.effects.remove(index);
                    }
                } else {
                    self.synchronize_turn_controllers();
                }
                Ok(())
            }
            Err(error) => {
                self.restore_fact_ordinals(&saved);
                if let Some(index) = rebase_effect_index {
                    self.effects.remove(index);
                }
                self.push_delivery_effects();
                Err(ConversationStateError::Delivery(error))
            }
        }
    }

    /// Restores fact ordinals saved before a speculative rebase.
    ///
    /// Only ordinals move, so replaying the saved values returns the
    /// registry to its exact pre-delivery shape after any refusal.
    fn restore_fact_ordinals(&mut self, saved: &[(SceneId, u64)]) {
        for (id, ordinal) in saved {
            if let Some(fact) = self.facts.get_mut(id) {
                fact.ordinal = *ordinal;
            }
        }
    }

    /// Plans new ordinals for derived facts colliding with incoming delivery.
    ///
    /// Returns `(fact id, new ordinal)` pairs in deterministic scene-id
    /// order. Only derived facts whose ordinal canonical growth reuses are
    /// listed; ids, turns, kinds, and timing are never replanned. Targets sit
    /// above the current snapshot durable maximum, the incoming watermark,
    /// and every retained fact ordinal, so one pass cannot collide with
    /// durable state or with another fact.
    ///
    /// # Errors
    ///
    /// Returns [`ConversationStateError::CapacityExhausted`] when ordinal
    /// assignment overflows, so no two facts can share `u64::MAX`.
    fn plan_derived_rebase(
        &self,
        event: &ConversationDeliveryEvent,
    ) -> Result<Vec<(SceneId, u64)>, ConversationStateError> {
        let incoming_ordinals: BTreeSet<u64> = match event {
            ConversationDeliveryEvent::SnapshotReceived(snapshot) => {
                if snapshot.thread_id() != self.thread_id() {
                    return Ok(Vec::new());
                }
                snapshot
                    .turns()
                    .iter()
                    .map(|turn| turn.ordinal.get())
                    .chain(snapshot.items().iter().map(|item| item.ordinal().get()))
                    .collect()
            }
            ConversationDeliveryEvent::BatchReceived(batch) => {
                if batch.thread_id() != self.thread_id() {
                    return Ok(Vec::new());
                }
                batch
                    .patches()
                    .iter()
                    .filter_map(|patch| match patch {
                        ConversationPatch::TurnUpsert { turn, .. } => Some(turn.ordinal.get()),
                        ConversationPatch::ItemUpsert { item, .. } => Some(item.ordinal().get()),
                        ConversationPatch::ItemAppend { .. }
                        | ConversationPatch::ItemLifecycle { .. }
                        | ConversationPatch::TurnLifecycle { .. } => None,
                    })
                    .collect()
            }
            ConversationDeliveryEvent::SubscriptionResumed { .. }
            | ConversationDeliveryEvent::RetryRequested
            | ConversationDeliveryEvent::Closed => return Ok(Vec::new()),
        };
        if incoming_ordinals.is_empty() {
            return Ok(Vec::new());
        }
        let mut colliding: Vec<&SceneId> = self
            .facts
            .values()
            .filter(|fact| fact.derived && incoming_ordinals.contains(&fact.ordinal))
            .map(|fact| &fact.id)
            .collect();
        if colliding.is_empty() {
            return Ok(Vec::new());
        }
        colliding.sort();
        let current_durable_max: u64 = self
            .delivery
            .snapshot()
            .map(|snapshot| {
                snapshot
                    .turns()
                    .iter()
                    .map(|turn| turn.ordinal.get())
                    .chain(snapshot.items().iter().map(|item| item.ordinal().get()))
                    .max()
                    .unwrap_or(0)
            })
            .unwrap_or(0);
        let ceiling = incoming_ordinals
            .last()
            .copied()
            .unwrap_or(0)
            .max(current_durable_max)
            .max(
                self.facts
                    .values()
                    .map(|fact| fact.ordinal)
                    .max()
                    .unwrap_or(0),
            );
        let overflow = || ConversationStateError::CapacityExhausted {
            resource: CapacityResource::SceneFacts,
            count: MAX_SCENE_FACTS.saturating_add(1),
            maximum: MAX_SCENE_FACTS,
        };
        let mut next = ceiling.checked_add(1).ok_or_else(overflow)?;
        let mut plan = Vec::with_capacity(colliding.len());
        for id in colliding {
            plan.push(((*id).clone(), next));
            next = next.checked_add(1).ok_or_else(overflow)?;
        }
        Ok(plan)
    }

    /// Synchronizes delivery-owned turn controllers with the last-good snapshot.
    ///
    /// After every accepted delivery event, each durable turn without an
    /// explicitly registered controller gains one (registry room permitting)
    /// and receives the events derived from canonical lifecycle and item
    /// evidence, so the live snapshot/patch path produces meaningful status
    /// without test-seeded manual drive. Derived refusals are swallowed: a
    /// sealed, stale, or regressed derivation only means the controller
    /// already covers that durable state. At most one invalidation is pushed,
    /// only when a controller changed leaf state; without effect room the
    /// accepted delivery still stands and status catches up on a later event.
    fn synchronize_turn_controllers(&mut self) {
        if self.delivery.is_closed() {
            return;
        }
        let Some(snapshot) = self.delivery.snapshot() else {
            return;
        };
        synchronize_turns(
            &mut self.turns,
            &self.explicit_turns,
            &self.facts,
            snapshot,
            &mut self.effects,
        );
    }

    fn dispatch_turn(
        &mut self,
        turn_id: TurnId,
        event: TurnEvent,
    ) -> Result<(), ConversationStateError> {
        let state = self
            .turns
            .get(&turn_id)
            .ok_or_else(|| ConversationStateError::UnknownTurn {
                turn_id: turn_id.clone(),
            })?
            .state();
        if matches!(&event, TurnEvent::Resume { .. }) && state != StateKind::Interrupted {
            return Err(ConversationStateError::InvalidTurnEvent { turn_id, state });
        }
        self.ensure_effect_capacity(1)?;
        self.turns
            .get_mut(&turn_id)
            .expect("turn was checked above")
            .dispatch(event)
            .map_err(|error| ConversationStateError::Turn { turn_id, error })?;
        self.push_effect(ConversationStateEffect::SceneInvalidated);
        Ok(())
    }

    fn dispatch_steering(&mut self, event: &SteeringEvent) -> Result<(), ConversationStateError> {
        let key = steering_key(event);
        if !self.steerings.contains_key(&key) {
            return Err(ConversationStateError::UnknownSteering {
                command_id: key.command_id.clone(),
                generation: key.generation,
            });
        }

        if let SteeringEvent::DurableItemAnchored { item_id, .. } = event {
            self.validate_steering_anchor(item_id)?;
        }
        self.ensure_effect_capacity(MAX_STEERING_EFFECTS_PER_EVENT)?;
        let child_effects = {
            let record = self
                .steerings
                .get_mut(&key)
                .expect("steering was checked above");
            record.controller.handle_event(event).map_err(|error| {
                ConversationStateError::Steering {
                    command_id: key.command_id.clone(),
                    generation: key.generation,
                    error,
                }
            })?;
            record.controller.drain_effects()
        };
        for effect in child_effects {
            self.push_effect(ConversationStateEffect::Steering {
                command_id: key.command_id.clone(),
                generation: key.generation,
                effect,
            });
        }
        Ok(())
    }

    fn dispatch_disclosure(
        &mut self,
        scene_id: SceneId,
        event: DisclosureEvent,
    ) -> Result<(), ConversationStateError> {
        if !self.disclosures.contains_key(&scene_id) {
            return Err(ConversationStateError::UnknownDisclosure {
                scene_id: scene_id.clone(),
            });
        }
        self.ensure_effect_capacity(MAX_DISCLOSURE_EFFECTS_PER_EVENT)?;
        let (effect, changed) = {
            let controller = self
                .disclosures
                .get_mut(&scene_id)
                .expect("disclosure was checked above");
            let before = controller.state();
            let effect = controller.handle(event);
            let changed = before != controller.state();
            (effect, changed)
        };
        if effect != DisclosureEffect::None {
            self.push_effect(ConversationStateEffect::Disclosure { scene_id, effect });
        }
        if changed {
            self.push_effect(ConversationStateEffect::SceneInvalidated);
        }
        Ok(())
    }

    fn dispatch_viewport(&mut self, event: ViewportEvent) -> Result<(), ConversationStateError> {
        if self.viewport.state().is_closed() && !matches!(&event, ViewportEvent::OwnerClosed) {
            return Err(ConversationStateError::ViewportClosed);
        }
        self.ensure_effect_capacity(MAX_VIEWPORT_EFFECTS_PER_EVENT)?;
        for effect in self.viewport.handle(event) {
            self.push_effect(ConversationStateEffect::Viewport(effect));
        }
        Ok(())
    }

    fn dispatch_fact(&mut self, command: SceneFactCommand) -> Result<(), ConversationStateError> {
        match command {
            SceneFactCommand::Register(fact) => self.register_fact_inner(fact)?,
            SceneFactCommand::Upsert(fact) => {
                if !self.upsert_fact_inner(fact)? {
                    return Ok(());
                }
            }
            SceneFactCommand::Remove { id } => {
                if !self.facts.contains_key(&id) {
                    return Err(ConversationStateError::UnknownFact { id });
                }
                self.ensure_effect_capacity(1)?;
                self.facts.remove(&id);
                self.push_effect(ConversationStateEffect::SceneInvalidated);
            }
        }
        // Facts feed the turn chart (work/thought evidence), so an accepted
        // fact mutation re-derives delivery-owned turns immediately instead
        // of waiting for the next delivery event.
        if !self.delivery.is_closed()
            && let Some(snapshot) = self.delivery.snapshot()
        {
            synchronize_turns(
                &mut self.turns,
                &self.explicit_turns,
                &self.facts,
                snapshot,
                &mut self.effects,
            );
        }
        Ok(())
    }

    fn register_fact_inner(&mut self, fact: SceneFact) -> Result<(), ConversationStateError> {
        if self.facts.contains_key(&fact.id) {
            return Err(ConversationStateError::DuplicateFact { id: fact.id });
        }
        Self::ensure_capacity(
            CapacityResource::SceneFacts,
            self.facts.len().saturating_add(1),
            MAX_SCENE_FACTS,
        )?;

        let item = fact
            .as_scene_item(None)
            .map_err(ConversationStateError::Scene)?;
        let Some(snapshot) = self.delivery.snapshot() else {
            return Err(ConversationStateError::UnknownTurn {
                turn_id: fact.turn_id,
            });
        };
        if !snapshot
            .turns()
            .iter()
            .any(|turn| turn.turn_id == item.turn_id)
        {
            return Err(ConversationStateError::UnknownTurn {
                turn_id: item.turn_id,
            });
        }
        Self::validate_fact_against_snapshot(&item, snapshot)?;
        let prospective_item_count = snapshot
            .items()
            .len()
            .saturating_add(self.facts.len())
            .saturating_add(1);
        if prospective_item_count > crate::conversation_scene::SCENE_MAX_ITEMS {
            return Err(ConversationStateError::Scene(
                SceneBuildError::TooManyItems {
                    count: prospective_item_count,
                    maximum: crate::conversation_scene::SCENE_MAX_ITEMS,
                },
            ));
        }
        if self
            .facts
            .values()
            .any(|existing| existing.ordinal == fact.ordinal)
        {
            return Err(ConversationStateError::SceneConflict { id: fact.id });
        }
        if self
            .steerings
            .values()
            .any(|record| record.scene_id == fact.id)
        {
            return Err(ConversationStateError::SceneConflict { id: fact.id });
        }
        self.ensure_effect_capacity(1)?;
        self.facts.insert(fact.id.clone(), fact);
        self.push_effect(ConversationStateEffect::SceneInvalidated);
        Ok(())
    }

    /// Atomically inserts or updates one fact, reporting whether state changed.
    ///
    /// Returns `Ok(false)` without touching effects when the registered fact
    /// already carries the same turn, kind, and timing: repeated projections
    /// of unchanged rows are effect-quiet. Otherwise validates like
    /// [`Self::register_fact_inner`] (closed owner, scene bounds, known
    /// turn, durable collisions, item ceiling, effect room), keeps the first
    /// accepted ordinal so repeated projections never shift scene order, and
    /// updates kind and timing in place. Reassignment to a different turn is
    /// refused with [`ConversationStateError::FactTurnMismatch`].
    fn upsert_fact_inner(&mut self, fact: SceneFact) -> Result<bool, ConversationStateError> {
        if let Some(existing) = self.facts.get(&fact.id) {
            if existing.turn_id != fact.turn_id {
                return Err(ConversationStateError::FactTurnMismatch {
                    id: fact.id,
                    turn_id: existing.turn_id.clone(),
                });
            }
            if existing.kind == fact.kind && existing.observed_at_ms == fact.observed_at_ms {
                return Ok(false);
            }
            let kept = SceneFact {
                id: fact.id.clone(),
                turn_id: fact.turn_id.clone(),
                ordinal: existing.ordinal,
                kind: fact.kind.clone(),
                observed_at_ms: fact.observed_at_ms,
                derived: fact.derived,
            };
            kept.as_scene_item(None)
                .map_err(ConversationStateError::Scene)?;
            let Some(snapshot) = self.delivery.snapshot() else {
                return Err(ConversationStateError::UnknownTurn {
                    turn_id: kept.turn_id.clone(),
                });
            };
            if !snapshot
                .turns()
                .iter()
                .any(|turn| turn.turn_id == kept.turn_id)
            {
                return Err(ConversationStateError::UnknownTurn {
                    turn_id: kept.turn_id.clone(),
                });
            }
            if snapshot_uses_ordinal(snapshot, kept.ordinal) {
                return Err(ConversationStateError::SceneConflict {
                    id: kept.id.clone(),
                });
            }
            if snapshot
                .items()
                .iter()
                .any(|durable| durable.item_id().as_str() == kept.id.as_str())
            {
                return Err(ConversationStateError::SceneConflict {
                    id: kept.id.clone(),
                });
            }
            self.ensure_effect_capacity(1)?;
            self.facts.insert(kept.id.clone(), kept);
            self.push_effect(ConversationStateEffect::SceneInvalidated);
            return Ok(true);
        }
        self.register_fact_inner(fact)?;
        Ok(true)
    }

    fn validate_steering_anchor(&self, item_id: &ItemId) -> Result<(), ConversationStateError> {
        let Some(snapshot) = self.delivery.snapshot() else {
            return Err(ConversationStateError::UnknownSteeringAnchor {
                anchor: item_id.clone(),
            });
        };
        let Some(item) = snapshot
            .items()
            .iter()
            .find(|item| item.item_id() == item_id)
        else {
            return Err(ConversationStateError::UnknownSteeringAnchor {
                anchor: item_id.clone(),
            });
        };
        if !matches!(item, ConversationItem::UserMessage(_) | ConversationItem::MultimodalUserMessage(_)) {
            return Err(ConversationStateError::NonUserSteeringAnchor {
                anchor: item_id.clone(),
            });
        }
        Ok(())
    }

    fn validate_snapshot_for_scene(
        &self,
        snapshot: &ConversationSnapshot,
    ) -> Result<(), ConversationStateError> {
        if snapshot.thread_id() != self.thread_id() {
            return Ok(());
        }
        if snapshot.turns().len() > crate::conversation_scene::SCENE_MAX_TURNS {
            return Err(ConversationStateError::Scene(
                SceneBuildError::TooManyTurns {
                    count: snapshot.turns().len(),
                    maximum: crate::conversation_scene::SCENE_MAX_TURNS,
                },
            ));
        }
        let item_count = snapshot.items().len().saturating_add(self.facts.len());
        if item_count > crate::conversation_scene::SCENE_MAX_ITEMS {
            return Err(ConversationStateError::Scene(
                SceneBuildError::TooManyItems {
                    count: item_count,
                    maximum: crate::conversation_scene::SCENE_MAX_ITEMS,
                },
            ));
        }
        for fact in self.facts.values() {
            if !snapshot
                .turns()
                .iter()
                .any(|turn| turn.turn_id == fact.turn_id)
            {
                return Err(ConversationStateError::UnknownTurn {
                    turn_id: fact.turn_id.clone(),
                });
            }
            if snapshot
                .items()
                .iter()
                .any(|item| item.item_id().as_str() == fact.id.as_str())
            {
                return Err(ConversationStateError::SceneConflict {
                    id: fact.id.clone(),
                });
            }
            if snapshot_uses_ordinal(snapshot, fact.ordinal) {
                return Err(ConversationStateError::SceneConflict {
                    id: fact.id.clone(),
                });
            }
        }

        for record in self.steerings.values() {
            let view = record.controller.view();
            if let ChildSteeringPlacement::AnchoredAfter { anchor } = view.placement
                && !snapshot_has_user_item(snapshot, &anchor)
            {
                return Err(ConversationStateError::SteeringAnchorUnavailable { anchor });
            }
        }
        Ok(())
    }

    fn validate_batch_for_scene(
        &self,
        batch: &artisan_domain::PatchBatch,
    ) -> Result<(), ConversationStateError> {
        if batch.thread_id() != self.thread_id() {
            return Ok(());
        }
        let Some(snapshot) = self.delivery.snapshot() else {
            return Ok(());
        };
        let mut turn_ids: BTreeSet<TurnId> = snapshot
            .turns()
            .iter()
            .map(|turn| turn.turn_id.clone())
            .collect();
        let mut item_ids: BTreeSet<ItemId> = snapshot
            .items()
            .iter()
            .map(|item| item.item_id().clone())
            .collect();
        for patch in batch.patches() {
            match patch {
                ConversationPatch::TurnUpsert { turn, .. } => {
                    turn_ids.insert(turn.turn_id.clone());
                    if turn_ids.len() > crate::conversation_scene::SCENE_MAX_TURNS {
                        return Err(ConversationStateError::Scene(
                            SceneBuildError::TooManyTurns {
                                count: turn_ids.len(),
                                maximum: crate::conversation_scene::SCENE_MAX_TURNS,
                            },
                        ));
                    }
                    for fact in self.facts.values() {
                        if fact.ordinal == turn.ordinal.get() {
                            return Err(ConversationStateError::SceneConflict {
                                id: fact.id.clone(),
                            });
                        }
                    }
                }
                ConversationPatch::ItemUpsert { item, .. } => {
                    self.validate_item_identity_and_ordinal(item.item_id(), item.ordinal().get())?;
                    if item_ids.insert(item.item_id().clone()) {
                        let item_count = item_ids.len().saturating_add(self.facts.len());
                        if item_count > crate::conversation_scene::SCENE_MAX_ITEMS {
                            return Err(ConversationStateError::Scene(
                                SceneBuildError::TooManyItems {
                                    count: item_count,
                                    maximum: crate::conversation_scene::SCENE_MAX_ITEMS,
                                },
                            ));
                        }
                    }
                }
                ConversationPatch::ItemAppend { item_id, .. }
                | ConversationPatch::ItemLifecycle { item_id, .. } => {
                    for fact in self.facts.values() {
                        if fact.id.as_str() == item_id.as_str() {
                            return Err(ConversationStateError::SceneConflict {
                                id: fact.id.clone(),
                            });
                        }
                    }
                }
                ConversationPatch::TurnLifecycle { .. } => {}
            }
        }
        Ok(())
    }

    fn validate_item_identity_and_ordinal(
        &self,
        item_id: &ItemId,
        ordinal: u64,
    ) -> Result<(), ConversationStateError> {
        for fact in self.facts.values() {
            if fact.id.as_str() == item_id.as_str() || fact.ordinal == ordinal {
                return Err(ConversationStateError::SceneConflict {
                    id: fact.id.clone(),
                });
            }
        }
        Ok(())
    }

    fn validate_fact_against_snapshot(
        item: &SceneItem,
        snapshot: &ConversationSnapshot,
    ) -> Result<(), ConversationStateError> {
        if snapshot_uses_ordinal(snapshot, item.ordinal) {
            return Err(ConversationStateError::SceneConflict {
                id: item.id.clone(),
            });
        }
        if snapshot
            .items()
            .iter()
            .any(|durable| durable.item_id().as_str() == item.id.as_str())
        {
            return Err(ConversationStateError::SceneConflict {
                id: item.id.clone(),
            });
        }
        Ok(())
    }

    fn scene_item_from_durable(
        &self,
        item: &ConversationItem,
    ) -> Result<SceneItem, ConversationStateError> {
        let id = SceneId::from_item_id(item.item_id());
        let disclosure = self.scene_disclosure(&id);
        let (turn_id, ordinal, kind) = match item {
            ConversationItem::UserMessage(message) => (
                message.turn_id.clone(),
                message.ordinal.get(),
                SceneItemKind::UserMessage {
                    body: message.body.as_str().to_owned(),
                },
            ),
            ConversationItem::MultimodalUserMessage(message) => (
                message.turn_id.clone(),
                message.ordinal.get(),
                SceneItemKind::MultimodalUserMessage {
                    body: message.text.as_ref().map_or_else(String::new, |text| text.as_str().to_owned()),
                    attachments: message.attachments.clone(),
                },
            ),
            ConversationItem::AssistantMessage(message) => (
                message.turn_id.clone(),
                message.ordinal.get(),
                SceneItemKind::AssistantMessage {
                    body: message.body.as_str().to_owned(),
                    phase: match message.phase {
                        AssistantMessagePhase::Final => AssistantPhase::Final,
                        AssistantMessagePhase::Unspecified | AssistantMessagePhase::Commentary => {
                            AssistantPhase::Streaming
                        }
                    },
                },
            ),
        };
        SceneItem::new(id, turn_id, ordinal, kind, disclosure)
            .map_err(ConversationStateError::Scene)
    }

    /// Attaches settled footer facts to eligible completed turns.
    ///
    /// Eligibility mirrors the Electron reference: a footer exists only for a
    /// turn whose lifecycle is exactly [`ConversationLifecycle::Completed`]
    /// with the latest-by-ordinal non-empty settled reply — a completed
    /// [`AssistantMessagePhase::Final`] item — in that same turn. The
    /// settlement time is the turn's authoritative Forge `updated_at`, never a
    /// local clock. Anything else keeps its footer without a settlement, so
    /// the renderer shows no footer rather than a fabricated one.
    fn annotate_turn_footer_settlements(
        &self,
        scene: &mut ConversationScene,
    ) -> Result<(), ConversationStateError> {
        let Some(snapshot) = self.delivery.snapshot() else {
            return Ok(());
        };
        for turn in snapshot.turns() {
            if turn.lifecycle != ConversationLifecycle::Completed {
                continue;
            }
            let mut reply: Option<&artisan_domain::AssistantMessageItem> = None;
            for item in snapshot.items() {
                let ConversationItem::AssistantMessage(message) = item else {
                    continue;
                };
                if message.turn_id != turn.turn_id
                    || message.phase != AssistantMessagePhase::Final
                    || message.lifecycle != ConversationLifecycle::Completed
                    || message.body.as_str().is_empty()
                {
                    continue;
                }
                let newer = reply.is_none_or(|current: &_| {
                    message.ordinal.get() > current.ordinal.get()
                });
                if newer {
                    reply = Some(message);
                }
            }
            let Some(message) = reply else {
                continue;
            };
            let settlement = TurnFooterSettlement::new(
                message.body.as_str().to_owned(),
                turn.updated_at.as_millis(),
            )
            .map_err(ConversationStateError::Scene)?;
            scene.set_turn_footer_settlement(&turn.turn_id, settlement);
        }
        Ok(())
    }

    fn scene_disclosure(&self, id: &SceneId) -> Option<SceneDisclosure> {
        let controller = self.disclosures.get(id)?;
        if controller.is_retired() {
            return None;
        }
        Some(match controller.disclosure() {
            Disclosure::Open => SceneDisclosure::Open,
            Disclosure::Closed => SceneDisclosure::Closed,
        })
    }

    fn push_delivery_effects(&mut self) {
        for effect in self.delivery.drain_effects() {
            self.push_effect(ConversationStateEffect::Delivery(effect));
        }
    }

    fn push_effect(&mut self, effect: ConversationStateEffect) {
        debug_assert!(self.effects.len() < MAX_PENDING_EFFECTS);
        self.effects.push(effect);
    }

    fn ensure_effect_capacity(&self, additional: usize) -> Result<(), ConversationStateError> {
        let count = self.effects.len().saturating_add(additional);
        if count > MAX_PENDING_EFFECTS {
            return Err(ConversationStateError::CapacityExhausted {
                resource: CapacityResource::PendingEffects,
                count,
                maximum: MAX_PENDING_EFFECTS,
            });
        }
        Ok(())
    }

    fn ensure_capacity(
        resource: CapacityResource,
        count: usize,
        maximum: usize,
    ) -> Result<(), ConversationStateError> {
        if count > maximum {
            return Err(ConversationStateError::CapacityExhausted {
                resource,
                count,
                maximum,
            });
        }
        Ok(())
    }
}

/// Drives delivery-owned turn controllers from one accepted snapshot.
///
/// For every durable turn without an explicitly registered controller, ensures
/// a controller exists (skipped when the turn registry is full) and dispatches
/// the events derived from canonical lifecycle and item evidence. Derived
/// refusals leave state unchanged: they only mean the controller already
/// covers that durable state. Pushes at most one
/// [`ConversationStateEffect::SceneInvalidated`], only when a controller
/// actually changed leaf state, so replays and refreshes stay effect-quiet.
fn synchronize_turns(
    turns: &mut BTreeMap<TurnId, ConversationTurnController>,
    explicit_turns: &BTreeSet<TurnId>,
    facts: &BTreeMap<SceneId, SceneFact>,
    snapshot: &ConversationSnapshot,
    effects: &mut Vec<ConversationStateEffect>,
) {
    // Without effect room the accepted mutation still stands and status
    // catches up on a later event.
    if effects.len().saturating_add(1) > MAX_PENDING_EFFECTS {
        return;
    }
    let mut items_by_turn: BTreeMap<&TurnId, Vec<&ConversationItem>> = BTreeMap::new();
    for item in snapshot.items() {
        items_by_turn.entry(item.turn_id()).or_default().push(item);
    }
    let mut work_by_turn: BTreeSet<&TurnId> = BTreeSet::new();
    let mut thought_by_turn: BTreeSet<&TurnId> = BTreeSet::new();
    for fact in facts.values() {
        match &fact.kind {
            SceneFactKind::Activity { .. } | SceneFactKind::ChangedFiles { .. } => {
                work_by_turn.insert(&fact.turn_id);
            }
            SceneFactKind::Reasoning { .. } => {
                thought_by_turn.insert(&fact.turn_id);
            }
            SceneFactKind::Compaction { .. }
            | SceneFactKind::WorkSession { .. }
            | SceneFactKind::Plan { .. }
            | SceneFactKind::Approval { .. }
            | SceneFactKind::Question { .. }
            | SceneFactKind::Error { .. }
            | SceneFactKind::UsageInterruption { .. }
            | SceneFactKind::ModelTransition { .. }
            | SceneFactKind::NativeFact { .. } => {}
        }
    }

    let mut changed = false;
    for turn in snapshot.turns() {
        if explicit_turns.contains(&turn.turn_id) {
            continue;
        }
        if !turns.contains_key(&turn.turn_id) {
            if turns.len() >= MAX_TURN_CONTROLLERS {
                continue;
            }
            turns.insert(turn.turn_id.clone(), ConversationTurnController::new());
        }
        let controller = turns
            .get_mut(&turn.turn_id)
            .expect("turn controller was ensured above");
        let before = controller.state();
        let items: &[&ConversationItem] = items_by_turn
            .get(&turn.turn_id)
            .map_or(&[], Vec::as_slice);
        for event in derive_turn_events(
            turn,
            items,
            work_by_turn.contains(&turn.turn_id),
            thought_by_turn.contains(&turn.turn_id),
            controller,
        ) {
            // Best-effort: a sealed, stale, or regressed derivation only means
            // this durable state is already covered.
            let _ = controller.dispatch(event);
        }
        changed |= controller.state() != before;
    }
    if changed {
        effects.push(ConversationStateEffect::SceneInvalidated);
    }
}

/// Derives the turn-chart events for one durable turn from canonical evidence.
///
/// Only canonical lifecycle, item, and fact evidence feeds the chart — never
/// text content or speculation:
///
/// - terminal lifecycles settle (`Completed`, `Failed`, `Cancelled`,
///   `Interrupted`), replaying through a work/thought pre-step first so the
///   settled kind stays truthful;
/// - active lifecycles report a streaming reply while non-commentary reply
///   text streams, else work/thought evidence, else provider wait;
/// - `Pending` derives nothing: a queued turn renders quiet.
///
/// Timestamps are authoritative Forge times from the turn's own entities:
/// first activation counts from the turn's own creation (send time), so the
/// live elapsed basis matches the reference and never resets; later drives
/// reuse the turn's own update time, which the projection guarantees is
/// non-decreasing per turn, and terminal events additionally respect the
/// controller's last observed time — only ever as a floor against impossible
/// input, never as inflation. In particular the window watermark is never
/// used: an old historical turn settling under a newer window must keep its
/// own span, not the window's age. Revisions ride the controller's own
/// monotonic lane (`revision + 1/2`): durable per-entity revisions are
/// incomparable across a turn and never enter the chart.
fn derive_turn_events(
    turn: &ConversationTurn,
    items: &[&ConversationItem],
    work_evidence: bool,
    thought_evidence: bool,
    controller: &ConversationTurnController,
) -> Vec<TurnEvent> {
    // Floor against impossible input only: durable per-turn times never move
    // backward across accepted frames, so this clamp is normally identity.
    let last_time = controller.phase_started_at().unwrap_or(i64::MIN);
    let activate_at = if controller.started_at().is_none() {
        turn.created_at.as_millis()
    } else {
        turn.updated_at.as_millis().max(last_time)
    };
    let settle_at = turn.updated_at.as_millis().max(last_time);
    let first_revision = controller.revision().saturating_add(1);
    let second_revision = controller.revision().saturating_add(2);

    match turn.lifecycle {
        ConversationLifecycle::Pending => Vec::new(),
        ConversationLifecycle::Completed
        | ConversationLifecycle::Failed
        | ConversationLifecycle::Cancelled => {
            // A turn first seen already settled (history load) still earns a
            // pre-step: it plants the truthful creation basis and the settled
            // kind instead of collapsing the whole span to zero.
            let needs_basis = controller.started_at().is_none();
            let mut events = Vec::with_capacity(2);
            if work_evidence {
                events.push(TurnEvent::Working {
                    at: activate_at,
                    revision: first_revision,
                });
            } else if thought_evidence || needs_basis {
                events.push(TurnEvent::Thinking {
                    at: activate_at,
                    revision: first_revision,
                });
            }
            let revision = if events.is_empty() {
                first_revision
            } else {
                second_revision
            };
            events.push(match turn.lifecycle {
                ConversationLifecycle::Completed => TurnEvent::Completed {
                    at: settle_at,
                    revision,
                },
                ConversationLifecycle::Failed => TurnEvent::Failed {
                    at: settle_at,
                    revision,
                    kind: None,
                },
                _ => TurnEvent::Cancelled {
                    at: settle_at,
                    revision,
                },
            });
            events
        }
        ConversationLifecycle::Interrupted => vec![TurnEvent::Interrupted {
            at: settle_at,
            revision: first_revision,
        }],
        ConversationLifecycle::Active
        | ConversationLifecycle::Streaming
        | ConversationLifecycle::Waiting => {
            let streaming_reply = items.iter().any(|item| {
                matches!(item, ConversationItem::AssistantMessage(message)
                    if message.lifecycle == ConversationLifecycle::Streaming
                        && message.phase != AssistantMessagePhase::Commentary
                        && !message.body.as_str().is_empty())
            });
            vec![if streaming_reply {
                TurnEvent::StreamingReply {
                    at: activate_at,
                    revision: first_revision,
                }
            } else if work_evidence {
                TurnEvent::Working {
                    at: activate_at,
                    revision: first_revision,
                }
            } else if thought_evidence {
                TurnEvent::Thinking {
                    at: activate_at,
                    revision: first_revision,
                }
            } else {
                TurnEvent::WaitingForProvider {
                    at: activate_at,
                    revision: first_revision,
                }
            }]
        }
    }
}

fn steering_scene_id(key: &SteeringKey) -> Result<SceneId, SceneIdError> {
    SceneId::parse(format!(
        "steering-{}-{}",
        key.command_id.as_str(),
        key.generation
    ))
}

fn steering_key(event: &SteeringEvent) -> SteeringKey {
    match event {
        SteeringEvent::DispatchStarted {
            command_id,
            generation,
            ..
        }
        | SteeringEvent::DispatchAccepted {
            command_id,
            generation,
            ..
        }
        | SteeringEvent::DispatchFailed {
            command_id,
            generation,
            ..
        }
        | SteeringEvent::DurableItemAnchored {
            command_id,
            generation,
            ..
        }
        | SteeringEvent::EngineAcknowledged {
            command_id,
            generation,
            ..
        }
        | SteeringEvent::Cancelled {
            command_id,
            generation,
            ..
        } => SteeringKey {
            command_id: command_id.clone(),
            generation: *generation,
        },
    }
}

fn snapshot_uses_ordinal(snapshot: &ConversationSnapshot, ordinal: u64) -> bool {
    snapshot
        .turns()
        .iter()
        .any(|turn| turn.ordinal.get() == ordinal)
        || snapshot
            .items()
            .iter()
            .any(|item| item.ordinal().get() == ordinal)
}

fn snapshot_has_user_item(snapshot: &ConversationSnapshot, item_id: &ItemId) -> bool {
    snapshot
        .items()
        .iter()
        .any(|item| item.item_id() == item_id && matches!(item, ConversationItem::UserMessage(_) | ConversationItem::MultimodalUserMessage(_)))
}

fn scene_narration(narration: &TurnNarration) -> SceneTurnNarration {
    match narration {
        TurnNarration::Hidden => SceneTurnNarration::Quiet,
        TurnNarration::WaitingForProvider => SceneTurnNarration::ProviderWait,
        TurnNarration::Compacting => SceneTurnNarration::Compacting,
        TurnNarration::Thinking => SceneTurnNarration::Thinking,
        TurnNarration::Working => SceneTurnNarration::Working,
        TurnNarration::StreamingReply => SceneTurnNarration::StreamingSuppression,
        TurnNarration::WaitingForBackground => SceneTurnNarration::BackgroundWait,
        TurnNarration::WorkedFor { elapsed_ms } => SceneTurnNarration::WorkedFor {
            millis: *elapsed_ms,
        },
        TurnNarration::ThoughtFor { elapsed_ms } => SceneTurnNarration::ThoughtFor {
            millis: *elapsed_ms,
        },
        TurnNarration::Failed { .. } => SceneTurnNarration::Failed,
        TurnNarration::Interrupted { .. } => SceneTurnNarration::Interrupted,
        TurnNarration::Cancelled { .. } => SceneTurnNarration::Cancelled,
    }
}

fn steering_label(label_kind: SteeringLabelKind) -> String {
    match label_kind {
        SteeringLabelKind::Steering => "steering".to_owned(),
    }
}
