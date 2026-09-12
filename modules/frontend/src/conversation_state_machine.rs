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
    ConversationSnapshot, ConversationTurn, ItemId, RequestId, RunId, ThreadId, TurnId,
};
use thiserror::Error;

use crate::conversation_delivery_machine::{
    ConversationDeliveryController, ConversationDeliveryEffect, ConversationDeliveryError,
    ConversationDeliveryEvent, ConversationDeliveryView, DeliveryPhase,
};
use crate::conversation_scene::{
    AssistantPhase, ConversationScene, ItemProvenance, SceneBuildError, SceneDisclosure, SceneId,
    SceneIdError, SceneItem, SceneItemKind, SceneTurn, SteeringPlacement as SceneSteeringPlacement,
    TurnFooterSettlement, TurnNarration as SceneTurnNarration, TurnNarrationEntry,
    session_anchor_id,
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

// Phase-1 split submodules (see conversation_state_machine/).

#[path = "conversation_state_machine/types.rs"]
mod types;

#[path = "conversation_state_machine/impl_dispatch.rs"]
mod impl_dispatch;

#[path = "conversation_state_machine/impl_scene.rs"]
mod impl_scene;

pub use types::*;

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
    /// Send-time engine display labels keyed by turn.
    ///
    /// Display metadata only: labels never fabricate work, sessions, or
    /// lifecycle. Entries exist only for turns present in [`Self::turns`],
    /// so the map stays bounded by [`MAX_TURN_CONTROLLERS`]; labels for
    /// turns that leave the authoritative snapshot are pruned during
    /// synchronization.
    turn_engine_labels: BTreeMap<TurnId, String>,
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
            .field("engine_label_count", &self.turn_engine_labels.len())
            .field("steering_count", &self.steerings.len())
            .field("disclosure_count", &self.disclosures.len())
            .field("scene_fact_count", &self.facts.len())
            .field("viewport_state", &self.viewport.state())
            .field("pending_effect_count", &self.effects.len())
            .finish()
    }
}
