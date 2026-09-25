//! Conversation scene build pipeline: identity anchoring, turn queries,
//! settlement promotion, and the scene `build` step.
//!
//! The large `build` entry point is retained verbatim here; decomposing it
//! into named sub-steps is deliberately deferred to a later pass.
//!
//! Extracted verbatim from `conversation_scene.rs` during the module split.

#![allow(clippy::module_name_repetitions)]

#[allow(clippy::wildcard_imports)]
use super::*;

/// Derives the stable session anchor for one turn.
///
/// The anchor is render-only grouping identity, never a persisted row:
/// `session-{turn_id}`. Turn identities admit no whitespace or control
/// characters, so only the byte ceiling can fail.
///
/// # Errors
///
/// Returns [`SceneBuildError::SessionAnchorTooLong`] when the anchor exceeds
/// [`SCENE_ID_MAX_BYTES`] UTF-8 bytes.
pub fn session_anchor_id(turn_id: &TurnId) -> Result<SceneId, SceneBuildError> {
    let text = format!("session-{}", turn_id.as_str());
    if text.len() > SCENE_ID_MAX_BYTES {
        return Err(SceneBuildError::SessionAnchorTooLong {
            length: text.len(),
            maximum: SCENE_ID_MAX_BYTES,
        });
    }
    SceneId::parse(text).map_err(|_| SceneBuildError::SessionAnchorTooLong {
        length: turn_id.as_str().len().saturating_add("session-".len()),
        maximum: SCENE_ID_MAX_BYTES,
    })
}

#[derive(Clone, Copy)]
enum AnchorKind {
    UserMessage,
    Other,
}

impl ConversationScene {
    /// Returns ordered turn scenes.
    #[must_use]
    pub fn turn_scenes(&self) -> &[TurnScene] {
        &self.turn_scenes
    }

    /// Returns typed deferred change-set cards.
    #[must_use]
    pub fn deferred_change_sets(&self) -> &[DeferredChangeSet] {
        &self.deferred
    }

    /// Returns the turn scene for `turn_id`, if present.
    #[must_use]
    pub fn turn_scene(&self, turn_id: &TurnId) -> Option<&TurnScene> {
        self.turn_scenes
            .iter()
            .find(|scene| &scene.turn_id == turn_id)
    }

    /// Returns the single promoted reply for `turn_id`, if promotion
    /// selected one.
    ///
    /// Promotion mirrors the reference final-message rules (explicit final,
    /// newest-phase reply, settled-last); dissolved and legacy turns may
    /// still promote a footer/copy source while rendering all prose
    /// top-level.
    #[must_use]
    pub fn promoted_reply_id(&self, turn_id: &TurnId) -> Option<SceneId> {
        self.promoted.get(turn_id).cloned()
    }

    /// Attaches settled footer facts to one turn's footer.
    ///
    /// The build emits every footer without a settlement; the aggregate owner
    /// supplies settlement only for eligible completed turns (see the
    /// projection contract). Returns whether the turn and its footer were
    /// present. Exactly one footer exists per turn, so at most one block is
    /// updated.
    pub fn set_turn_footer_settlement(
        &mut self,
        turn_id: &TurnId,
        settlement: TurnFooterSettlement,
    ) -> bool {
        let mut pending = Some(settlement);
        let mut applied = false;
        for scene in &mut self.turn_scenes {
            if &scene.turn_id != turn_id {
                continue;
            }
            for block in &mut scene.blocks {
                if let TurnBlock::TurnFooter(footer) = block
                    && let Some(next) = pending.take()
                {
                    footer.settlement = Some(next);
                    applied = true;
                }
            }
        }
        applied
    }

