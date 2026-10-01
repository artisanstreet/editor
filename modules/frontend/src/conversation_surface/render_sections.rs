//! Single-kind transcript block rendering for a turn row:
//! compaction, change sets, plans, approvals, questions, errors, usage
//! interruptions, model transitions, native facts, steering, footer, and
//! controlled cards; the status row renders from `render_status.rs`.
//!
//! Extracted from `conversation_surface.rs` during the phase-2 module split;
//! rendered by [`TurnRowView`] from its synced inputs (see `turn_row.rs`).

use super::*;

#[path = "render_status.rs"]
mod render_status;

impl TurnRowView {
    pub(super) fn render_compaction(
        &self,
        block: &CompactionBlock,
        selector: String,
        theme: &ArtisanTheme,
        anchors: &mut ScrollAnchorRegistry<'_>,
    ) -> AnyElement {
        self.render_text_block(
            TextBlockRender {
                id: &block.id,
                disclosure: block.disclosure,
                selector,
                title: "Compaction",
                body: &block.summary,
            },
            theme,
            anchors,
        )
    }

    pub(super) fn render_change_set(
        &self,
        block: &ChangeSetBlock,
        selector: String,
        theme: &ArtisanTheme,
        anchors: &mut ScrollAnchorRegistry<'_>,
    ) -> AnyElement {
        let style = CardStyle::resolve(*theme);
        let header = card_heading(format!("Changed files ({})", block.files.len()), theme);
        let mut rows = div()
            .w_full()
            .flex()
            .flex_col()
            .gap(theme.spacing.steps(2.0));
        for (index, file) in block.files.iter().enumerate() {
            rows = rows.child(changed_file_row(&block.id, index, file, theme));
            if index + 1 < block.files.len() {
                rows = rows.child(separator(
                    theme.colors.border.to_paint(),
                    SeparatorAxis::Horizontal,
                ));
            }
        }
        self.render_controlled_card(
            ControlledCardOptions {
                id: block.id.clone(),
                item_id: item_id_for_scene_id(&block.id),
                disclosure: block.disclosure,
                selector,
                style,
            },
            compact_card_content(style).child(header),
            compact_card_content(style).child(rows),
            anchors,
        )
    }

