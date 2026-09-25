//! Durable scene projection, bounded validation, and footer settlement for
//! [`ConversationStateController`].
//!
//! Extracted verbatim from `conversation_state_machine.rs` during the module
//! split.

#[allow(clippy::wildcard_imports)]
use super::*;

impl ConversationStateController {
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
            disclosure_views,
            viewport_state: self.viewport.state(),
            viewport_generation: self.viewport.generation(),
            pending_effect_count: self.effects.len(),
            scene_fact_count: self.facts.len(),
            closed: self.delivery.is_closed(),
        }
    }

    /// Purely projects the last-good durable snapshot, typed facts, child
    /// narrations, and child disclosure values.
    ///
    /// While delivery is recovering, its child retains the last-good snapshot;
    /// this method therefore projects that same durable scene rather than a
    /// partially applied batch.
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
                let mut entry =
                    TurnNarrationEntry::new(turn_id.clone(), scene_narration(&view.narration));
                // The basis is the turn's own first active-entry event time,
                // never a sampled clock, so it stays fixed across rerenders,
                // resumes, and snapshot refreshes.
                if view.state.is_active()
                    && let Some(started_at) = view.started_at
                {
                    entry = entry.with_active_started_at_ms(started_at);
                }
                // Session disclosure travels on the turn-scoped entry: the
                // anchor is a pure function of the turn, so no build input
                // changes shape for it.
                if let Ok(anchor) = session_anchor_id(turn_id)
                    && let Some(disclosure) = self.scene_disclosure(&anchor)
                {
                    entry = entry.with_session_disclosure(disclosure);
                }
                // Explicit send-time engine labels travel the same entry so
                // the row can name the engine without session state.
                if let Some(engine) = self.turn_engine_labels.get(turn_id) {
                    entry = entry.with_turn_engine(engine);
                }
                narrations.push(entry);
            }
        }

        // Durable messages and retained work observations have separate ordinal
        // spaces. Merge them for rendering without changing either stored order
        // or identity. Use first-event time so streamed updates do not move rows.
        if self
            .facts
            .values()
            .any(|fact| fact.derived && fact.first_observed_at_ms.is_some())
            && let Some(snapshot) = self.delivery.snapshot()
        {
            let mut message_positions: std::collections::HashMap<TurnId, Vec<(i64, u64)>> =
                std::collections::HashMap::new();
            for message in snapshot.items() {
                let time = match message {
                    ConversationItem::UserMessage(message) => message.created_at,
                    ConversationItem::MultimodalUserMessage(message) => message.created_at,
                    ConversationItem::AssistantMessage(message) => message.created_at,
                };
                message_positions
                    .entry(message.turn_id().clone())
                    .or_default()
                    .push((time.as_millis(), message.ordinal().get()));
            }
            for positions in message_positions.values_mut() {
                positions.sort_unstable();
                let mut ordinal = 0;
                for (_, position) in positions {
                    ordinal = ordinal.max(*position);
                    *position = ordinal;
                }
            }
            let mut positions = Vec::with_capacity(items.len() + turns.len());
            for turn in &turns {
                positions.push(((turn.ordinal, 0, 0, 0), None));
            }
            for (index, item) in items.iter().enumerate() {
                let mut key = (item.ordinal, 0, 0, 0);
                if let Some(fact) = self.facts.get(&item.id)
                    && fact.derived
                    && let Some(time) = fact.first_observed_at_ms
                {
                    let anchor = message_positions
                        .get(&item.turn_id)
                        .and_then(|positions| {
                            let index = positions.partition_point(|(created, _)| *created <= time);
                            index.checked_sub(1).map(|index| positions[index].1)
                        })
                        .or_else(|| {
                            turns
                                .iter()
                                .find(|turn| turn.turn_id == item.turn_id)
                                .map(|turn| turn.ordinal)
                        })
                        .unwrap_or(item.ordinal);
                    key = (anchor, 1, time, item.ordinal);
                }
                positions.push((key, Some(index)));
            }
            positions.sort_by_key(|(key, _)| *key);
            let mut turn_ordinals = std::collections::HashMap::new();
            for (ordinal, (key, index)) in positions.into_iter().enumerate() {
                if let Some(index) = index {
                    items[index].ordinal = ordinal as u64;
                } else {
                    turn_ordinals.insert(key.0, ordinal as u64);
                }
            }
            for turn in &mut turns {
                turn.ordinal = turn_ordinals[&turn.ordinal];
            }
        }

        ConversationScene::build(turns, items, narrations, Vec::new())
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

    pub(super) fn validate_snapshot_for_scene(
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
        Ok(())
    }

    pub(super) fn validate_batch_for_scene(
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

    pub(super) fn validate_fact_against_snapshot(
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
        let (turn_id, ordinal, kind, provenance): (
            TurnId,
            u64,
            SceneItemKind,
            Option<ItemProvenance>,
        ) = match item {
            ConversationItem::UserMessage(message) => (
                message.turn_id.clone(),
                message.ordinal.get(),
                SceneItemKind::UserMessage {
                    body: message.body.as_str().to_owned(),
                },
                None,
            ),
            ConversationItem::MultimodalUserMessage(message) => (
                message.turn_id.clone(),
                message.ordinal.get(),
                SceneItemKind::MultimodalUserMessage {
                    body: message
                        .text
                        .as_ref()
                        .map_or_else(String::new, |text| text.as_str().to_owned()),
                    attachments: message.attachments.clone(),
                },
                None,
            ),
            ConversationItem::AssistantMessage(message) => {
                let kind = SceneItemKind::AssistantMessage {
                    body: message.body.as_str().to_owned(),
                    phase: match message.phase {
                        AssistantMessagePhase::Final => AssistantPhase::Final,
                        AssistantMessagePhase::Unspecified => AssistantPhase::Unspecified,
                        AssistantMessagePhase::Commentary => AssistantPhase::Commentary,
                    },
                };
                let provenance = ItemProvenance {
                    run_id: Some(message.run_id.clone()),
                    lifecycle: Some(message.lifecycle),
                };
                (
                    message.turn_id.clone(),
                    message.ordinal.get(),
                    kind,
                    Some(provenance),
                )
            }
        };
        let item = SceneItem::new(id, turn_id, ordinal, kind, disclosure)
            .map_err(ConversationStateError::Scene)?;
        Ok(match provenance {
            Some(provenance) => item.with_provenance(provenance),
            None => item,
        })
    }

    /// Attaches settled footer facts to eligible completed turns.
    ///
    /// Eligibility mirrors the Electron reference: a footer exists only for a
    /// turn whose lifecycle is exactly [`ConversationLifecycle::Completed`]
    /// with the build-promoted reply settled. The reply comes from the
    /// scene's single promotion source, never a second scan; the settlement
    /// time is the turn's authoritative Forge `updated_at`, never a local
    /// clock. Anything else keeps its footer without a settlement, so the
    /// renderer shows no footer rather than a fabricated one.
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
            let Some(reply_id) = scene.promoted_reply_id(&turn.turn_id) else {
                continue;
            };
            let Some(item) = snapshot
                .items()
                .iter()
                .find(|item| item.item_id().as_str() == reply_id.as_str())
            else {
                continue;
            };
            let ConversationItem::AssistantMessage(message) = item else {
                continue;
            };
            if message.lifecycle != ConversationLifecycle::Completed
                || message.body.as_str().is_empty()
            {
                continue;
            }
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
}

pub(super) fn snapshot_uses_ordinal(snapshot: &ConversationSnapshot, ordinal: u64) -> bool {
    snapshot
        .turns()
        .iter()
        .any(|turn| turn.ordinal.get() == ordinal)
        || snapshot
            .items()
            .iter()
            .any(|item| item.ordinal().get() == ordinal)
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
