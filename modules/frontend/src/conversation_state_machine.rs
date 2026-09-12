//! Bounded synchronous composition of the conversation state machines.
//!
//! [`ConversationStateController`] is the one composition owner for a rendered
//! thread.  It owns the durable delivery controller, the delivery-derived turn
//! machines, disclosure machines, one viewport machine, and a small typed set
//! of non-durable scene facts.  It does not perform I/O or execute any effect:
//! callers drain [`ConversationStateEffect`] and decide how to execute those
//! effects at a boundary outside this module.
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
    ConversationSnapshot, ConversationTurn, ItemId, RunId, ThreadId, TurnId,
};
use thiserror::Error;

use crate::conversation_delivery_machine::{
    ConversationDeliveryController, ConversationDeliveryEffect, ConversationDeliveryError,
    ConversationDeliveryEvent, ConversationDeliveryView, DeliveryPhase,
};
use crate::conversation_scene::{
    AssistantPhase, ConversationScene, ItemProvenance, SceneBuildError, SceneDisclosure, SceneId,
    SceneItem, SceneItemKind, SceneTurn, TurnFooterSettlement,
    TurnNarration as SceneTurnNarration, TurnNarrationEntry, session_anchor_id,
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

/// Sole synchronous conversation composition authority for one fixed thread.
pub struct ConversationStateController {
    delivery: ConversationDeliveryController,
    turns: BTreeMap<TurnId, ConversationTurnController>,
    /// Send-time engine display labels keyed by turn.
    ///
    /// Display metadata only: labels never fabricate work, sessions, or
    /// lifecycle. Entries exist only for turns present in [`Self::turns`],
    /// so the map stays bounded by [`MAX_TURN_CONTROLLERS`]; labels for
    /// turns that leave the authoritative snapshot are pruned during
    /// synchronization.
    turn_engine_labels: BTreeMap<TurnId, String>,
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
            .field("engine_label_count", &self.turn_engine_labels.len())
            .field("disclosure_count", &self.disclosures.len())
            .field("scene_fact_count", &self.facts.len())
            .field("viewport_state", &self.viewport.state())
            .field("pending_effect_count", &self.effects.len())
            .finish()
    }
}