    pub(super) fn render_plan(
        &self,
        block: &PlanBlock,
        selector: String,
        theme: &ArtisanTheme,
        anchors: &mut ScrollAnchorRegistry<'_>,
    ) -> AnyElement {
        let style = CardStyle::resolve(*theme);
        let mut entries = div()
            .w_full()
            .flex()
            .flex_col()
            .gap(theme.spacing.steps(2.0));
        for entry in &block.entries {
            entries = entries.child(
                div()
                    .w_full()
                    .flex()
                    .flex_row()
                    .items_start()
                    .gap(theme.spacing.steps(2.0))
                    .child(div().flex_shrink_0().child("•"))
                    .child(body_text(entry, theme)),
            );
        }
        let content = div()
            .w_full()
            .flex()
            .flex_col()
            .gap(theme.spacing.steps(2.0))
            .child(body_text(&block.title, theme))
            .child(entries);
        self.render_controlled_card(
            ControlledCardOptions {
                id: block.id.clone(),
                item_id: item_id_for_scene_id(&block.id),
                disclosure: block.disclosure,
                selector,
                style,
            },
            compact_card_content(style).child(card_heading("Plan", theme)),
            compact_card_content(style).child(content),
            anchors,
        )
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one GPUI builder renders the approval body, both decision controls, and the shared card treatment in visual order"
    )]
    pub(super) fn render_approval(
        &self,
        block: &crate::conversation_scene::ApprovalBlock,
        selector: String,
        theme: &ArtisanTheme,
        anchors: &mut ScrollAnchorRegistry<'_>,
    ) -> AnyElement {
        let style = CardStyle::resolve(*theme);
        let key = block.id.as_str().to_owned();
        // The block carries the engine's approval identity and its owning
        // run from durable provenance; the host supplies the thread.
        let target = block
            .run_id
            .clone()
            .map(|run_id| (run_id, block.approval_id.clone()));
        let gate = self.approval_state(&key);
        let in_flight = gate.is_some_and(|gate| gate.in_flight);
        let pending = gate.and_then(|gate| gate.pending_decision);
        let failure = gate.and_then(|gate| gate.failure.as_deref());
        let ready = target.is_some() && self.answer_ready;
        let disabled = !ready || in_flight;
        let approve_label = if pending == Some(true) {
            pending_approval_label(&PresentationApprovalKind::Action, false)
        } else {
            "Approve"
        };
        let deny_label = if pending == Some(false) {
            APPROVAL_DENYING_LABEL
        } else {
            APPROVAL_DENY_LABEL
        };

        let surface = self.surface.clone();
        let approve_key = key.clone();
        let approve_target = target.clone();
        let approve_button = Button::new(
            SharedString::from(format!("{selector}-approve")),
            self.answer_focus.clone(),
            *theme,
            MotionPolicy::Reduced,
            ButtonVariant::Default,
            ButtonSize::Small,
            ButtonContent::text(approve_label),
        )
        .map(|button| {
            button
                .focus_visibility(FocusVisibility::Visible)
                .debug_selector(format!("{selector}-{APPROVAL_CONFIRM_SELECTOR_SUFFIX}"))
                .disabled(disabled)
                .on_activate(move |_, _, app| {
                    if let Some((run_id, approval_id)) = &approve_target {
                        let _ = surface.update(app, |surface, cx| {
                            surface.submit_approval_gesture(
                                &approve_key,
                                run_id,
                                approval_id,
                                true,
                                cx,
                            );
                        });
                    }
                })
        });

        let surface = self.surface.clone();
        let deny_key = key.clone();
        let deny_button = Button::new(
            SharedString::from(format!("{selector}-deny")),
            self.answer_focus.clone(),
            *theme,
            MotionPolicy::Reduced,
            ButtonVariant::Outline,
            ButtonSize::Small,
            ButtonContent::text(deny_label),
        )
        .map(|button| {
            button
                .focus_visibility(FocusVisibility::Visible)
                .debug_selector(format!("{selector}-{APPROVAL_DENY_SELECTOR_SUFFIX}"))
                .disabled(disabled)
                .on_activate(move |_, _, app| {
                    if let Some((run_id, approval_id)) = &target {
                        let _ = surface.update(app, |surface, cx| {
                            surface.submit_approval_gesture(
                                &deny_key,
                                run_id,
                                approval_id,
                                false,
                                cx,
                            );
                        });
                    }
                })
        });

        let mut actions = div()
            .w_full()
            .flex()
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap(theme.spacing.steps(2.0));
        if let Ok(button) = approve_button {
            actions = actions.child(button);
        }
        if let Ok(button) = deny_button {
            actions = actions.child(button);
        }

        let mut details = div()
            .w_full()
            .flex()
            .flex_col()
            .gap(theme.spacing.steps(2.0))
            .child(body_text(&block.prompt, theme))
            .child(actions);
        if let Some(failure) = failure {
            let failure_selector = format!("{selector}-{APPROVAL_FAILURE_SELECTOR_SUFFIX}");
            details = details.child(crate::dismissible_notice::DismissibleNotice::new(
                format!("{failure_selector}-{failure}"),
                body_text(failure, theme).debug_selector(move || failure_selector.clone()),
                *theme,
            ));
        }
        self.render_controlled_card(
            ControlledCardOptions {
                id: block.id.clone(),
                item_id: item_id_for_scene_id(&block.id),
                disclosure: block.disclosure,
                selector,
                style,
            },
            compact_card_content(style).child(card_heading("Approval requested", theme)),
            compact_card_content(style).child(details),
            anchors,
        )
    }

    /// Renders one agent question as the transcript's record of it.
    ///
    /// Questions are answered from the composer, which extends upward with
    /// every open questionnaire; the transcript only records what was asked
    /// and, once given, the answer.
    pub(super) fn render_question(
        &self,
        block: &QuestionBlock,
        selector: String,
        theme: &ArtisanTheme,
        anchors: &mut ScrollAnchorRegistry<'_>,
    ) -> AnyElement {
        let style = CardStyle::resolve(*theme);
        let (heading, status) = match block.answer.as_deref() {
            None => ("Question", "Answer it above the message box.".to_owned()),
            Some("") => ("Skipped question", "You skipped this question.".to_owned()),
            Some(answer) => ("Answered question", format!("You answered: {answer}")),
        };
        let details = div()
            .w_full()
            .flex()
            .flex_col()
            .gap(theme.spacing.steps(2.0))
            .child(body_text(&block.prompt, theme))
            .child(
                body_text(&status, theme)
                    .text_color(theme.colors.muted_foreground.to_paint())
                    .debug_selector(|| format!("{selector}-status")),
            );
        self.render_controlled_card(
            ControlledCardOptions {
                id: block.id.clone(),
                item_id: item_id_for_scene_id(&block.id),
                disclosure: block.disclosure,
                selector,
                style,
            },
            compact_card_content(style).child(card_heading(heading, theme)),
            compact_card_content(style).child(details),
            anchors,
        )
    }

    /// Renders one error as the reference's single destructive card.
    ///
    /// The alert carries its own face, `role="alert"` semantics, icon,
    /// title, and description: no outer wrapper card and no second heading.
    /// The recipe specializes the existing destructive style to the reference
    /// card (`rounded-xl`, destructive border/tint, tight paddings/gaps,
    /// muted description) without touching the shared global. Error facts
    /// stay mounted in every disclosure state (the reference shows no
    /// disclosure control for errors), while the stable anchor and debug
    /// selector preserve scroll and test addressing. No copy action: the
    /// scene block carries only the message.
    pub(super) fn render_error(
        block: &ErrorBlock,
        selector: String,
        theme: &ArtisanTheme,
        anchors: &mut ScrollAnchorRegistry<'_>,
    ) -> AnyElement {
        let mut style = AlertStyle::resolve(*theme, AlertVariant::Destructive);
        style.corner_radius = RadiusTokens::value(RadiusStep::Xl);
        style.horizontal_padding = theme.spacing.steps(3.5);
        style.vertical_padding = theme.spacing.steps(3.0);
        style.content_gap = theme.spacing.steps(1.5);
        style.icon_gap = theme.spacing.steps(2.0);
        style.border_color = theme.colors.destructive.with_alpha(0.25).to_paint();
        style.background = theme.colors.destructive.with_alpha(0.05).to_paint();
        style.description_foreground = theme.colors.muted_foreground.to_paint();
        let alert = Alert::new(style)
            .icon(AssetId::TABLER_CIRCLE_X)
            .title("Error")
            .description(block.message.clone())
            .debug_selector(format!("{selector}-alert"));
        let mut element = anchors.attach(
            div().w_full().min_w_0(),
            Some(&block.id),
            item_id_for_scene_id(&block.id).as_ref(),
        );
        element = element.debug_selector(move || selector.clone());
        element
            .child(crate::dismissible_notice::DismissibleNotice::new(
                format!("error-{:?}-{}", block.id, block.message),
                alert,
                *theme,
            ))
            .into_any_element()
    }

    pub(super) fn render_usage_interruption(
        &self,
        block: &UsageInterruptionBlock,
        selector: String,
        theme: &ArtisanTheme,
        anchors: &mut ScrollAnchorRegistry<'_>,
    ) -> AnyElement {
        let style = CardStyle::resolve(*theme);
        let alert = Alert::from_theme(*theme, AlertVariant::Default)
            .title("Usage interruption")
            .description(block.detail.clone())
            .debug_selector(format!("{selector}-alert"));
        self.render_controlled_card(
            ControlledCardOptions {
                id: block.id.clone(),
                item_id: item_id_for_scene_id(&block.id),
                disclosure: block.disclosure,
                selector,
                style,
            },
            compact_card_content(style).child(card_heading("Usage interruption", theme)),
            alert,
            anchors,
        )
    }

    pub(super) fn render_model_transition(
        &self,
        block: &ModelTransitionBlock,
        selector: String,
        theme: &ArtisanTheme,
        anchors: &mut ScrollAnchorRegistry<'_>,
    ) -> AnyElement {
        let text = format!("{} → {}", block.from_model, block.to_model);
        self.render_text_block(
            TextBlockRender {
                id: &block.id,
                disclosure: block.disclosure,
                selector,
                title: "Model transition",
                body: &text,
            },
            theme,
            anchors,
        )
    }

    pub(super) fn render_native_fact(
        &self,
        block: &NativeFactBlock,
        selector: String,
        theme: &ArtisanTheme,
        anchors: &mut ScrollAnchorRegistry<'_>,
    ) -> AnyElement {
        self.render_text_block(
            TextBlockRender {
                id: &block.id,
                disclosure: block.disclosure,
                selector,
                title: "Native fact",
                body: &block.text,
            },
            theme,
            anchors,
        )
    }

    pub(super) fn render_steering(
        block: &SteeringBlock,
        selector: String,
        theme: &ArtisanTheme,
        anchors: &mut ScrollAnchorRegistry<'_>,
    ) -> AnyElement {
        let user_message_anchor = anchors.anchor_for_item(&block.anchor);
        let label = div()
            .w_full()
            .min_w_0()
            .text_size(theme.typography.label_text)
            .text_color(theme.colors.muted_foreground.to_paint())
            .whitespace_normal()
            .child(block.label.clone());
        let label = anchors.attach(label, Some(&block.id), None);
        let label = label.debug_selector(move || selector.clone());
        if let Some((anchor, painted)) = user_message_anchor {
            anchors.register_item_alias(block.anchor.clone(), anchor, painted);
        }
        label.into_any_element()
    }

    /// Renders the hover/focus-only settled footer for one turn.
    ///
    /// A flow row inside the message column, four pixels below the response,
    /// invisible until turn
    /// hover or copy-button focus, carrying the ghost copy control and the
    /// relative age. Only an eligible settlement paints; unsettled turns keep
    /// no placeholder and no gap slot. Hover and focus emit
    /// [`ConversationSurfaceAction::TurnFooterRevealed`] so the host can take
    /// its one clock sample; the copy gesture emits
    /// [`ConversationSurfaceAction::TurnFooterCopyRequested`] with the exact
    /// settlement bytes.
    #[expect(
        clippy::too_many_lines,
        reason = "one GPUI builder composes the footer reveal, copy control, and relative-age mirror in visual order"
    )]
    pub(super) fn render_footer(
        &self,
        turn_id: &TurnId,
        block: &TurnFooterBlock,
        selector: String,
        theme: &ArtisanTheme,
        window: &mut Window,
        motion: MotionPolicy,
    ) -> Option<AnyElement> {
        let settlement = footer_settlement(block)?;
        let key = footer_key(turn_id);
        let mirror = self.footer_mirror.as_ref();
        let relative_age = mirror.map_or("", |staged| staged.relative_age.as_str());
        let copy_message = mirror.map_or("", |staged| staged.copy_message.as_str());
        let handle = self
            .footer_focus
            .clone()
            .unwrap_or_else(|| self.answer_focus.clone());
        let focused = handle.is_focused(window);

        let surface = self.surface.clone();
        let copy_turn = turn_id.clone();
        // A shared handle: the copy payload is cloned per render, the bytes
        // only when the reader actually copies.
        let copy_text = settlement.response_text_shared();
        let copy_label = AccessibleLabel::new(COPY_RESPONSE_LABEL).ok()?;
        let copied_elapsed = mirror
            .and_then(|mirror| mirror.copied_at)
            .map(|at| at.elapsed());
        let copied = copied_elapsed.map_or(0.0, |elapsed| {
            if elapsed < COPY_FEEDBACK_WINDOW {
                window.request_animation_frame();
            }
            copy_feedback_progress(elapsed, motion)
        });
        let copy_icon = copy_feedback_icon(copied);
        let copy_button = Button::new(
            SharedString::from(format!("{selector}-copy")),
            handle,
            *theme,
            MotionPolicy::Reduced,
            ButtonVariant::Ghost,
            ButtonSize::IconSmall,
            ButtonContent::icon_only(AssetId::TABLER_COPY, copy_label),
        )
        .map(|button| {
            button
                .icon_slot(copy_icon)
                .bare()
                .focus_visibility(FocusVisibility::Visible)
                .tint(
                    theme.colors.muted_foreground.to_paint(),
                    theme.colors.foreground.to_paint(),
                )
                .debug_selector(format!("{selector}-{FOOTER_COPY_SELECTOR_SUFFIX}"))
                .on_activate(move |_, _, app| {
                    let _ = surface.update(app, |surface, cx| {
                        if surface.enqueue_action(
                            ConversationSurfaceAction::TurnFooterCopyRequested {
                                turn: copy_turn.clone(),
                                text: copy_text.to_string(),
                            },
                        ) {
                            cx.notify();
                        }
                    });
                })
        });

        let hover_surface = self.surface.clone();
        let reveal_turn = turn_id.clone();
        let reveal_key = key.clone();
        let time_selector = format!("{selector}-time-{}", settlement.settled_at_ms());
        let mut footer = div()
            .id(format!("{selector}-footer"))
            .mt(-theme.spacing.steps(5.0))
            .flex()
            .flex_row()
            .items_center()
            .gap(theme.spacing.steps(2.5))
            .text_size(theme.typography.control_text)
            .font_weight(ProseTypography::BODY_WEIGHT)
            .letter_spacing(px(ProseTypography::body_tracking_px(14.0)))
            .text_color(theme.colors.muted_foreground.to_paint())
            .opacity(if focused || self.footer_revealed {
                1.0
            } else {
                0.0
            })
            .group_hover(TURN_GROUP, |hover| hover.opacity(1.0))
            .aria_label(TURN_ACTIONS_LABEL)
            .debug_selector(|| selector.clone())
            .on_hover(move |hovered, _, app| {
                let _ = hover_surface.update(app, |surface, cx| {
                    let mut changed = footer_hover_transition(
                        &mut surface.footer_revealed,
                        &reveal_key,
                        *hovered,
                    );
                    if *hovered
                        && surface.enqueue_action(ConversationSurfaceAction::TurnFooterRevealed {
                            turn: reveal_turn.clone(),
                        })
                    {
                        changed = true;
                    }
                    if changed {
                        cx.notify();
                    }
                });
            });
        if let Ok(copy_button) = copy_button {
            footer = footer.child(copy_button);
        }
        if !copy_message.is_empty() {
            footer = footer.child(
                div()
                    .text_color(theme.colors.destructive.to_paint())
                    .debug_selector(|| format!("{selector}-copy-message"))
                    .child(copy_message.to_owned()),
            );
        }
        if !relative_age.is_empty() {
            let mut throughput = div()
                .id(format!("{time_selector}-throughput"))
                .debug_selector(move || time_selector.clone())
                .flex()
                .items_center()
                .gap(theme.spacing.steps(1.0))
                .child(relative_age.trim_end_matches(" ago").to_owned());
            if let Some(speed) = mirror.and_then(|mirror| mirror.token_speed.as_deref()) {
                throughput = throughput
                    .child(
                        div()
                            .text_color(transcript_separator_color(theme))
                            .child("∷"),
                    )
                    .child(speed.to_owned());
            }
            footer = footer.child(throughput);
        }
        Some(footer.into_any_element())
    }

    pub(super) fn render_controlled_card(
        &self,
        options: ControlledCardOptions,
        trigger: impl IntoElement,
        content: impl IntoElement,
        anchors: &mut ScrollAnchorRegistry<'_>,
    ) -> AnyElement {
        let ControlledCardOptions {
            id,
            item_id,
            disclosure,
            selector,
            style,
        } = options;
        let disabled = disclosure.is_none();
        let open = !matches!(disclosure, Some(SceneDisclosure::Closed));
        let disclosure_selector = format!("{selector}-disclosure");
        let mut collapsible = Collapsible::new(
            SharedString::from(disclosure_selector.clone()),
            self.disclosure_focus.clone(),
            open,
            trigger,
            content,
        )
        .disabled(disabled)
        .force_mount(disabled)
        .debug_selector(disclosure_selector);

        if !disabled {
            let surface = self.surface.clone();
            let action_id = id.clone();
            collapsible = collapsible.on_change(move |requested_open, _, _, app| {
                let action = ConversationSurfaceAction::DisclosureToggleRequested {
                    id: action_id.clone(),
                    requested_open,
                };
                let _ = surface.update(app, |surface, cx| {
                    if surface.enqueue_action(action) {
                        cx.notify();
                    }
                });
            });
        }

        let card = compact_card(style).w_full();
        let card = anchors.attach(card, Some(&id), item_id.as_ref());
        let card = card.debug_selector(move || selector.clone());
        card.child(collapsible).into_any_element()
    }
}
