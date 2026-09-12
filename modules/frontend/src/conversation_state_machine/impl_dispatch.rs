//! Aggregate dispatch, bounded registry mutation, and effect plumbing for
//! [`ConversationStateController`].
//!
//! Extracted verbatim from `conversation_state_machine.rs` during the module
//! split.

use super::impl_scene::snapshot_uses_ordinal;
#[allow(clippy::wildcard_imports)]
use super::*;

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
            turn_engine_labels: BTreeMap::new(),
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
            ConversationStateEvent::Turn { turn_id, event } => self.dispatch_turn(turn_id, event),
            ConversationStateEvent::SetTurnEngineLabel {
                turn_id,
                engine_label,
            } => self.set_turn_engine_label(turn_id, engine_label),
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

    /// Routes one event to a delivery-derived turn controller.
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

    /// Sets or clears one turn's send-time engine display label.
    ///
    /// # Errors
    ///
    /// Returns [`ConversationStateError::OwnerClosed`] for a closed owner,
    /// [`ConversationStateError::UnknownTurn`] when no controller (explicit
    /// or delivery-derived) exists for the turn, [`ConversationStateError::Scene`]
    /// when a supplied label violates the scene label limits, or
    /// [`ConversationStateError::CapacityExhausted`] when the invalidation
    /// cannot wait. A repeated identical label (including clearing an
    /// already absent one) is a no-op success.
    pub fn on_turn_engine_label(
        &mut self,
        turn_id: TurnId,
        engine_label: Option<String>,
    ) -> Result<(), ConversationStateError> {
        self.dispatch(ConversationStateEvent::SetTurnEngineLabel {
            turn_id,
            engine_label,
        })
    }

    /// Applies a send-time engine display label mutation.
    ///
    /// Storage is keyed by turns already present in [`Self::turns`], so the
    /// map stays bounded by [`MAX_TURN_CONTROLLERS`] without its own ceiling.
    /// Labels for turns that leave the authoritative snapshot are pruned by
    /// [`Self::synchronize_turn_controllers`].
    ///
    /// # Errors
    ///
    /// Returns [`ConversationStateError::OwnerClosed`] for a closed owner,
    /// [`ConversationStateError::UnknownTurn`] for an absent turn,
    /// [`ConversationStateError::Scene`] for a label that violates the scene
    /// label limits, or [`ConversationStateError::CapacityExhausted`] when
    /// the invalidation cannot wait.
    pub fn set_turn_engine_label(
        &mut self,
        turn_id: TurnId,
        engine_label: Option<String>,
    ) -> Result<(), ConversationStateError> {
        if self.delivery.is_closed() {
            return Err(ConversationStateError::OwnerClosed);
        }
        if !self.turns.contains_key(&turn_id) {
            return Err(ConversationStateError::UnknownTurn {
                turn_id: turn_id.clone(),
            });
        }
        if let Some(label) = &engine_label {
            crate::conversation_scene::validate_engine_label(label)
                .map_err(ConversationStateError::Scene)?;
        }
        let changed = match (self.turn_engine_labels.get(&turn_id), &engine_label) {
            (None, None) => false,
            (Some(stored), Some(incoming)) if stored == incoming => false,
            _ => true,
        };
        if !changed {
            return Ok(());
        }
        self.ensure_effect_capacity(1)?;
        match engine_label {
            Some(label) => {
                self.turn_engine_labels.insert(turn_id, label);
            }
            None => {
                self.turn_engine_labels.remove(&turn_id);
            }
        }
        self.push_effect(ConversationStateEffect::SceneInvalidated);
        Ok(())
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
                if !rebase.is_empty() && self.delivery.snapshot() == before.as_ref() {
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
        let current_durable_max: u64 = self.delivery.snapshot().map_or(0, |snapshot| {
            snapshot
                .turns()
                .iter()
                .map(|turn| turn.ordinal.get())
                .chain(snapshot.items().iter().map(|item| item.ordinal().get()))
                .max()
                .unwrap_or(0)
        });
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
            &self.facts,
            snapshot,
            &mut self.effects,
        );
        // A turn that left the authoritative snapshot is retired from view:
        // its engine label goes with it so a later turn reusing nothing
        // stale can never inherit it. Removal changes future scenes, so it
        // invalidates exactly once when anything was actually dropped.
        let live: BTreeSet<TurnId> = snapshot
            .turns()
            .iter()
            .map(|turn| turn.turn_id.clone())
            .collect();
        let labels_before = self.turn_engine_labels.len();
        self.turn_engine_labels
            .retain(|turn_id, _| live.contains(turn_id));
        if self.turn_engine_labels.len() != labels_before
            && self.effects.len() < MAX_PENDING_EFFECTS
        {
            self.push_effect(ConversationStateEffect::SceneInvalidated);
        }
        self.synchronize_session_disclosures();
    }

    /// Ensures one disclosure controller per derived session anchor and
    /// routes lifecycle-following auto events.
    ///
    /// Session anchors come from the same run-counting rule as the scene
    /// build (exactly one content run: assistant runs plus run-attributed
    /// work facts). Registration seeds open-while-working so settled history
    /// starts closed even with details or failure; auto events follow the
    /// turn narration while explicit user choice stays authoritative inside
    /// the controller. Failure opening-once and reply-phase folding stay
    /// renderer-driven (they observe user intent); this only keeps the auto
    /// baseline truthful. Every turn is fail-soft: without registry or
    /// effect room the turn keeps its prior disclosure state.
    fn synchronize_session_disclosures(&mut self) {
        if self.delivery.is_closed() {
            return;
        }
        // Phase 1 borrows shared state only and collects the minimal owned
        // action per turn, so phase 2 can mutate without holding snapshot
        // borrows (and without cloning the snapshot).
        let mut actions: Vec<(SceneId, bool, Option<DisclosureEvent>)> = Vec::new();
        if let Some(snapshot) = self.delivery.snapshot() {
            for turn in snapshot.turns() {
                let mut runs: BTreeSet<String> = BTreeSet::new();
                for item in snapshot.items() {
                    if item.turn_id() != &turn.turn_id {
                        continue;
                    }
                    if let ConversationItem::AssistantMessage(message) = item {
                        runs.insert(message.run_id.as_str().to_owned());
                    }
                }
                for fact in self.facts.values() {
                    if fact.turn_id != turn.turn_id {
                        continue;
                    }
                    let work_kind = matches!(
                        &fact.kind,
                        SceneFactKind::Reasoning { .. }
                            | SceneFactKind::Activity { .. }
                            | SceneFactKind::Compaction { .. }
                            | SceneFactKind::NativeFact { .. }
                    );
                    if work_kind && let Some(run_id) = &fact.run_id {
                        runs.insert(run_id.as_str().to_owned());
                    }
                }
                if runs.len() != 1 {
                    continue;
                }
                let Ok(anchor) = session_anchor_id(&turn.turn_id) else {
                    continue;
                };
                let Some(controller) = self.turns.get(&turn.turn_id) else {
                    continue;
                };
                let working = controller.state().is_active();
                let event = if working {
                    Some(DisclosureEvent::WorkBecameActive)
                } else {
                    match &controller.view().narration {
                        TurnNarration::WorkedFor { .. } | TurnNarration::ThoughtFor { .. } => {
                            Some(DisclosureEvent::WorkSettledSuccessfully)
                        }
                        TurnNarration::Failed { .. }
                        | TurnNarration::Interrupted { .. }
                        | TurnNarration::Cancelled { .. } => {
                            Some(DisclosureEvent::WorkFailedOrInterrupted)
                        }
                        _ => None,
                    }
                };
                actions.push((anchor, working, event));
            }
        }
        // Phase 2 applies the collected actions; every turn stays fail-soft
        // without registry or effect room.
        for (anchor, working, event) in actions {
            let needs_register = !self.disclosures.contains_key(&anchor);
            let need_effects = if needs_register { 3 } else { 2 };
            if self.effects.len().saturating_add(need_effects) > MAX_PENDING_EFFECTS {
                continue;
            }
            if needs_register && self.register_disclosure(anchor.clone(), working).is_err() {
                continue;
            }
            if let Some(event) = event {
                let _ = self.on_disclosure(anchor, event);
            }
        }
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
        let Some(controller) = self.turns.get_mut(&turn_id) else {
            return Err(ConversationStateError::UnknownTurn { turn_id });
        };
        controller
            .dispatch(event)
            .map_err(|error| ConversationStateError::Turn { turn_id, error })?;
        self.push_effect(ConversationStateEffect::SceneInvalidated);
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
            let Some(controller) = self.disclosures.get_mut(&scene_id) else {
                return Err(ConversationStateError::UnknownDisclosure { scene_id });
            };
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
                &self.facts,
                snapshot,
                &mut self.effects,
            );
            self.synchronize_session_disclosures();
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
            if existing.kind == fact.kind
                && existing.observed_at_ms == fact.observed_at_ms
                && (fact.activity_lifecycle.is_none()
                    || existing.activity_lifecycle == fact.activity_lifecycle)
            {
                return Ok(false);
            }
            let kept = SceneFact {
                id: fact.id.clone(),
                turn_id: fact.turn_id.clone(),
                ordinal: existing.ordinal,
                kind: fact.kind.clone(),
                run_id: existing.run_id.clone(),
                activity_lifecycle: fact.activity_lifecycle.or(existing.activity_lifecycle),
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
/// For every durable turn, ensures a controller exists (skipped when the turn
/// registry is full) and dispatches the events derived from canonical
/// lifecycle and item evidence. Derived
/// refusals leave state unchanged: they only mean the controller already
/// covers that durable state. Late historical fact evidence for an
/// already-settled success rebuilds the delivery-owned controller from the
/// same canonical times and replays the fuller evidence, so the narration
/// becomes truthfully `Worked for` with identical elapsed; other sealed
/// outcomes and explicitly driven turns are never rebuilt. Pushes at most one
/// [`ConversationStateEffect::SceneInvalidated`], only when a controller's
/// rendered narration actually changed, so replays and refreshes stay
/// effect-quiet.
fn synchronize_turns(
    turns: &mut BTreeMap<TurnId, ConversationTurnController>,
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
        if !turns.contains_key(&turn.turn_id) {
            if turns.len() >= MAX_TURN_CONTROLLERS {
                continue;
            }
            turns.insert(turn.turn_id.clone(), ConversationTurnController::new());
        }
        let work = work_by_turn.contains(&turn.turn_id);
        let items: &[&ConversationItem] =
            items_by_turn.get(&turn.turn_id).map_or(&[], Vec::as_slice);
        // Late historical work evidence for an already-settled success:
        // sealed states refuse re-derivation, so a turn that settled before
        // its persisted tool history arrived would narrate `Thought for`
        // forever. Reconstruct the delivery-owned controller from the same
        // canonical turn, items, and facts and replay the fuller evidence:
        // the work pre-step plants the same creation basis and the same
        // settlement instant, so elapsed stays canonical while the narration
        // becomes truthfully `Worked for`. Other sealed outcomes narrate
        // independently of work evidence and keep their controllers.
        let settled_thought_success = turns.get(&turn.turn_id).is_some_and(|controller| {
            controller.state() == StateKind::Completed
                && work
                && matches!(
                    controller.view().narration,
                    TurnNarration::ThoughtFor { .. }
                )
        });
        if settled_thought_success {
            turns.insert(turn.turn_id.clone(), ConversationTurnController::new());
        }
        let Some(controller) = turns.get_mut(&turn.turn_id) else {
            continue;
        };
        let before = controller.state();
        let narration_before = controller.view().narration;
        for event in derive_turn_events(
            turn,
            items,
            work,
            thought_by_turn.contains(&turn.turn_id),
            controller,
        ) {
            // Best-effort: a sealed, stale, or regressed derivation only means
            // this durable state is already covered.
            let _ = controller.dispatch(event);
        }
        changed |= controller.state() != before || controller.view().narration != narration_before;
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
/// - `Pending` with a visible durable user message reports like an active turn; without one it stays quiet.
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
        ConversationLifecycle::Pending => {
            // A durable user message is the launch evidence: the send
            // happened, so the request is out even though the provider has
            // not responded yet. Without one, there is nothing visible to
            // anchor a status row to and the turn stays quiet.
            let launched = items.iter().any(|item| {
                matches!(
                    item,
                    ConversationItem::UserMessage(_) | ConversationItem::MultimodalUserMessage(_)
                )
            });
            if !launched {
                return Vec::new();
            }
            vec![drive_active_like(
                items,
                work_evidence,
                thought_evidence,
                activate_at,
                first_revision,
            )]
        }
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
            vec![drive_active_like(
                items,
                work_evidence,
                thought_evidence,
                activate_at,
                first_revision,
            )]
        }
    }
}

/// Single active-like drive shared by launched lifecycles.
///
/// Evidence of provider response outranks the wait, in reference order: a
/// streaming non-commentary reply speaks for itself, then work evidence,
/// then thought evidence; otherwise the request is still out and waits on
/// the provider. Callers gate on launch evidence; this helper only ranks it.
fn drive_active_like(
    items: &[&ConversationItem],
    work_evidence: bool,
    thought_evidence: bool,
    activate_at: i64,
    first_revision: u64,
) -> TurnEvent {
    let streaming_reply = items.iter().any(|item| {
        matches!(item, ConversationItem::AssistantMessage(message)
            if message.lifecycle == ConversationLifecycle::Streaming
                && message.phase != AssistantMessagePhase::Commentary
                && !message.body.as_str().is_empty())
    });
    if streaming_reply {
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
    }
}
