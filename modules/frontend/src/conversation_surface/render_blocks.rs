//! Turn, message, work-group, and trace-chain block rendering for
//! [`ConversationSurface`].
//!
//! Extracted verbatim from `conversation_surface.rs` during the phase-2 module
//! split; visibility was widened to `pub(super)` for parent-owned methods.

use super::*;

impl ConversationSurface {
    #[expect(
        clippy::too_many_arguments,
        reason = "one GPUI turn builder needs the scene, entity, theme, anchors, window, motion, and context it threads through its children"
    )]
    pub(super) fn render_turn(
        &self,
        turn: &TurnScene,
        entity: &Entity<Self>,
        theme: &ArtisanTheme,
        anchors: &mut ScrollAnchorRegistry<'_>,
        window: &mut Window,
        status_motion: MotionPolicy,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selector = turn_selector(&turn.turn_id);
        let turn_element = div()
            .w_full()
            .max_w(px(TRANSCRIPT_PROSE_WIDTH_PX))
            .mx_auto()
            .px(px(TRANSCRIPT_GUTTER_PX))
            .relative()
            .group(TURN_GROUP)
            .flex()
            .flex_col()
            // Reference intra-turn rhythm is `gap-[1lh]`: no app override in
            // lib/styles, so Tailwind preflight 1.5 on the 16 px base applies
            // and one line height is 24 px.
            .gap(theme.spacing.steps(6.0));
        // At most one group owns the live Thinking/Working line: the latest
        // group headers it once from the same accepted narration and clock,
        // and the separate status row below stands down unless it narrates
        // something distinct. Terminal labels always win over the live line
        // inside the group.
        let live_header: Option<String> = turn_owner_header(turn, self.active_now_ms);
        let live_owner = if live_header.is_some() {
            owning_group_index(turn)
        } else {
            None
        };
        // Identities mirror the children pushed below in block order. A
        // suppressed status row and an unsettled footer paint no child, so
        // neither contributes a slot.
        let turn_has_work_group = turn
            .blocks()
            .iter()
            .any(|block| matches!(block, TurnBlock::WorkGroup(_)));
        let child_identities: Vec<(Option<SceneId>, Option<ItemId>)> = turn
            .blocks()
            .iter()
            .filter_map(|block| {
                if let TurnBlock::TurnStatus(status) = block {
                    // Same paint decision as render_status: structural rule
                    // plus identical-duplicate suppression, so measured
                    // children and identities stay one-to-one.
                    let copy = turn_status_copy_text(
                        status.narration,
                        status.active_started_at_ms,
                        self.active_now_ms,
                        status.reasoning_summary.as_deref(),
                        status.engine_label.as_deref(),
                    );
                    let owner = if matches!(
                        status.narration,
                        TurnNarration::Thinking | TurnNarration::Working
                    ) {
                        turn_owner_header(turn, self.active_now_ms)
                    } else {
                        None
                    };
                    if !turn_status_paints(
                        turn_has_work_group,
                        status.narration,
                        copy.as_deref(),
                        owner.as_deref(),
                    ) {
                        return None;
                    }
                }
                if let TurnBlock::TurnFooter(footer) = block
                    && !footer_has_content(footer)
                {
                    return None;
                }
                Some(block_scroll_identity(&turn.turn_id, block))
            })
            .collect();
        let surface = entity.downgrade();
        let turn_element =
            turn_element.on_children_prepainted(move |children_bounds, window, app| {
                let _ = surface.update(app, |surface, _| {
                    surface.apply_executed_scroll_targets(
                        &child_identities,
                        &children_bounds,
                        window,
                    );
                });
            });
        let turn_element = anchors.attach(
            turn_element,
            SceneId::parse(turn.turn_id.as_str()).ok().as_ref(),
            None,
        );
        let mut turn_element = turn_element.debug_selector(move || selector.clone());

        for (block_index, block) in turn.blocks().iter().enumerate() {
            if let Some(element) = self.render_block(
                &turn.turn_id,
                turn.lifecycle,
                block,
                entity,
                theme,
                anchors,
                &mut *window,
                status_motion,
                block_index,
                &live_header,
                live_owner,
                cx,
            ) {
                turn_element = turn_element.child(element);
            }
        }

        turn_element.into_any_element()
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "one block dispatcher threads the scene row, entity, theme, anchors, window, motion, and live-owner index to every block renderer"
    )]
    #[expect(
        clippy::ref_option,
        reason = "the live-owner header is an owned scene string that renderers read as a whole; an Option<&str> would only push the borrow outward"
    )]
    pub(super) fn render_block(
        &self,
        turn_id: &TurnId,
        turn_lifecycle: ConversationLifecycle,
        block: &TurnBlock,
        entity: &Entity<Self>,
        theme: &ArtisanTheme,
        anchors: &mut ScrollAnchorRegistry<'_>,
        window: &mut Window,
        status_motion: MotionPolicy,
        block_index: usize,
        turn_live_header: &Option<String>,
        live_owner_index: Option<usize>,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let selector = block_selector(turn_id, block);
        match block {
            TurnBlock::UserMessage(block) => {
                Some(self.render_user_message(block, selector, theme, cx))
            }
            TurnBlock::AssistantMessage(block) => {
                Some(self.render_assistant_message(block, selector, entity, theme, anchors))
            }
            TurnBlock::WorkGroup(block) => {
                // Only the owning (latest) group headers the live line, so
                // it paints exactly once per turn.
                let owned = if live_owner_index == Some(block_index) {
                    turn_live_header.clone()
                } else {
                    None
                };
                // Mounted-working freezes in retained window state on first
                // mount: history that later goes live must never enter.
                let mounted_working = {
                    let working_now = owned.is_some();
                    let state = window.use_keyed_state(
                        ElementId::Name(SharedString::from(format!("{selector}-mounted-working"))),
                        cx,
                        move |_, _| working_now,
                    );
                    *state.read(cx)
                };
                Some(self.render_work_group(
                    turn_id,
                    turn_lifecycle,
                    block,
                    &selector,
                    entity,
                    theme,
                    anchors,
                    owned,
                    status_motion,
                    mounted_working,
                    &mut *window,
                    cx,
                ))
            }
            TurnBlock::Compaction(block) => {
                Some(self.render_compaction(block, selector, entity, theme, anchors))
            }
            TurnBlock::ChangeSet(block) => {
                Some(self.render_change_set(block, selector, entity, theme, anchors))
            }
            TurnBlock::Plan(block) => {
                Some(self.render_plan(block, selector, entity, theme, anchors))
            }
            TurnBlock::Approval(block) => {
                Some(self.render_approval(block, selector, entity, theme, anchors))
            }
            TurnBlock::Question(block) => {
                Some(self.render_question(block, selector, entity, theme, anchors))
            }
            TurnBlock::Error(block) => {
                Some(Self::render_error(block, selector, entity, theme, anchors))
            }
            TurnBlock::UsageInterruption(block) => {
                Some(self.render_usage_interruption(block, selector, entity, theme, anchors))
            }
            TurnBlock::ModelTransition(block) => {
                Some(self.render_model_transition(block, selector, entity, theme, anchors))
            }
            TurnBlock::NativeFact(block) => {
                Some(self.render_native_fact(block, selector, entity, theme, anchors))
            }
            TurnBlock::SteeringLabel(block) => {
                Some(Self::render_steering(block, selector, theme, anchors))
            }
            TurnBlock::TurnStatus(block) => {
                self.render_status(turn_id, block, selector, theme, status_motion)
            }
            TurnBlock::TurnFooter(block) => self.render_footer(
                turn_id,
                block,
                selector,
                entity,
                theme,
                window,
                status_motion,
            ),
        }
    }

    pub(super) fn render_user_message(
        &self,
        block: &UserMessageBlock,
        selector: String,
        theme: &ArtisanTheme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // Parity with conversation-message.svelte user branch: right-aligned
        // gradient bubble, rounded-2xl, plain pre-wrap paragraph, no title.
        // Reference `bg-linear-to-t from-surface-850 to-surface-775` lays the
        // from-stop at the bottom, so the native face runs S775 (top) to
        // S850 (bottom) through the existing two-stop GPUI gradient.
        let body_selector = format!("{selector}-body");
        let mut message = div().w_full().flex().flex_col().items_end().gap(px(8.0));
        if let Some(images) = self.message_images.as_ref() {
            let mut tray = div()
                .flex()
                .flex_wrap()
                .justify_end()
                .gap(px(8.0))
                .max_w(px(576.0));
            for reference in &block.attachments {
                let tile = images.update(cx, |images, cx| {
                    images
                        .render_thumbnail(reference, *theme, cx)
                        .into_any_element()
                });
                tray = tray.child(tile);
            }
            if !block.attachments.is_empty() {
                message = message.child(tray);
            }
        }
        if !block.body.is_empty() {
            // The body text is the shared selectable element: retained state
            // (selection, drag latch, focus) lives in framework element state
            // under the stable body id across frames, with no caller maps or
            // focus handles. Styling stays on the container (prose size,
            // 28 px line height, 410 weight, bubble metrics, gradient face
            // with the reference card shadow beneath), so no duplicate body,
            // glyph, or padding is introduced.
            let body_id = SharedString::from(body_selector.clone());
            message = message.child(
                div()
                    .max_w(px(576.0))
                    .rounded(RadiusTokens::value(RadiusStep::X2l))
                    .bg(vertical_gradient(
                        SurfaceStep::S775.oklch(),
                        SurfaceStep::S850.oklch(),
                    ))
                    .shadow(
                        theme
                            .elevation
                            .card_shadow
                            .iter()
                            .map(|layer| layer.to_box_shadow())
                            .collect::<Vec<_>>(),
                    )
                    .px(px(16.0))
                    .py(px(12.0))
                    .debug_selector(move || selector.clone())
                    .child(
                        div()
                            .w_full()
                            .min_w_0()
                            .text_size(px(ProseTypography::BODY_SIZE_PX))
                            .line_height(px(ProseTypography::BODY_LINE_PX))
                            .font_weight(ProseTypography::BODY_WEIGHT)
                            .letter_spacing(px(ProseTypography::BODY_TRACKING_PX))
                            .whitespace_normal()
                            .debug_selector(move || body_selector.clone())
                            .child(SelectableText::retained(
                                body_id,
                                block.body.clone(),
                                *theme,
                                Vec::new(),
                            )),
                    ),
            );
        }
        message.into_any_element()
    }

    pub(super) fn render_assistant_message(
        &self,
        block: &crate::conversation_scene::AssistantMessageBlock,
        selector: String,
        _entity: &Entity<Self>,
        theme: &ArtisanTheme,
        _anchors: &mut ScrollAnchorRegistry<'_>,
    ) -> AnyElement {
        // Parity with conversation-message.svelte assistant branch: chromeless
        // markdown at prose width, no card, no title. The reply body reads in
        // the foreground token, including assistant prose in work history.
        // Shaping stays inside the per-row render
        // budget so one pathological body cannot parse every frame.
        let rendered_body = self.render_budgeted_markdown(
            &block.body,
            theme,
            selector.clone(),
            MarkdownBodyTone::Foreground,
        );
        div()
            .w_full()
            .max_w(px(672.0))
            .debug_selector(move || selector.clone())
            .child(rendered_body)
            .into_any_element()
    }

    pub(super) fn render_text_block(
        &self,
        params: TextBlockRender<'_>,
        entity: &Entity<Self>,
        theme: &ArtisanTheme,
        anchors: &mut ScrollAnchorRegistry<'_>,
    ) -> AnyElement {
        let TextBlockRender {
            id,
            disclosure,
            selector,
            title,
            body,
        } = params;
        let style = CardStyle::resolve(*theme);
        self.render_controlled_card(
            ControlledCardOptions {
                id: id.clone(),
                item_id: item_id_for_scene_id(id),
                disclosure,
                selector,
                style,
            },
            compact_card_content(style).child(card_heading(title, theme)),
            compact_card_content(style).child(body_text(body, theme)),
            entity,
            anchors,
        )
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "one GPUI work-group builder threads the scene row, selector, entity, theme, anchors, owners, motion, and window its children share"
    )]
    #[expect(
        clippy::too_many_lines,
        reason = "one GPUI builder composes the work-group rail, disclosure chrome, and activity rows in visual order"
    )]
    pub(super) fn render_work_group(
        &self,
        turn_id: &TurnId,
        turn_lifecycle: ConversationLifecycle,
        block: &WorkGroupBlock,
        selector: &str,
        entity: &Entity<Self>,
        theme: &ArtisanTheme,
        anchors: &mut ScrollAnchorRegistry<'_>,
        live_header: Option<String>,
        status_motion: MotionPolicy,
        mounted_working: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // The stable anchor prefers the session id (disclosure/scroll key);
        // legacy positional groups fall back to the first-item derivation.
        let group_id = block
            .session
            .clone()
            .or_else(|| work_group_anchor_id(turn_id, block));
        // Terminal duration wins; otherwise the owning group headers the
        // turn's live Thinking/Working line once (see render_turn). Earlier
        // groups and the separate status row stand down, so the line paints
        // exactly once per turn.
        let terminal = work_group_header_copy(block.label);
        let owns_live_header = live_header.is_some();
        let header = terminal.or(live_header);
        // Engine handoffs fold into the header far end, never as standalone
        // timeline rows while a session hosts them.
        let transition = block
            .transition
            .as_ref()
            .map(|handoff| format!("{} → {}", handoff.from_model, handoff.to_model));

        // Controlled state is never overridden: Closed hides the panel in
        // every case, and the toggle always flows through the existing
        // disclosure action. Uncontrolled groups always show their items,
        // exactly like the previous static branch did. Control additionally
        // requires actual visible trace content: an empty `Thought for …`
        // group paints its header with no chevron at all, never a blank
        // disclosure.
        let controlled = group_id.is_some()
            && block.disclosure.is_some()
            && work_group_has_visible_details(block);
        let open = !controlled || !matches!(block.disclosure, Some(SceneDisclosure::Closed));
        let items_mounted =
            !controlled || !matches!(block.disclosure, Some(SceneDisclosure::Closed));

        let mut items = div()
            .w_full()
            .flex()
            .flex_col()
            .gap(theme.spacing.steps(2.0));
        let rows = ordered_detail_rows(block);
        // Consecutive activity rows form one collapsible trace chain, exactly
        // like the reference segmentation: anything else (assistant prose,
        // compaction, native fact) is a seam that ends the chain. Chains keep
        // their rows as their own children so each row's measured bound stays
        // reachable by the scroll-target handoff below.
        let mut child_identities: Vec<(Option<SceneId>, Option<ItemId>)> = Vec::new();
        let mut chain_index = 0usize;
        let mut row_index = 0usize;
        while row_index < rows.len() {
            let is_activity = matches!(rows[row_index].1, DetailRow::Activity { .. });
            if is_activity {
                let start = row_index;
                while row_index < rows.len()
                    && matches!(rows[row_index].1, DetailRow::Activity { .. })
                {
                    row_index += 1;
                }
                let chain_rows = &rows[start..row_index];
                let (live, failed) =
                    activity_chain_disclosure_state(chain_rows, owns_live_header, turn_lifecycle);
                items = items.child(self.render_trace_chain(
                    selector,
                    chain_index,
                    chain_rows,
                    live,
                    failed,
                    status_motion,
                    entity,
                    theme,
                    anchors,
                    items_mounted,
                    window,
                    cx,
                ));
                // The chain's own prepaint listener resolves its row bounds;
                // at this level the chain is opaque to row targets so a
                // collapsed chain cannot resolve one against its header.
                child_identities.push((None, None));
                chain_index += 1;
            } else {
                items = items.child(self.render_detail_row(
                    rows[row_index].1,
                    rows[row_index].0,
                    selector,
                    entity,
                    theme,
                    anchors,
                    items_mounted,
                ));
                let id = rows[row_index].1.scene_id();
                child_identities.push((Some(id.clone()), item_id_for_scene_id(id)));
                row_index += 1;
            }
        }
        if header.is_some() {
            items = items.pt(theme.spacing.steps(2.0));
        }
        // Identities mirror painted children in order, so executed scroll
        // targets resolve against measured child bounds one-to-one.
        let surface = entity.downgrade();
        items = items.on_children_prepainted(move |children_bounds, window, app| {
            let _ = surface.update(app, |surface, _| {
                surface.apply_executed_scroll_targets(&child_identities, &children_bounds, window);
            });
        });

        // Plain section on the transcript surface: reference work sessions
        // and activity rows render without card chrome.
        let section = div().w_full().min_w_0().flex().flex_col();
        let mut section = anchors.attach(section, group_id.as_ref(), None);
        section = section.debug_selector({
            let selector = selector.to_owned();
            move || selector.clone()
        });
        // One header element for the group's whole life: the reference
        // keeps a single session header carrying both the entrance and the
        // divider, so plain and disclosable renders share this construction.
        // Splitting the headers would remount the row the instant the first
        // detail arrived.
        let header_row = Self::work_group_header_row(
            header,
            transition,
            open,
            controlled,
            selector,
            status_motion,
            mounted_working,
            theme,
        );
        // The disclosure root wraps header plus panel in every state so the
        // header ancestry never changes; registering disclosure later must
        // not remount the row. `Collapsible` cannot own this slot: it unmounts
        // closed content and holds no clock, which is exactly the flash the
        // reference `t-acc-panel` lip removes.
        let disclosure_selector = format!("{selector}-disclosure");
        let base_trigger = div().min_w_0().child(header_row).debug_selector({
            let trigger_selector = format!("{disclosure_selector}-trigger");
            move || trigger_selector.clone()
        });
        let trigger: AnyElement =
            if let Some(action_id) = controlled.then(|| group_id.clone()).flatten() {
                // The toggle callback exists only for controlled groups; the
                // wrapper itself stays mounted in every case. Uncontrolled groups
                // have no scene identity to address, and their trigger is inert.
                let surface = entity.downgrade();
                let requested_open = !open;
                base_trigger
                    .id(ElementId::Name(SharedString::from(format!(
                        "{disclosure_selector}-trigger"
                    ))))
                    .track_focus(&self.disclosure_focus)
                    .on_click(move |event, _, app| {
                        if !event.standard_click() {
                            return;
                        }
                        let action = ConversationSurfaceAction::DisclosureToggleRequested {
                            id: action_id.clone(),
                            requested_open,
                        };
                        let _ = surface.update(app, |surface, cx| {
                            if surface.enqueue_action(action) {
                                cx.notify();
                            }
                        });
                    })
                    .into_any_element()
            } else {
                base_trigger.into_any_element()
            };
        let disclosure = div()
            .flex()
            .flex_col()
            .min_w_0()
            .child(trigger)
            .debug_selector({
                let root_selector = disclosure_selector.clone();
                move || root_selector.clone()
            });

        // The panel is the reference lip as a real clipped height. Its content
        // stays mounted only while it can be seen or measured (open, or a
        // collapse still in flight), so settled history keeps the reference's
        // unmounted-details policy.
        let panel = div()
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .flex_shrink_0()
            .debug_selector({
                let panel_selector = format!("{selector}-disclosure-panel");
                move || panel_selector.clone()
            });

        let disclosure = disclosure_flight_panel(
            disclosure,
            panel,
            items.into_any_element(),
            selector,
            open,
            controlled,
            status_motion,
            window,
            cx,
        );

        section.child(disclosure).into_any_element()
    }

    /// Renders one contiguous activity chain with the reference trace header,
    /// left rail, and rows.
    ///
    /// The header carries the category icon, the counted clause sentence, and
    /// the chevron; the rows indent under a 2 px rail inside the shared
    /// accordion panel. Disclosure is surface-local state keyed by the first
    /// activity's identity, exactly like the reference `open_groups`: an
    /// unset chain re-evaluates its failed/live default every render, and a
    /// user toggle pins the value for the surface's life.
    #[expect(
        clippy::too_many_arguments,
        reason = "one GPUI chain builder threads its selector, rows, state, motion, entity, theme, anchors, and window through the shared accordion"
    )]
    #[expect(
        clippy::too_many_lines,
        reason = "one GPUI builder composes the trace header, rail, and row loop behind the shared disclosure panel"
    )]
    pub(super) fn render_trace_chain(
        &self,
        group_selector: &str,
        chain_index: usize,
        rows: &[(u64, DetailRow<'_>)],
        live: bool,
        failed: bool,
        motion: MotionPolicy,
        entity: &Entity<Self>,
        theme: &ArtisanTheme,
        anchors: &mut ScrollAnchorRegistry<'_>,
        mounted: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selector = format!("{group_selector}-trace-{chain_index}");
        let chain_key = rows
            .first()
            .map(|(_, row)| row.scene_id().as_str().to_owned())
            .unwrap_or_default();
        let open = self
            .trace_groups_open
            .borrow()
            .get(&chain_key)
            .copied()
            .unwrap_or(live || failed);
        let header_color = if open {
            theme.colors.foreground
        } else {
            theme.colors.muted_foreground
        };
        let kinds: Vec<&str> = rows
            .iter()
            .map(|(_, row)| match row {
                DetailRow::Activity { kind, .. } => (*kind).unwrap_or("tool"),
                _ => "tool",
            })
            .collect();
        let clause = activity_chain_clause(kinds.iter().copied());
        let icon = activity_chain_icon(kinds.iter().copied());

        let header = div()
            .id(ElementId::Name(SharedString::from(format!(
                "{selector}-trigger"
            ))))
            .flex()
            .flex_row()
            .items_center()
            .gap(theme.spacing.steps(2.0))
            .w_full()
            .min_w_0()
            .py(theme.spacing.steps(0.5))
            .cursor_pointer()
            .track_focus(&self.disclosure_focus)
            .aria_label("Toggle activity details")
            .debug_selector({
                let trigger_selector = format!("{selector}-trigger");
                move || trigger_selector.clone()
            })
            .on_click({
                let surface = entity.downgrade();
                let chain_key = chain_key.clone();
                let requested_open = !open;
                move |event, _, app| {
                    if !event.standard_click() {
                        return;
                    }
                    let _ = surface.update(app, |surface, cx| {
                        surface
                            .trace_groups_open
                            .borrow_mut()
                            .insert(chain_key.clone(), requested_open);
                        cx.notify();
                    });
                }
            })
            .child(
                div()
                    .flex_shrink_0()
                    .text_color(header_color.to_paint())
                    .child(asset_glyph(icon).size(px(16.0))),
            )
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .truncate()
                    .text_size(px(ProseTypography::BODY_SIZE_PX))
                    .line_height(theme.spacing.steps(6.0))
                    .text_color(header_color.to_paint())
                    .child(clause),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .text_color(header_color.to_paint())
                    .child(
                        asset_glyph(if open {
                            AssetId::TABLER_CHEVRON_DOWN
                        } else {
                            AssetId::TABLER_CHEVRON_RIGHT
                        })
                        .size(px(14.0)),
                    ),
            );

        // The reference `pl-6` rail column: one 16 px absolute rail with a
        // centered 2 px `border/60` line, and the rows flowing beside it.
        let mut rows_column = div()
            .relative()
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(theme.spacing.steps(1.0))
            .pl(theme.spacing.steps(6.0))
            .child(
                div()
                    .absolute()
                    .top(px(0.0))
                    .bottom(px(0.0))
                    .left(px(0.0))
                    .w(theme.spacing.steps(4.0))
                    .child(
                        div()
                            .absolute()
                            .top(px(0.0))
                            .bottom(px(0.0))
                            .left(theme.spacing.steps(1.75))
                            .w(px(2.0))
                            .bg(theme.colors.border.with_alpha(0.6).to_paint())
                            .debug_selector({
                                let rail_selector = format!("{selector}-rail");
                                move || rail_selector.clone()
                            }),
                    ),
            );
        // The absolute rail is the first child, so it takes the leading
        // identity slot and the rows line up one-to-one behind it.
        let mut row_identities: Vec<(Option<SceneId>, Option<ItemId>)> = vec![(None, None)];
        for (ordinal, row) in rows {
            rows_column = rows_column.child(self.render_detail_row(
                *row,
                *ordinal,
                group_selector,
                entity,
                theme,
                anchors,
                mounted,
            ));
            let id = row.scene_id();
            row_identities.push((Some(id.clone()), item_id_for_scene_id(id)));
        }
        let surface = entity.downgrade();
        rows_column = rows_column.on_children_prepainted(move |children_bounds, window, app| {
            let _ = surface.update(app, |surface, _| {
                surface.apply_executed_scroll_targets(&row_identities, &children_bounds, window);
            });
        });

        let disclosure = div()
            .flex()
            .flex_col()
            .min_w_0()
            .child(header)
            .debug_selector({
                let root_selector = format!("{selector}-disclosure");
                move || root_selector.clone()
            });
        let panel = div()
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .flex_shrink_0()
            .debug_selector({
                let panel_selector = format!("{selector}-disclosure-panel");
                move || panel_selector.clone()
            });

        disclosure_flight_panel(
            disclosure,
            panel,
            rows_column.into_any_element(),
            &selector,
            open,
            true,
            motion,
            window,
            cx,
        )
    }

    /// Renders one ordered detail row with its scroll anchor.
    ///
    /// Per-row disclosure stays data-only: visibility follows the group
    /// control, matching the reference grouping, which never shows nested
    /// toggles. Assistant details render full markdown like top-level
    /// replies; compaction and native facts reuse the native card
    /// presentation statically.
    #[expect(
        clippy::too_many_arguments,
        reason = "one detail-row builder needs the row, ordinal, group selector, entity, theme, anchors, and mount state it composes"
    )]
    #[expect(
        clippy::too_many_lines,
        reason = "one GPUI builder renders every detail-row variant behind the shared disclosure and anchor treatment"
    )]
    pub(super) fn render_detail_row(
        &self,
        row: DetailRow<'_>,
        ordinal: u64,
        group_selector: &str,
        entity: &Entity<Self>,
        theme: &ArtisanTheme,
        anchors: &mut ScrollAnchorRegistry<'_>,
        mounted: bool,
    ) -> AnyElement {
        let selector = format!("{group_selector}-detail-{ordinal}");
        let row_id = row.scene_id().clone();
        match &row {
            DetailRow::Compaction { id, summary, .. } => self.render_text_block(
                TextBlockRender {
                    id,
                    disclosure: None,
                    selector,
                    title: "Compaction",
                    body: summary,
                },
                entity,
                theme,
                anchors,
            ),
            DetailRow::NativeFact { id, text, .. } => self.render_text_block(
                TextBlockRender {
                    id,
                    disclosure: None,
                    selector,
                    title: "Native fact",
                    body: text,
                },
                entity,
                theme,
                anchors,
            ),
            row => {
                let content: AnyElement = match &row {
                    DetailRow::Assistant { body, .. } => {
                        let rendered = self.render_budgeted_markdown(
                            body,
                            theme,
                            format!("{selector}-markdown"),
                            MarkdownBodyTone::Foreground,
                        );
                        div()
                            .w_full()
                            .max_w(px(672.0))
                            .child(rendered)
                            .into_any_element()
                    }
                    // The reference activity row: the category label in the
                    // foreground, then the detail — normalized and mono for
                    // terminal commands, raw and truncated otherwise — or the
                    // presentation label when no detail was disclosed. Rows
                    // without a provider kind keep the legacy flat body.
                    DetailRow::Activity {
                        body,
                        kind,
                        detail,
                        lifecycle,
                        ..
                    } => match kind {
                        Some(kind) => {
                            let category = activity_category(kind);
                            let row_text = activity_detail_text(kind, *detail, *lifecycle);
                            let detail_element: AnyElement = if *kind == "terminal_activity" {
                                // The monospace face already says "shell": the
                                // normalized command truncates on one line at
                                // the reference `font-mono text-sm` recipe.
                                div()
                                    .min_w_0()
                                    .flex_1()
                                    .truncate()
                                    .font_family(theme.typography.mono.family)
                                    .text_size(theme.typography.control_text)
                                    .text_color(theme.colors.muted_foreground.to_paint())
                                    .child(row_text)
                                    .into_any_element()
                            } else {
                                div()
                                    .min_w_0()
                                    .flex_1()
                                    .truncate()
                                    .text_color(theme.colors.muted_foreground.to_paint())
                                    .child(row_text)
                                    .into_any_element()
                            };
                            div()
                                .w_full()
                                .min_w_0()
                                .py(theme.spacing.steps(0.5))
                                .text_size(px(ProseTypography::BODY_SIZE_PX))
                                .line_height(theme.spacing.steps(6.0))
                                .font_weight(ProseTypography::BODY_WEIGHT)
                                .letter_spacing(px(ProseTypography::body_tracking_px(
                                    ProseTypography::BODY_SIZE_PX,
                                )))
                                .text_color(theme.colors.muted_foreground.to_paint())
                                .child(
                                    div()
                                        .flex()
                                        .flex_row()
                                        .items_center()
                                        .gap(theme.spacing.steps(2.0))
                                        .min_w_0()
                                        .child(
                                            div()
                                                .flex_shrink_0()
                                                .text_color(theme.colors.foreground.to_paint())
                                                .child(category.label()),
                                        )
                                        .child(detail_element),
                                )
                                .into_any_element()
                        }
                        None => div()
                            .w_full()
                            .min_w_0()
                            .text_size(theme.typography.control_text)
                            .font_weight(ProseTypography::BODY_WEIGHT)
                            .letter_spacing(px(ProseTypography::body_tracking_px(14.0)))
                            .text_color(theme.colors.foreground.to_paint())
                            .child(body.to_string())
                            .into_any_element(),
                    },
                    // Session titles render muted at base size (reference
                    // header tone); counting lives in the group header and
                    // status row.
                    DetailRow::SessionTitle { title, .. } => div()
                        .w_full()
                        .min_w_0()
                        .text_size(px(ProseTypography::BODY_SIZE_PX))
                        .line_height(theme.spacing.steps(6.0))
                        .font_weight(ProseTypography::BODY_WEIGHT)
                        .letter_spacing(px(ProseTypography::BODY_TRACKING_PX))
                        .text_color(theme.colors.muted_foreground.to_paint())
                        .child(title.to_string())
                        .into_any_element(),
                    DetailRow::Compaction { .. } | DetailRow::NativeFact { .. } => {
                        unreachable!("card rows render above")
                    }
                };
                if mounted {
                    let mut element = anchors.attach(
                        div().w_full().min_w_0(),
                        Some(&row_id),
                        item_id_for_scene_id(&row_id).as_ref(),
                    );
                    element = element.debug_selector(move || selector.clone());
                    element.child(content).into_any_element()
                } else {
                    div()
                        .w_full()
                        .min_w_0()
                        .debug_selector(move || selector.clone())
                        .child(content)
                        .into_any_element()
                }
            }
        }
    }

    /// Builds the single session header row shared by plain and disclosable
    /// renders, mirroring `conversation-work-session.svelte` §469:
    /// `relative flex w-full items-center justify-between gap-3 pb-2` with
    /// the label (or disclosure chevron) at the near end, an engine handoff
    /// at the far end, and the 1 px settled divider pinned to the bottom
    /// edge. Controlled groups carry the label tone on the chevron;
    /// uncontrolled text stays static; a headerless controlled group keeps
    /// the chevron-only affordance with an honest accessible name —
    /// disclosure chrome, never invented content.
    #[expect(
        clippy::too_many_arguments,
        reason = "the header row takes the label, transition, disclosure state, motion, and theme it paints as one unit"
    )]
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the header curve samples in f64 and feeds the f32 animation clock; the narrowing is the intended precision"
    )]
    pub(super) fn work_group_header_row(
        label: Option<String>,
        transition: Option<String>,
        open: bool,
        controlled: bool,
        selector: &str,
        motion: MotionPolicy,
        mounted_working: bool,
        theme: &ArtisanTheme,
    ) -> AnyElement {
        let near: AnyElement = match (controlled, label) {
            (true, Some(label)) => div()
                .flex()
                .flex_row()
                .items_center()
                .gap(theme.spacing.steps(1.0))
                .min_w_0()
                .text_color(theme.colors.muted_foreground.to_paint())
                .child(label)
                .child(Self::work_group_chevron(open, theme))
                .into_any_element(),
            (true, None) => div()
                .id(format!("{selector}-work-trigger"))
                .flex()
                .flex_row()
                .items_center()
                .text_color(theme.colors.muted_foreground.to_paint())
                .aria_label("Toggle work details")
                .child(Self::work_group_chevron(open, theme))
                .into_any_element(),
            (false, Some(label)) => div()
                .min_w_0()
                .text_color(theme.colors.muted_foreground.to_paint())
                .child(label)
                .into_any_element(),
            (false, None) => div().into_any_element(),
        };
        let mut header = div()
            .relative()
            .w_full()
            .min_w_0()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap(theme.spacing.steps(3.0))
            .text_size(px(ProseTypography::BODY_SIZE_PX))
            .line_height(theme.spacing.steps(6.0))
            .font_weight(ProseTypography::BODY_WEIGHT)
            .letter_spacing(px(ProseTypography::body_tracking_px(
                ProseTypography::BODY_SIZE_PX,
            )))
            .text_color(theme.colors.muted_foreground.to_paint())
            .pb(theme.spacing.steps(2.0))
            .debug_selector({
                let selector = format!("{selector}-header");
                move || selector.clone()
            })
            .child(near);
        if let Some(handoff) = transition {
            header = header.child(
                div()
                    .flex_shrink_0()
                    .text_color(theme.colors.muted_foreground.to_paint())
                    .child(handoff),
            );
        }
        // The settled divider: the reference `t-settle-underline` base rule,
        // always painted at its exact end geometry. Growing it from the
        // measured label width has no text-measurement primitive here, so no
        // width tween is faked; the rule rides inside the entrance below
        // while one plays.
        header = header.child(
            separator(theme.colors.border.to_paint(), SeparatorAxis::Horizontal)
                .absolute()
                .bottom(px(0.0))
                .left(px(0.0)),
        );
        // Mounted-working entrance only: the reference `status-swap-enter`
        // plays solely for headers mounted while working, and history
        // arriving settled stays static. The frozen flag (retained window
        // state from first mount) means history that later goes live never
        // enters. The selector-keyed chain plays once per group —
        // re-renders never restart it — and settling leaves it at its end
        // state structurally. Hold plus enter mirror the reference 150 ms
        // delay and 150 ms EaseInOut run; opacity, relative 4 px rise (layout
        // neutral, like the reference translate), and 2 px blur all ride the
        // same eased clock. Reduced motion skips the wrapper and rests at
        // the unfiltered state.
        if !mounted_working {
            return header.into_any_element();
        }
        match motion.resolve(MotionRecipe::TextSwap) {
            MotionPlan::Immediate => header.into_any_element(),
            MotionPlan::Animate(animation) => header
                .opacity(0.0)
                .with_animations(
                    ElementId::Name(SharedString::from(format!("{selector}-header-enter"))),
                    vec![
                        Animation::new(MotionDuration::Quick.as_duration()),
                        animation.gpui_clock(),
                    ],
                    |header, index, value| {
                        if index == 0 {
                            header
                                .opacity(0.0)
                                .top(px(-4.0))
                                .filter(vec![Filter::Blur(px(2.0))])
                        } else {
                            let eased = MotionCurve::EaseInOut.sample(f64::from(value)) as f32;
                            header
                                .opacity(eased)
                                .top(px(-4.0 * (1.0 - eased)))
                                .filter(vec![Filter::Blur(px(2.0 * (1.0 - eased)))])
                        }
                    },
                )
                .into_any_element(),
        }
    }

    /// Disclosure chevron for one session header: the reference `size-4`
    /// `ChevronRight` rotated 90 degrees when open. Rotation rides the SVG
    /// render transformation, which [`AssetGlyph`] does not forward yet, so
    /// open swaps in the down glyph at the same 16 px muted geometry — the
    /// accordion lane's own discrete mapping — with no tween claimed. The
    /// exact rotation needs a minimal shared forwarding,
    /// `AssetGlyph::with_transformation(Transformation)`, owned by the
    /// asset-seam lane; with it the chevron becomes one right glyph under
    /// `Transformation::rotate` on the 250 ms `AccordionChevron` clock.
    pub(super) fn work_group_chevron(open: bool, theme: &ArtisanTheme) -> AnyElement {
        asset_glyph(if open {
            AssetId::TABLER_CHEVRON_DOWN
        } else {
            AssetId::TABLER_CHEVRON_RIGHT
        })
        .size(px(16.0))
        .text_color(theme.colors.muted_foreground.to_paint())
        .into_any_element()
    }
}