    /// Builds a deterministic scene from authoritative inputs.
    ///
    /// Frozen rules:
    ///
    /// - turns and items are ordered by their globally unique stable ordinal;
    /// - duplicate identity/ordinal and unknown ownership are typed errors;
    /// - contiguous reasoning/activity/work-session items coalesce into one
    ///   work group, while every other item is a barrier;
    /// - a compaction card cannot be paired with a generic thinking/working
    ///   narration;
    /// - a streaming assistant suppresses the status only when the supplied
    ///   narration is [`TurnNarration::StreamingSuppression`];
    /// - change cards render only for a terminal domain lifecycle and are
    ///   retained in [`Self::deferred_change_sets`] before that point;
    /// - an active-work clock basis on a narration entry is copied onto that
    ///   turn's status block, and is refused on any non-active-work narration;
    /// - every footer is emitted without a settlement; the aggregate owner
    ///   attaches one later with [`Self::set_turn_footer_settlement`] only for
    ///   eligible completed turns;
    /// - ordinary blocks are followed by a terminal change card, one status,
    ///   and one footer;
    /// - each steering placement appears once immediately after its exact
    ///   user-message anchor.
    ///
    /// The build is atomic: any validation failure returns only a typed error
    /// and no partial scene.
    ///
    /// # Errors
    ///
    /// Returns [`SceneBuildError`] when collection bounds, payload bounds,
    /// identities, ordinals, ownership, or steering placement rules fail.
    ///
    /// # Panics
    ///
    /// Panics if a session detail row appears before its group header; the
    /// collector is required to emit headers before their details.
    #[expect(
        clippy::too_many_lines,
        reason = "the scene build is one atomic validation pass; splitting it would break the all-or-nothing construction guarantee"
    )]
    pub fn build(
        turns: Vec<SceneTurn>,
        items: Vec<SceneItem>,
        narrations: Vec<TurnNarrationEntry>,
        steerings: Vec<SteeringPlacement>,
    ) -> Result<Self, SceneBuildError> {
        // Count bounds are checked before ownership is rearranged or any
        // large text payload is cloned into output blocks.
        if turns.len() > SCENE_MAX_TURNS {
            return Err(SceneBuildError::TooManyTurns {
                count: turns.len(),
                maximum: SCENE_MAX_TURNS,
            });
        }
        if items.len() > SCENE_MAX_ITEMS {
            return Err(SceneBuildError::TooManyItems {
                count: items.len(),
                maximum: SCENE_MAX_ITEMS,
            });
        }
        if narrations.len() > SCENE_MAX_NARRATIONS {
            return Err(SceneBuildError::TooManyNarrations {
                count: narrations.len(),
                maximum: SCENE_MAX_NARRATIONS,
            });
        }
        if steerings.len() > SCENE_MAX_STEERING_PLACEMENTS {
            return Err(SceneBuildError::TooManySteeringPlacements {
                count: steerings.len(),
                maximum: SCENE_MAX_STEERING_PLACEMENTS,
            });
        }

        // Validate every owned payload while it is still borrowed from the
        // caller's vectors. This also covers public struct literals that did
        // not use `SceneItem::new` or `SceneFileChange::new`.
        for item in &items {
            validate_item_kind(&item.kind)?;
        }
        for steering in &steerings {
            validate_steering_label(&steering.label)?;
        }

        // Domain snapshot invariants use globally unique turn/item ordinals;
        // the scene repeats that contract before sorting.
        let mut turn_ids: HashSet<TurnId> = HashSet::with_capacity(turns.len());
        let mut all_ordinals: HashSet<u64> =
            HashSet::with_capacity(turns.len().saturating_add(items.len()));
        for turn in &turns {
            if !turn_ids.insert(turn.turn_id.clone()) {
                return Err(SceneBuildError::DuplicateTurnId {
                    turn_id: turn.turn_id.clone(),
                });
            }
            if !all_ordinals.insert(turn.ordinal) {
                return Err(SceneBuildError::DuplicateOrdinal {
                    ordinal: turn.ordinal,
                });
            }
        }

        let mut narration_map: HashMap<TurnId, TurnNarrationEntry> =
            HashMap::with_capacity(narrations.len());
        for entry in narrations {
            if !turn_ids.contains(&entry.turn_id) {
                return Err(SceneBuildError::UnknownNarrationTurn {
                    turn_id: entry.turn_id.clone(),
                });
            }
            if entry.active_started_at_ms.is_some() && !entry.narration.is_active_work() {
                return Err(SceneBuildError::ActiveBasisWithoutActiveNarration {
                    narration: entry.narration,
                });
            }
            // Explicit engine labels are validated here, not only at the
            // aggregate boundary: entry fields and builders are public, so a
            // direct build caller can bypass dispatch validation.
            if let Some(label) = &entry.engine_label {
                validate_engine_label(label)?;
            }
            let narration_turn_id = entry.turn_id.clone();
            if narration_map
                .insert(narration_turn_id.clone(), entry)
                .is_some()
            {
                return Err(SceneBuildError::DuplicateNarration {
                    turn_id: narration_turn_id,
                });
            }
        }

        let mut item_ids: HashSet<SceneId> = HashSet::with_capacity(items.len());
        for item in &items {
            if !turn_ids.contains(&item.turn_id) {
                return Err(SceneBuildError::UnknownTurn {
                    item_id: item.id.clone(),
                    turn_id: item.turn_id.clone(),
                });
            }
            if !item_ids.insert(item.id.clone()) {
                return Err(SceneBuildError::DuplicateItemId {
                    id: item.id.clone(),
                });
            }
            if !all_ordinals.insert(item.ordinal) {
                return Err(SceneBuildError::DuplicateOrdinal {
                    ordinal: item.ordinal,
                });
            }
        }

        // Keep the exact ItemId in the placement, but use the validated text
        // only as the lookup bridge to a scene identity. The bridge cannot
        // relocate a label: it is checked against the one exact user item.
        let mut item_anchor_kinds: HashMap<&str, AnchorKind> = HashMap::with_capacity(items.len());
        for item in &items {
            item_anchor_kinds.insert(
                item.id.as_str(),
                if matches!(
                    &item.kind,
                    SceneItemKind::UserMessage { .. } | SceneItemKind::MultimodalUserMessage { .. }
                ) {
                    AnchorKind::UserMessage
                } else {
                    AnchorKind::Other
                },
            );
        }

        let mut steering_ids: HashSet<SceneId> = HashSet::with_capacity(steerings.len());
        for steering in &steerings {
            if !steering_ids.insert(steering.id.clone()) {
                return Err(SceneBuildError::DuplicateSteeringId {
                    id: steering.id.clone(),
                });
            }
            match item_anchor_kinds.get(steering.anchor.as_str()) {
                None => {
                    return Err(SceneBuildError::UnknownSteeringAnchor {
                        anchor: steering.anchor.clone(),
                    });
                }
                Some(AnchorKind::Other) => {
                    return Err(SceneBuildError::NonUserSteeringAnchor {
                        anchor: steering.anchor.clone(),
                    });
                }
                Some(AnchorKind::UserMessage) => {}
            }
        }
        drop(item_anchor_kinds);

        let mut sorted_turns = turns;
        sorted_turns.sort_by_key(|turn| turn.ordinal);

        let mut sorted_items = items;
        sorted_items.sort_by_key(|item| item.ordinal);

        let mut items_by_turn: HashMap<TurnId, Vec<SceneItem>> =
            HashMap::with_capacity(sorted_turns.len());
        for item in sorted_items {
            items_by_turn
                .entry(item.turn_id.clone())
                .or_default()
                .push(item);
        }

        // Preserve caller order when multiple legal placements share one
        // exact anchor. Different anchors remain independently addressable.
        let mut steerings_by_anchor: HashMap<String, Vec<SteeringPlacement>> =
            HashMap::with_capacity(steerings.len());
        for steering in steerings {
            steerings_by_anchor
                .entry(steering.anchor.as_str().to_owned())
                .or_default()
                .push(steering);
        }

        let mut turn_scenes = Vec::with_capacity(sorted_turns.len());
        let mut deferred = Vec::new();
        let mut promoted: HashMap<TurnId, SceneId> = HashMap::new();

        for turn in &sorted_turns {
            let entry = narration_map.get(&turn.turn_id);
            let narration = entry.map_or(TurnNarration::Quiet, |entry| entry.narration);
            let active_started_at_ms = entry.and_then(|entry| entry.active_started_at_ms);
            let session_disclosure = entry.and_then(|entry| entry.session_disclosure);
            // Explicit send-time engine labels outrank transition-derived
            // ones everywhere, including outside session mode.
            let explicit_engine_label = entry.and_then(|entry| entry.engine_label.clone());
            let turn_items = items_by_turn.remove(&turn.turn_id).unwrap_or_default();

            // --- Session derivation pre-pass (R1/H): exact run evidence only.
            // Steering anchors in this turn, by ordinal: items after an
            // anchor render top-level and never join session details.
            let mut steer_anchor_ordinals: Vec<u64> = Vec::new();
            let mut saw_user_message = false;
            for item in &turn_items {
                if !matches!(
                    &item.kind,
                    SceneItemKind::UserMessage { .. } | SceneItemKind::MultimodalUserMessage { .. }
                ) {
                    continue;
                }
                // Durable mid-run sends are user messages in the existing
                // turn. They are boundaries even without an optional label.
                if saw_user_message || steerings_by_anchor.contains_key(item.id.as_str()) {
                    steer_anchor_ordinals.push(item.ordinal);
                }
                saw_user_message = true;
            }
            let min_steer_ordinal: Option<u64> = steer_anchor_ordinals.into_iter().min();
            let is_post_steer =
                |ordinal: u64| min_steer_ordinal.is_some_and(|anchor| ordinal > anchor);

            // Runs with session content: assistant runs plus work-fact runs
            // carrying run attribution. Runs are never parsed, guessed, or
            // defaulted; unattributed content selects the legacy layout.
            let mut content_runs: HashMap<String, RunId> = HashMap::new();
            for item in &turn_items {
                let run_id: Option<&RunId> = match &item.kind {
                    SceneItemKind::AssistantMessage { .. }
                    | SceneItemKind::ReasoningSummary { .. }
                    | SceneItemKind::Activity { .. }
                    | SceneItemKind::Compaction { .. }
                    | SceneItemKind::NativeFact { .. } => item
                        .provenance
                        .as_ref()
                        .and_then(|provenance| provenance.run_id.as_ref()),
                    _ => None,
                };
                if let Some(run_id) = run_id {
                    content_runs
                        .entry(run_id.as_str().to_owned())
                        .or_insert_with(|| run_id.clone());
                }
            }
            // Exactly one content run mirrors the reference single session;
            // zero runs keep the legacy positional layout and several runs
            // dissolve grouping the same flat way (no group, prose top-level).
            let session_run: Option<RunId> = if content_runs.len() == 1 {
                content_runs.values().next().cloned()
            } else {
                None
            };
            let session_mode = session_run.is_some();

            // Final promotion (reference store.ts:626-702): the latest
            // non-commentary non-empty reply plus, independently, the latest
            // explicit final; explicit final wins unless newest-phase reply
            // promotes phaseless prose while current; settled-last promotes
            // the completed last reply of a completed turn. Exactly one reply
            // id per turn at most.
            let mut latest_reply: Option<(u64, SceneId)> = None;
            let mut latest_final: Option<(u64, SceneId)> = None;
            for item in &turn_items {
                if let SceneItemKind::AssistantMessage { body, phase } = &item.kind {
                    if *phase == AssistantPhase::Commentary || body.is_empty() {
                        continue;
                    }
                    if latest_reply
                        .as_ref()
                        .is_none_or(|(ordinal, _)| item.ordinal > *ordinal)
                    {
                        latest_reply = Some((item.ordinal, item.id.clone()));
                    }
                    if *phase == AssistantPhase::Final
                        && latest_final
                            .as_ref()
                            .is_none_or(|(ordinal, _)| item.ordinal > *ordinal)
                    {
                        latest_final = Some((item.ordinal, item.id.clone()));
                    }
                }
            }
            let mut promoted_id: Option<SceneId> = latest_final.map(|(_, id)| id);
            let mut newest_reply_ord: Option<u64> = None;
            let mut newest_work_ord: Option<u64> = None;
            for item in &turn_items {
                match &item.kind {
                    SceneItemKind::AssistantMessage { body, phase }
                        if *phase != AssistantPhase::Commentary && !body.is_empty() =>
                    {
                        newest_reply_ord = Some(
                            newest_reply_ord.map_or(item.ordinal, |ord| ord.max(item.ordinal)),
                        );
                    }
                    SceneItemKind::Activity { .. } => {
                        newest_work_ord =
                            Some(newest_work_ord.map_or(item.ordinal, |ord| ord.max(item.ordinal)));
                    }
                    SceneItemKind::ReasoningSummary { body } if !body.is_empty() => {
                        newest_work_ord =
                            Some(newest_work_ord.map_or(item.ordinal, |ord| ord.max(item.ordinal)));
                    }
                    _ => {}
                }
            }
            let progress = match (newest_reply_ord, newest_work_ord) {
                (None, None) => ProgressPhase::None,
                (Some(_), None) => ProgressPhase::Reply,
                (None, Some(_)) => ProgressPhase::Work,
                (Some(reply), Some(work)) => {
                    if reply > work {
                        ProgressPhase::Reply
                    } else {
                        ProgressPhase::Work
                    }
                }
            };
            if progress == ProgressPhase::Reply {
                promoted_id = latest_reply.map(|(_, id)| id);
            }
            if session_mode {
                // Settled-last promotes the completed last reply of a
                // completed turn only. Failed/Cancelled turns are not
                // completed work sessions; without an independent session
                // lifecycle the limitation is stated, not equated away.
                if turn.lifecycle == ConversationLifecycle::Completed {
                    let mut candidate: Option<(u64, SceneId)> = None;
                    for item in &turn_items {
                        if let SceneItemKind::AssistantMessage { body, phase } = &item.kind {
                            let completed = item
                                .provenance
                                .as_ref()
                                .and_then(|provenance| provenance.lifecycle)
                                == Some(ConversationLifecycle::Completed);
                            if *phase == AssistantPhase::Commentary || body.is_empty() || !completed
                            {
                                continue;
                            }
                            if candidate
                                .as_ref()
                                .is_none_or(|(ordinal, _)| item.ordinal > *ordinal)
                            {
                                candidate = Some((item.ordinal, item.id.clone()));
                            }
                        }
                    }
                    if let Some((_, id)) = candidate {
                        promoted_id = Some(id);
                    }
                }
            }
            if let Some(id) = &promoted_id {
                promoted.insert(turn.turn_id.clone(), id.clone());
            }

            // Detail membership (session mode): work kinds, commentary, and
            // non-promoted assistants — except post-steer items, which stay
            // top-level, and WorkSession markers, which the session consumes.
            // The anchor sits at the earliest session-owned position so late
            // work joins before the final reply.
            let mut detail_ids: HashSet<String> = HashSet::new();
            let mut marker_ids: HashSet<String> = HashSet::new();
            let mut fold_transition_id: Option<String> = None;
            let mut anchor_pos: Option<u64> = None;
            let mut track_anchor = |ordinal: u64| {
                anchor_pos = Some(anchor_pos.map_or(ordinal, |pos| pos.min(ordinal)));
            };
            if session_mode {
                for item in &turn_items {
                    if promoted_id.as_ref().is_some_and(|reply| &item.id == reply) {
                        track_anchor(item.ordinal);
                    }
                    if is_post_steer(item.ordinal) {
                        continue;
                    }
                    match &item.kind {
                        SceneItemKind::ReasoningSummary { .. }
                        | SceneItemKind::Activity { .. }
                        | SceneItemKind::Compaction { .. }
                        | SceneItemKind::NativeFact { .. } => {
                            detail_ids.insert(item.id.as_str().to_owned());
                            track_anchor(item.ordinal);
                        }
                        SceneItemKind::AssistantMessage { .. } => {
                            if promoted_id.as_ref().is_none_or(|reply| &item.id != reply) {
                                detail_ids.insert(item.id.as_str().to_owned());
                                track_anchor(item.ordinal);
                            }
                        }
                        SceneItemKind::WorkSession { .. } => {
                            marker_ids.insert(item.id.as_str().to_owned());
                        }
                        SceneItemKind::ModelTransition { .. } if fold_transition_id.is_none() => {
                            fold_transition_id = Some(item.id.as_str().to_owned());
                        }
                        _ => {}
                    }
                }
            }

            // Live-reply and tool-progress inputs (session mode only): a
            // genuine reply (Final/Unspecified, live lifecycle, non-empty,
            // and the newest phase) suppresses the row as its own status,
            // while commentary never does; a live tool chain newer than
            // model prose suppresses it the same way. Tool liveness comes
            // from typed lifecycles only — unknown or settled tools never
            // wait. Model prose is non-empty assistant text of any phase
            // plus non-empty reasoning summaries. Legacy inputs never
            // suppress.
            let mut live_reply = false;
            let mut newest_model_ord: Option<u64> = None;
            let mut newest_tool_ord: Option<u64> = None;
            if session_mode {
                for item in &turn_items {
                    match &item.kind {
                        SceneItemKind::AssistantMessage { body, phase } => {
                            let live = item
                                .provenance
                                .as_ref()
                                .and_then(|provenance| provenance.lifecycle)
                                .is_some_and(is_live_lifecycle);
                            if live
                                && *phase != AssistantPhase::Commentary
                                && !body.is_empty()
                                && progress == ProgressPhase::Reply
                            {
                                live_reply = true;
                            }
                            if !body.is_empty() {
                                newest_model_ord = Some(
                                    newest_model_ord
                                        .map_or(item.ordinal, |ord| ord.max(item.ordinal)),
                                );
                            }
                        }
                        SceneItemKind::ReasoningSummary { body } if !body.is_empty() => {
                            newest_model_ord = Some(
                                newest_model_ord.map_or(item.ordinal, |ord| ord.max(item.ordinal)),
                            );
                        }
                        SceneItemKind::Activity { .. } => {
                            let live = item
                                .provenance
                                .as_ref()
                                .and_then(|provenance| provenance.lifecycle)
                                .is_some_and(is_live_lifecycle);
                            if live {
                                newest_tool_ord = Some(
                                    newest_tool_ord
                                        .map_or(item.ordinal, |ord| ord.max(item.ordinal)),
                                );
                            }
                        }
                        _ => {}
                    }
                }
            }
            let waiting_for_activity = narration.is_active_work()
                && newest_tool_ord
                    .is_some_and(|tool| newest_model_ord.is_none_or(|model| tool > model));

            // The anchor is render-only grouping identity, resolved eagerly
            // so a malformed identity fails the atomic build explicitly.
            let anchor_id: Option<SceneId> = if session_mode {
                Some(session_anchor_id(&turn.turn_id)?)
            } else {
                None
            };

            let mut blocks = Vec::new();
            let mut work_buffer = Vec::new();
            let mut work_disclosure = None;
            let mut change_id = None;
            let mut change_disclosure = None;
            let mut change_files = Vec::new();
            let mut top_level_compaction = false;
            let mut group_index: Option<usize> = None;
            let mut pending_transition: Option<ModelTransitionBlock> = None;
            let mut summary_all: Option<(u64, String)> = None;
            let mut summary_scoped: Option<(u64, String)> = None;
            let mut engine_label: Option<String> = None;

            // The session group emits at the anchor position even when later
            // details are still ahead: later members append into the emitted
            // group by index, so bodies move exactly once.
            let emit_session_group =
                |blocks: &mut Vec<TurnBlock>,
                 group_index: &mut Option<usize>,
                 pending_transition: &mut Option<ModelTransitionBlock>| {
                    blocks.push(TurnBlock::WorkGroup(WorkGroupBlock {
                        items: Vec::new(),
                        label: None,
                        disclosure: session_disclosure,
                        session: anchor_id.clone(),
                        session_run: session_run.clone(),
                        superseded: false,
                        reasoning_summary: None,
                        progress,
                        transition: pending_transition.take(),
                        session_details: Vec::new(),
                    }));
                    *group_index = Some(blocks.len() - 1);
                };

            for item in turn_items {
                // Session-owned content never reaches the positional arms.
                // The group emits at the anchor position first; later members
                // append into it by index, so bodies move exactly once.
                if session_mode
                    && group_index.is_none()
                    && anchor_pos.is_some_and(|pos| item.ordinal >= pos)
                {
                    emit_session_group(&mut blocks, &mut group_index, &mut pending_transition);
                }
                if session_mode && detail_ids.contains(item.id.as_str()) {
                    let Some(TurnBlock::WorkGroup(group)) =
                        group_index.and_then(|index| blocks.get_mut(index))
                    else {
                        continue;
                    };
                    match item.kind {
                        SceneItemKind::Activity { body, kind, detail } => {
                            group.session_details.push(SessionDetail::Activity {
                                id: item.id,
                                body,
                                kind,
                                detail,
                                lifecycle: item
                                    .provenance
                                    .as_ref()
                                    .and_then(|provenance| provenance.lifecycle),
                                ordinal: item.ordinal,
                                disclosure: item.disclosure,
                            });
                        }
                        SceneItemKind::ReasoningSummary { body } => {
                            // Reasoning feeds the one live summary line only;
                            // it never becomes a visible row (R2).
                            if !body.is_empty() {
                                let run_matches = match (
                                    &session_run,
                                    item.provenance
                                        .as_ref()
                                        .and_then(|provenance| provenance.run_id.as_ref()),
                                ) {
                                    (None, _) | (_, None) => true,
                                    (Some(session), Some(run)) => session == run,
                                };
                                if run_matches
                                    && summary_scoped
                                        .as_ref()
                                        .is_none_or(|(ordinal, _)| item.ordinal > *ordinal)
                                {
                                    summary_scoped = Some((item.ordinal, body.clone()));
                                }
                                if summary_all
                                    .as_ref()
                                    .is_none_or(|(ordinal, _)| item.ordinal > *ordinal)
                                {
                                    summary_all = Some((item.ordinal, body));
                                }
                            }
                        }
                        SceneItemKind::AssistantMessage { body, phase } => {
                            group.session_details.push(SessionDetail::Assistant {
                                id: item.id,
                                body,
                                phase,
                                ordinal: item.ordinal,
                                provenance: item.provenance,
                                disclosure: item.disclosure,
                            });
                        }
                        SceneItemKind::Compaction { summary } => {
                            group.session_details.push(SessionDetail::Compaction {
                                id: item.id,
                                summary,
                                ordinal: item.ordinal,
                                disclosure: item.disclosure,
                            });
                        }
                        SceneItemKind::NativeFact { text } => {
                            group.session_details.push(SessionDetail::NativeFact {
                                id: item.id,
                                text,
                                ordinal: item.ordinal,
                                disclosure: item.disclosure,
                            });
                        }
                        _ => {
                            unreachable!("detail membership covers only detail kinds");
                        }
                    }
                    continue;
                }
                if session_mode && marker_ids.contains(item.id.as_str()) {
                    // WorkSession markers are consumed as the session signal;
                    // the session group itself is their rendering.
                    continue;
                }
                if session_mode && fold_transition_id.as_deref() == Some(item.id.as_str()) {
                    if let SceneItemKind::ModelTransition {
                        from_model,
                        to_model,
                    } = item.kind
                    {
                        let block = ModelTransitionBlock {
                            id: item.id,
                            from_model,
                            to_model,
                            disclosure: item.disclosure,
                        };
                        engine_label = Some(block.to_model.clone());
                        if let Some(group_index) = group_index {
                            let TurnBlock::WorkGroup(group) = &mut blocks[group_index] else {
                                unreachable!("session index always addresses its group");
                            };
                            group.transition = Some(block);
                        } else {
                            pending_transition = Some(block);
                        }
                    }
                    continue;
                }
                // Change facts are a barrier even though the card is rendered
                // at the terminal position. This preserves *contiguous* work
                // grouping around a deferred or settled change event.
                if matches!(
                    &item.kind,
                    SceneItemKind::ChangeSet { .. } | SceneItemKind::FileChange { .. }
                ) {
                    flush_work(&mut work_buffer, &mut work_disclosure, &mut blocks)?;
                    match item.kind {
                        SceneItemKind::ChangeSet { files } => {
                            if change_id.is_none() {
                                change_id = Some(item.id);
                                change_disclosure = item.disclosure;
                            } else if change_disclosure.is_none() {
                                change_disclosure = item.disclosure;
                            }
                            change_files.extend(files);
                        }
                        SceneItemKind::FileChange { file } => {
                            if change_id.is_none() {
                                change_id = Some(item.id);
                                change_disclosure = item.disclosure;
                            } else if change_disclosure.is_none() {
                                change_disclosure = item.disclosure;
                            }
                            change_files.push(file);
                        }
                        _ => unreachable!("change barrier matched only change variants"),
                    }
                    if change_files.len() > SCENE_MAX_CHANGED_FILES_PER_CARD {
                        return Err(SceneBuildError::TooManyChangedFiles {
                            count: change_files.len(),
                            maximum: SCENE_MAX_CHANGED_FILES_PER_CARD,
                        });
                    }
                    continue;
                }

                let is_work_like = matches!(
                    &item.kind,
                    SceneItemKind::ReasoningSummary { .. }
                        | SceneItemKind::Activity { .. }
                        | SceneItemKind::WorkSession { .. }
                );
                if is_work_like {
                    if work_buffer.len() >= SCENE_MAX_WORK_GROUP_ITEMS {
                        return Err(SceneBuildError::TooManyWorkItems {
                            count: work_buffer.len() + 1,
                            maximum: SCENE_MAX_WORK_GROUP_ITEMS,
                        });
                    }
                    let work_item = match item.kind {
                        SceneItemKind::ReasoningSummary { body } => WorkItem::Reasoning {
                            id: item.id,
                            body,
                            disclosure: item.disclosure,
                        },
                        SceneItemKind::Activity { body, kind, detail } => WorkItem::Activity {
                            id: item.id,
                            body,
                            kind,
                            detail,
                            lifecycle: item
                                .provenance
                                .as_ref()
                                .and_then(|provenance| provenance.lifecycle),
                            disclosure: item.disclosure,
                        },
                        SceneItemKind::WorkSession { title } => WorkItem::WorkSession {
                            id: item.id,
                            title,
                            disclosure: item.disclosure,
                        },
                        _ => unreachable!("work-like match covered every work variant"),
                    };
                    if work_disclosure.is_none() {
                        work_disclosure = work_item_disclosure(&work_item);
                    }
                    work_buffer.push(work_item);
                    continue;
                }

                // Messages, cards, and facts are all work-group barriers.
                flush_work(&mut work_buffer, &mut work_disclosure, &mut blocks)?;

                match item.kind {
                    SceneItemKind::UserMessage { body } => {
                        let anchor_key = item.id.as_str().to_owned();
                        blocks.push(TurnBlock::UserMessage(UserMessageBlock {
                            attachments: Vec::new(),
                            id: item.id,
                            body,
                            disclosure: item.disclosure,
                        }));
                        if let Some(placements) = steerings_by_anchor.remove(&anchor_key) {
                            for placement in placements {
                                blocks.push(TurnBlock::SteeringLabel(SteeringBlock {
                                    id: placement.id,
                                    anchor: placement.anchor,
                                    label: placement.label,
                                }));
                            }
                        }
                    }
                    SceneItemKind::MultimodalUserMessage { body, attachments } => {
                        let anchor_key = item.id.as_str().to_owned();
                        blocks.push(TurnBlock::UserMessage(UserMessageBlock {
                            attachments,
                            id: item.id,
                            body,
                            disclosure: item.disclosure,
                        }));
                        if let Some(placements) = steerings_by_anchor.remove(&anchor_key) {
                            for placement in placements {
                                blocks.push(TurnBlock::SteeringLabel(SteeringBlock {
                                    id: placement.id,
                                    anchor: placement.anchor,
                                    label: placement.label,
                                }));
                            }
                        }
                    }
                    SceneItemKind::AssistantMessage { body, phase } => {
                        blocks.push(TurnBlock::AssistantMessage(AssistantMessageBlock {
                            id: item.id,
                            body,
                            phase,
                            provenance: item.provenance,
                            disclosure: item.disclosure,
                        }));
                    }
                    SceneItemKind::Compaction { summary } => {
                        top_level_compaction = true;
                        blocks.push(TurnBlock::Compaction(CompactionBlock {
                            id: item.id,
                            summary,
                            disclosure: item.disclosure,
                        }));
                    }
                    SceneItemKind::Plan { title, entries } => {
                        blocks.push(TurnBlock::Plan(PlanBlock {
                            id: item.id,
                            title,
                            entries,
                            disclosure: item.disclosure,
                        }));
                    }
                    SceneItemKind::Approval { prompt } => {
                        blocks.push(TurnBlock::Approval(ApprovalBlock {
                            id: item.id,
                            prompt,
                            disclosure: item.disclosure,
                        }));
                    }
                    SceneItemKind::Question { prompt } => {
                        blocks.push(TurnBlock::Question(QuestionBlock {
                            id: item.id,
                            prompt,
                            disclosure: item.disclosure,
                        }));
                    }
                    SceneItemKind::Error { message } => {
                        blocks.push(TurnBlock::Error(ErrorBlock {
                            id: item.id,
                            message,
                            disclosure: item.disclosure,
                        }));
                    }
                    SceneItemKind::UsageInterruption { detail } => {
                        blocks.push(TurnBlock::UsageInterruption(UsageInterruptionBlock {
                            id: item.id,
                            detail,
                            disclosure: item.disclosure,
                        }));
                    }
                    SceneItemKind::ModelTransition {
                        from_model,
                        to_model,
                    } => {
                        blocks.push(TurnBlock::ModelTransition(ModelTransitionBlock {
                            id: item.id,
                            from_model,
                            to_model,
                            disclosure: item.disclosure,
                        }));
                    }
                    SceneItemKind::NativeFact { text } => {
                        blocks.push(TurnBlock::NativeFact(NativeFactBlock {
                            id: item.id,
                            text,
                            disclosure: item.disclosure,
                        }));
                    }
                    SceneItemKind::ReasoningSummary { .. }
                    | SceneItemKind::Activity { .. }
                    | SceneItemKind::WorkSession { .. }
                    | SceneItemKind::ChangeSet { .. }
                    | SceneItemKind::FileChange { .. } => {
                        unreachable!("all work and change variants were handled above")
                    }
                }
            }

            flush_work(&mut work_buffer, &mut work_disclosure, &mut blocks)?;

            // Finalize the session group: live summary, engine label, and
            // superseded state resolve here, once the whole turn was seen.
            // A superseded session never narrates: later content in the same
            // turn owns the live line.
            let live_summary = (summary_scoped.or(summary_all))
                .filter(|_| narration.is_active_work())
                .map(|(_, body)| body);
            if let Some(group_index) = group_index {
                let is_last_content = group_index == blocks.len().saturating_sub(1);
                let TurnBlock::WorkGroup(group) = &mut blocks[group_index] else {
                    unreachable!("session index always addresses its group");
                };
                group.superseded = !is_last_content;
                group.reasoning_summary.clone_from(&live_summary);
            }

            // A duration narration is a terminal label, not a label on every
            // historical work fragment. Prefer the session group when the
            // turn has one; otherwise keep the latest positional group.
            if let Some(label) = narration.terminal_label() {
                let session_at = blocks.iter().position(
                    |block| matches!(block, TurnBlock::WorkGroup(group) if group.session.is_some()),
                );
                let positional_at = blocks
                    .iter()
                    .rposition(|block| matches!(block, TurnBlock::WorkGroup(_)));
                if let Some(index) = session_at.or(positional_at)
                    && let TurnBlock::WorkGroup(group) = &mut blocks[index]
                {
                    group.label = Some(label);
                }
            }

            if let Some(change_id) = change_id
                && !change_files.is_empty()
            {
                let card = ChangeSetBlock {
                    id: change_id,
                    files: change_files,
                    disclosure: change_disclosure,
                };
                if turn.lifecycle.is_terminal() {
                    // Terminal cards are deliberately appended before the
                    // status/footer pair, regardless of item ordinal.
                    blocks.push(TurnBlock::ChangeSet(card));
                } else {
                    deferred.push(DeferredChangeSet {
                        turn_id: turn.turn_id.clone(),
                        card,
                    });
                }
            }

            if top_level_compaction
                && matches!(narration, TurnNarration::Thinking | TurnNarration::Working)
            {
                return Err(SceneBuildError::CompactionNarrationConflict { narration });
            }

            // A genuine live reply is its own status; commentary never
            // suppresses as if it were a reply. Waiting tool progress
            // suppresses the same way. Legacy inputs never suppress.
            let suppress_status = (live_reply || waiting_for_activity)
                && (narration == TurnNarration::StreamingSuppression || narration.is_active_work());
            if !suppress_status {
                blocks.push(TurnBlock::TurnStatus(TurnStatusBlock {
                    narration,
                    active_started_at_ms,
                    reasoning_summary: live_summary.filter(|_| session_mode),
                    engine_label: explicit_engine_label
                        .clone()
                        .or(engine_label.filter(|_| session_mode)),
                    engine: entry.and_then(|entry| entry.engine),
                }));
            }
            blocks.push(TurnBlock::TurnFooter(TurnFooterBlock {
                turn_id: turn.turn_id.clone(),
                settlement: None,
            }));

            turn_scenes.push(TurnScene {
                turn_id: turn.turn_id.clone(),
                ordinal: turn.ordinal,
                lifecycle: turn.lifecycle,
                blocks,
            });
        }

        if let Some(placement) = steerings_by_anchor
            .values()
            .next()
            .and_then(|placements| placements.first())
        {
            return Err(SceneBuildError::SteeringAnchorNotPlaced {
                anchor: placement.anchor.clone(),
            });
        }

        // Turns were already sorted, but retain the final sort as a local
        // invariant if construction changes later.
        turn_scenes.sort_by_key(|scene| scene.ordinal);

        Ok(Self {
            turn_scenes,
            deferred,
            promoted,
        })
    }
}
