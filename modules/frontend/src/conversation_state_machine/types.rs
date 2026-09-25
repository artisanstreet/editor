//! Aggregate event, view, effect, and error vocabulary for the conversation
//! state machine.
//!
//! Extracted verbatim from `conversation_state_machine.rs` during the module
//! split.

#[allow(clippy::wildcard_imports)]
use super::*;

/// Maximum registered turn controllers retained by one conversation owner.
pub const MAX_TURN_CONTROLLERS: usize = crate::conversation_scene::SCENE_MAX_TURNS;

/// Maximum disclosure controllers retained by one conversation owner.
pub const MAX_DISCLOSURE_CONTROLLERS: usize = crate::conversation_scene::SCENE_MAX_ITEMS;

/// Maximum typed non-durable facts retained by one conversation owner.
pub const MAX_SCENE_FACTS: usize = crate::conversation_scene::SCENE_MAX_ITEMS;

/// Maximum aggregate effects that may wait to be drained.
pub const MAX_PENDING_EFFECTS: usize = 4_096;

pub(super) const MAX_DELIVERY_EFFECTS_PER_EVENT: usize = 2;
pub(super) const MAX_VIEWPORT_EFFECTS_PER_EVENT: usize = 4;
pub(super) const MAX_DISCLOSURE_EFFECTS_PER_EVENT: usize = 2;
pub(super) const MAX_CLOSE_EFFECTS: usize = 2;

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
    /// Attributed run for run-scoped session derivation, when known.
    ///
    /// Filled by the observation projection from row attribution; manually
    /// registered facts leave it empty and never fabricate one.
    pub run_id: Option<RunId>,
    /// Typed liveness for activity facts, from the row's own lifecycle
    /// report.
    ///
    /// Only `Activity` facts ever carry this; every other kind leaves it
    /// empty. Unknown (empty) never means live: the scene treats only an
    /// explicitly live lifecycle as tool progress.
    pub activity_lifecycle: Option<ConversationLifecycle>,
    /// Narrow typed event timing in signed Unix millis, when known.
    ///
    /// This is the persisted engine commit time for activity projected from
    /// retained observations, never a sampled clock or a global watermark.
    /// It orders activity without fabricating time; elapsed evidence still
    /// derives from the canonical turn's own creation/update times.
    pub observed_at_ms: Option<i64>,
    /// First persisted event time, used only to interleave work with messages.
    pub first_observed_at_ms: Option<i64>,
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
            .field("run_id", &self.run_id)
            .field("activity_lifecycle", &self.activity_lifecycle)
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
            run_id: None,
            activity_lifecycle: None,
            observed_at_ms: None,
            first_observed_at_ms: None,
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

    /// Attaches the attributed run for run-scoped session derivation.
    ///
    /// The run comes from retained observation attribution, never parsed
    /// text. It does not change the fact identity, owning turn, ordinal, or
    /// kind.
    #[must_use]
    pub fn with_run_id(self, run_id: RunId) -> Self {
        Self {
            run_id: Some(run_id),
            ..self
        }
    }

    /// Attaches the typed activity liveness for tool-progress detection.
    ///
    /// The lifecycle comes from the row's own lifecycle report (tool action
    /// or terminal state mapped at the projection boundary), never inferred
    /// from body text. Only meaningful on `Activity` facts; the scene
    /// ignores it on every other kind. Absent never means live.
    #[must_use]
    pub fn with_activity_lifecycle(self, lifecycle: ConversationLifecycle) -> Self {
        Self {
            activity_lifecycle: Some(lifecycle),
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

    pub(super) fn as_scene_item(
        &self,
        disclosure: Option<SceneDisclosure>,
    ) -> Result<SceneItem, SceneBuildError> {
        let item = SceneItem::new(
            self.id.clone(),
            self.turn_id.clone(),
            self.ordinal,
            self.kind.as_scene_item_kind(),
            disclosure,
        )?;
        // Liveness rides provenance so the scene never infers it from text;
        // only Activity facts ever carry it, and only an explicit live
        // lifecycle counts downstream.
        let provenance = match (&self.run_id, &self.kind) {
            (Some(run_id), SceneFactKind::Activity { .. }) => Some(ItemProvenance {
                run_id: Some(run_id.clone()),
                lifecycle: self.activity_lifecycle,
            }),
            (Some(run_id), _) => Some(ItemProvenance {
                run_id: Some(run_id.clone()),
                lifecycle: None,
            }),
            (None, _) => None,
        };
        Ok(match provenance {
            Some(provenance) => item.with_provenance(provenance),
            None => item,
        })
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
    Activity {
        /// Bounded body preserved for legacy flat rows.
        body: String,
        /// Provider activity kind, when the source row carried one.
        kind: Option<String>,
        /// Raw provider detail, when disclosed.
        detail: Option<String>,
    },
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
            | Self::WorkSession { title: summary }
            | Self::Approval { prompt: summary }
            | Self::Question { prompt: summary }
            | Self::Error { message: summary }
            | Self::UsageInterruption { detail: summary }
            | Self::NativeFact { text: summary } => {
                structure.field("text_bytes", &summary.len());
            }
            Self::Activity { body, kind, detail } => {
                structure
                    .field("text_bytes", &body.len())
                    .field("kind_bytes", &kind.as_ref().map_or(0, String::len))
                    .field("detail_bytes", &detail.as_ref().map_or(0, String::len));
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
            Self::Activity { body, kind, detail } => SceneItemKind::Activity {
                body: body.clone(),
                kind: kind.clone(),
                detail: detail.clone(),
            },
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
    /// Route one event to a delivery-derived turn.
    Turn { turn_id: TurnId, event: TurnEvent },
    /// Set or clear one turn's send-time engine display label.
    ///
    /// Carries only validated display metadata captured at send time; it
    /// never fabricates work, sessions, or lifecycle. `None` clears a
    /// previously set label.
    SetTurnEngineLabel {
        /// Turn the label belongs to.
        turn_id: TurnId,
        /// Typed engine plus display label, or `None` to clear.
        engine_label: Option<TurnEngineLabel>,
    },
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
            Self::Turn { turn_id, event } => formatter
                .debug_struct("Turn")
                .field("turn_id", turn_id)
                .field("event", event)
                .finish(),
            Self::SetTurnEngineLabel {
                turn_id,
                engine_label,
            } => formatter
                .debug_struct("SetTurnEngineLabel")
                .field("turn_id", turn_id)
                .field("engine_label", engine_label)
                .finish(),
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
                Self::SetTurnEngineLabel {
                    turn_id: left_id,
                    engine_label: left_label,
                },
                Self::SetTurnEngineLabel {
                    turn_id: right_id,
                    engine_label: right_label,
                },
            ) => left_id == right_id && left_label == right_label,
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
    /// Disclosure-controller registry.
    Disclosures,
    /// Non-durable fact registry.
    SceneFacts,
    /// Aggregate pending-effect outbox.
    PendingEffects,
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
    /// A turn event targeted no known turn.
    #[error("turn {turn_id} is not registered")]
    UnknownTurn { turn_id: TurnId },
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
    /// A known turn received an event that its child chart deliberately does
    /// not accept.
    #[error("turn {turn_id} in {state:?} cannot accept this event")]
    InvalidTurnEvent { turn_id: TurnId, state: StateKind },
    /// The delivery child rejected a request-generation allocation.
    #[error("delivery child refused the event: {0}")]
    Delivery(#[source] ConversationDeliveryError),
    /// A turn child rejected the event.
    #[error("turn {turn_id} refused its event: {error}")]
    Turn {
        /// Turn identity.
        turn_id: TurnId,
        /// Child refusal.
        error: TurnError,
    },
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
