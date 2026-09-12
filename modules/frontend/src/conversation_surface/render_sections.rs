//! Single-kind transcript block rendering for [`ConversationSurface`]:
//! compaction, change sets, plans, approvals, questions, errors, usage
//! interruptions, model transitions, native facts, steering, status, footer,
//! and controlled cards.
//!
//! Extracted verbatim from `conversation_surface.rs` during the phase-2 module
//! split; visibility was widened to `pub(super)` for parent-owned methods.

use super::*;
use gpui::{AppContext as _, prelude::FluentBuilder as _};

impl ConversationSurface {
    pub(super) fn render_compaction(
        &self,
        block: &CompactionBlock,
        selector: String,
        entity: &Entity<Self>,
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
            entity,
            theme,
            anchors,
        )
    }

    pub(super) fn render_change_set(
        &self,
        block: &ChangeSetBlock,
        selector: String,
        entity: &Entity<Self>,
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
            entity,
            anchors,
        )
    }

    pub(super) fn render_plan(
        &self,
        block: &PlanBlock,
        selector: String,
        entity: &Entity<Self>,
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
            entity,
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
        entity: &Entity<Self>,
        theme: &ArtisanTheme,
        anchors: &mut ScrollAnchorRegistry<'_>,
    ) -> AnyElement {
        let style = CardStyle::resolve(*theme);
        let key = block.id.as_str().to_owned();
        let approval_id = ObservationId::parse(block.id.as_str()).ok();
        let gate = self.approval_gates.get(&key);
        let in_flight = gate.is_some_and(ApprovalAnswerGate::is_in_flight);
        let pending = gate.and_then(ApprovalAnswerGate::pending_decision);
        let failure = gate.and_then(ApprovalAnswerGate::failure_message);
        let ready =
            approval_id.is_some() && self.answer_thread.is_some() && self.answer_run.is_some();
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

        let surface = entity.downgrade();
        let approve_key = key.clone();
        let approve_id = approval_id.clone();
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
                    if let Some(approval_id) = approve_id.clone() {
                        let _ = surface.update(app, |surface, cx| {
                            surface.submit_approval_gesture(&approve_key, &approval_id, true, cx);
                        });
                    }
                })
        });

        let surface = entity.downgrade();
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
                    if let Some(approval_id) = approval_id.clone() {
                        let _ = surface.update(app, |surface, cx| {
                            surface.submit_approval_gesture(&deny_key, &approval_id, false, cx);
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
            details = details
                .child(body_text(failure, theme).debug_selector(move || failure_selector.clone()));
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
            entity,
            anchors,
        )
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one GPUI builder renders the question body, option rows, and answer controls behind the shared card treatment"
    )]
    pub(super) fn render_question(
        &self,
        block: &QuestionBlock,
        selector: String,
        entity: &Entity<Self>,
        theme: &ArtisanTheme,
        anchors: &mut ScrollAnchorRegistry<'_>,
    ) -> AnyElement {
        let style = CardStyle::resolve(*theme);
        let key = block.id.as_str().to_owned();
        let choices = self.question_choices.get(&key);
        let gate = self.question_gates.get(&key);
        let in_flight = gate.is_some_and(QuestionAnswerGate::is_in_flight);
        let failure = gate.and_then(QuestionAnswerGate::failure_message);
        let ready = ObservationId::parse(block.id.as_str()).is_ok()
            && self.answer_thread.is_some()
            && self.answer_run.is_some();

        let mut details = div()
            .w_full()
            .flex()
            .flex_col()
            .gap(theme.spacing.steps(2.0))
            .child(body_text(&block.prompt, theme));

        if let Some(cache) = choices.filter(|cache| cache.is_choice()) {
            let no_selection: &[String] = &[];
            let selected: &[String] = gate.map_or(no_selection, QuestionAnswerGate::selected);
            let mut options = div()
                .w_full()
                .flex()
                .flex_col()
                .gap(theme.spacing.steps(2.0));
            for (index, (label, description)) in cache.options.iter().enumerate() {
                let chosen = selected.iter().any(|known| known == label);
                let surface = entity.downgrade();
                let option_key = key.clone();
                let option_label = label.clone();
                let option_selector =
                    format!("{selector}-{QUESTION_OPTION_SELECTOR_SUFFIX}-{index}");
                let option_button = Button::new(
                    SharedString::from(option_selector.clone()),
                    self.answer_focus.clone(),
                    *theme,
                    MotionPolicy::Reduced,
                    if chosen {
                        ButtonVariant::Default
                    } else {
                        ButtonVariant::Outline
                    },
                    ButtonSize::Small,
                    ButtonContent::text(label.clone()),
                )
                .map(|button| {
                    button
                        .focus_visibility(FocusVisibility::Visible)
                        .debug_selector(option_selector)
                        .disabled(!ready || in_flight)
                        .on_activate(move |_, _, app| {
                            let _ = surface.update(app, |surface, cx| {
                                surface.submit_question_option_gesture(
                                    &option_key,
                                    option_label.clone(),
                                    cx,
                                );
                            });
                        })
                });
                let mut option_row = div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .gap(theme.spacing.steps(1.0));
                if let Ok(option_button) = option_button {
                    option_row = option_row.child(option_button);
                }
                if let Some(description) = description {
                    option_row = option_row.child(body_text(description, theme));
                }
                options = options.child(option_row);
            }
            details = details.child(options);
            if cache.multi_select {
                let surface = entity.downgrade();
                let answer_key = key.clone();
                let answer_button = Button::new(
                    SharedString::from(format!("{selector}-answer")),
                    self.answer_focus.clone(),
                    *theme,
                    MotionPolicy::Reduced,
                    ButtonVariant::Default,
                    ButtonSize::Small,
                    ButtonContent::text(QUESTION_ANSWER_LABEL),
                )
                .map(|button| {
                    button
                        .focus_visibility(FocusVisibility::Visible)
                        .debug_selector(format!("{selector}-{QUESTION_ANSWER_SELECTOR_SUFFIX}"))
                        .disabled(!ready || in_flight || selected.is_empty())
                        .on_activate(move |_, _, app| {
                            let _ = surface.update(app, |surface, cx| {
                                surface.submit_question_gesture(&answer_key, cx);
                            });
                        })
                });
                if let Ok(answer_button) = answer_button {
                    details = details.child(answer_button);
                }
            }
        } else {
            let draft = gate.map_or("", QuestionAnswerGate::draft);
            let draft_text = if draft.trim().is_empty() {
                QUESTION_INPUT_PLACEHOLDER.to_owned()
            } else {
                draft.to_owned()
            };
            let input_selector = format!("{selector}-input");
            let row_focus = self
                .question_focus
                .get(&key)
                .cloned()
                .unwrap_or_else(|| self.answer_focus.clone());
            let surface = entity.downgrade();
            let key_id = key.clone();
            let transcript = self.transcript_focus.clone();
            let input = div()
                .track_focus(&row_focus)
                .tab_index(0)
                .debug_selector(move || input_selector.clone())
                .child(body_text(&draft_text, theme))
                .on_key_down(move |event, window, app| {
                    let key = event.keystroke.key.as_str().to_owned();
                    let transcript = transcript.clone();
                    let _ = surface.update(app, |surface, cx| {
                        if surface.handle_question_key(
                            &key_id,
                            &key,
                            &event.keystroke.modifiers,
                            cx,
                        ) == QuestionKeyOutcome::FocusTranscript
                        {
                            window.focus(&transcript, cx);
                        }
                    });
                });
            details = details.child(input);
            let surface = entity.downgrade();
            let answer_key = key.clone();
            let answer_button = Button::new(
                SharedString::from(format!("{selector}-answer")),
                self.answer_focus.clone(),
                *theme,
                MotionPolicy::Reduced,
                ButtonVariant::Default,
                ButtonSize::Small,
                ButtonContent::text(QUESTION_ANSWER_LABEL),
            )
            .map(|button| {
                button
                    .focus_visibility(FocusVisibility::Visible)
                    .debug_selector(format!("{selector}-{QUESTION_ANSWER_SELECTOR_SUFFIX}"))
                    .disabled(!ready || in_flight || draft.trim().is_empty())
                    .on_activate(move |_, _, app| {
                        let _ = surface.update(app, |surface, cx| {
                            surface.submit_question_gesture(&answer_key, cx);
                        });
                    })
            });
            if let Ok(answer_button) = answer_button {
                details = details.child(answer_button);
            }
        }
        if let Some(failure) = failure {
            let failure_selector = format!("{selector}-{QUESTION_FAILURE_SELECTOR_SUFFIX}");
            details = details
                .child(body_text(failure, theme).debug_selector(move || failure_selector.clone()));
        }
        self.render_controlled_card(
            ControlledCardOptions {
                id: block.id.clone(),
                item_id: item_id_for_scene_id(&block.id),
                disclosure: block.disclosure,
                selector,
                style,
            },
            compact_card_content(style).child(card_heading("Question", theme)),
            compact_card_content(style).child(details),
            entity,
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
        _entity: &Entity<Self>,
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
        element.child(alert).into_any_element()
    }

    pub(super) fn render_usage_interruption(
        &self,
        block: &UsageInterruptionBlock,
        selector: String,
        entity: &Entity<Self>,
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
            entity,
            anchors,
        )
    }

    pub(super) fn render_model_transition(
        &self,
        block: &ModelTransitionBlock,
        selector: String,
        entity: &Entity<Self>,
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
            entity,
            theme,
            anchors,
        )
    }

    pub(super) fn render_native_fact(
        &self,
        block: &NativeFactBlock,
        selector: String,
        entity: &Entity<Self>,
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
            entity,
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

    pub(super) fn render_status(
        &self,
        turn_id: &TurnId,
        block: &crate::conversation_scene::TurnStatusBlock,
        selector: String,
        theme: &ArtisanTheme,
        status_motion: MotionPolicy,
    ) -> Option<AnyElement> {
        // The terminal duration prefers the work-group header when the turn
        // carries the group; the reference settles to the header alone.
        let turn_scene = self.scene.turn_scene(turn_id);
        let turn_has_work_group = turn_scene.is_some_and(|turn| {
            turn.blocks()
                .iter()
                .any(|block| matches!(block, TurnBlock::WorkGroup(_)))
        });
        // The thinking line is the scene summary reduced to one line when
        // one rides the block; settled rows never carry it (builder
        // guarantee). An unfinished phase reduces to nothing and falls back
        // to the narration, exactly like the reference. Otherwise the
        // narration supplies the verb, with the engine-named wait for a
        // known provider.
        let summary: Option<String> = status_summary_copy(block.reasoning_summary.as_deref());
        let has_summary = summary.is_some();
        let copy = turn_status_copy_text(
            block.narration,
            block.active_started_at_ms,
            self.active_now_ms,
            block.reasoning_summary.as_deref(),
            block.engine_label.as_deref(),
        )?;
        // A live line identical to the owning group header paints once, in
        // the header; a distinct narration (a summary counts) still paints.
        // Render and scroll identities share this exact decision.
        let owner_header = if matches!(
            block.narration,
            TurnNarration::Thinking | TurnNarration::Working
        ) {
            turn_scene.and_then(|turn| turn_owner_header(turn, self.active_now_ms))
        } else {
            None
        };
        if !turn_status_paints(
            turn_has_work_group,
            block.narration,
            Some(copy.as_str()),
            owner_header.as_deref(),
        ) {
            return None;
        }
        // Parity with the work-session status line: base-size muted copy on a
        // half-rem vertical rhythm. The effective motion resolves the live
        // window signal at render time (see `effective_status_motion`); the
        // shimmer animates only for live rows under `Full` and stays
        // immediate for settled history and reduced motion. A summary sweeps
        // with the summary cadence and parses inline marks through the
        // frozen text-runs contract (faces survive the band identically
        // under Full and Reduced); verbs keep the verb cadence.
        let live = matches!(
            block.narration,
            TurnNarration::Thinking
                | TurnNarration::Working
                | TurnNarration::ProviderWait
                | TurnNarration::Compacting
                | TurnNarration::BackgroundWait
        );
        let content: AnyElement = if has_summary {
            // Faces ride the shared shimmer through the frozen text-runs
            // contract: the sweep recolors while family and zero tracking
            // compile at layout, identically under Full and Reduced, with
            // selection retained per stable id.
            let runs = inline_runs(&copy, *theme);
            ShimmerText::new(runs.text, *theme, status_motion)
                .text_runs(
                    format!("{selector}-summary"),
                    runs.highlights,
                    runs.overrides,
                )
                .active(live)
                .delay_seconds(0.0)
                .duration_seconds(2.0)
                .text_color(status_color(theme, block.narration))
                .into_element()
        } else {
            ShimmerText::new(copy, *theme, status_motion)
                .active(live)
                .delay_seconds(1.5)
                .duration_seconds(3.0)
                .text_color(status_color(theme, block.narration))
                .into_element()
        };
        let mut status = div()
            .w_full()
            .min_w_0()
            .flex()
            .flex_row()
            .items_center()
            .my(theme.spacing.steps(2.0))
            .text_size(px(ProseTypography::BODY_SIZE_PX))
            .line_height(px(ProseTypography::BODY_LINE_PX))
            .font_weight(ProseTypography::BODY_WEIGHT)
            .letter_spacing(px(ProseTypography::BODY_TRACKING_PX))
            .text_color(theme.colors.muted_foreground.to_paint())
            .child(content);
        status = status.debug_selector(move || selector.clone());
        Some(status.into_any_element())
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
    #[expect(
        clippy::too_many_arguments,
        reason = "footer receives the shared renderer context and effective motion preference"
    )]
    pub(super) fn render_footer(
        &self,
        turn_id: &TurnId,
        block: &TurnFooterBlock,
        selector: String,
        entity: &Entity<Self>,
        theme: &ArtisanTheme,
        window: &mut Window,
        motion: MotionPolicy,
    ) -> Option<AnyElement> {
        let settlement = footer_settlement(block)?;
        let key = footer_key(turn_id);
        let mirror = self.footer_mirrors.get(&key);
        let relative_age = mirror.map_or("", |staged| staged.relative_age.as_str());
        let copy_message = mirror.map_or("", |staged| staged.copy_message.as_str());
        let handle = self
            .footer_focus
            .get(&key)
            .cloned()
            .unwrap_or_else(|| self.answer_focus.clone());
        let focused = handle.is_focused(window);

        let surface = entity.downgrade();
        let copy_turn = turn_id.clone();
        let copy_text = settlement.response_text().to_owned();
        let copy_label = AccessibleLabel::new(COPY_RESPONSE_LABEL).ok()?;
        let copied_elapsed = mirror
            .and_then(|mirror| mirror.copied_at)
            .map(|at| at.elapsed());
        let copied = copied_elapsed.map_or(0.0, |elapsed| {
            if elapsed < Duration::from_millis(1750) {
                window.request_animation_frame();
            }
            copy_feedback_progress(elapsed, motion)
        });
        let icon_face = |asset, opacity: f32| {
            div()
                .absolute()
                .size(px(16.0))
                .flex()
                .items_center()
                .justify_center()
                .opacity(opacity)
                .blur(px(2.0 * (1.0 - opacity)))
                .child(asset_glyph(asset).size(px(16.0 * (0.25 + 0.75 * opacity))))
        };
        let copy_icon = div()
            .relative()
            .size(px(16.0))
            .child(icon_face(AssetId::TABLER_COPY, 1.0 - copied))
            .child(icon_face(AssetId::TABLER_CHECK, copied));
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
                                text: copy_text.clone(),
                            },
                        ) {
                            cx.notify();
                        }
                    });
                })
        });

        let hover_surface = entity.downgrade();
        let reveal_turn = turn_id.clone();
        let reveal_key = key.clone();
        let message_selector = format!("{selector}-copy-message");
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
            .opacity(
                if focused || self.footer_revealed.as_deref() == Some(key.as_str()) {
                    1.0
                } else {
                    0.0
                },
            )
            .group_hover(TURN_GROUP, |hover| hover.opacity(1.0))
            .aria_label(TURN_ACTIONS_LABEL)
            .debug_selector(move || selector.clone())
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
                    .debug_selector(move || message_selector.clone())
                    .child(copy_message.to_owned()),
            );
        }
        if !relative_age.is_empty() {
            footer = footer.child(
                div()
                    .id(format!("{time_selector}-throughput"))
                    .debug_selector(move || time_selector.clone())
                    .when(
                        mirror.is_some_and(|mirror| mirror.token_speed.is_some()),
                        |element| {
                            let theme = *theme;
                            element
                                .tooltip(move |_, cx| cx.new(|_| FooterSpeedTooltip(theme)).into())
                        },
                    )
                    .child(
                        match mirror.and_then(|mirror| mirror.token_speed.as_deref()) {
                            Some(speed) => {
                                format!("{} • {speed}", relative_age.trim_end_matches(" ago"))
                            }
                            None => relative_age.trim_end_matches(" ago").to_owned(),
                        },
                    ),
            );
        }
        Some(footer.into_any_element())
    }

    pub(super) fn render_controlled_card(
        &self,
        options: ControlledCardOptions,
        trigger: impl IntoElement,
        content: impl IntoElement,
        entity: &Entity<Self>,
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
            let surface = entity.downgrade();
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

struct FooterSpeedTooltip(ArtisanTheme);

impl gpui::Render for FooterSpeedTooltip {
    fn render(&mut self, _: &mut Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
        artisan_ui::tooltip::tooltip_content(
            artisan_ui::tooltip::TooltipStyle::resolve(self.0),
            "Estimated visible-text streaming speed using the o200k reference tokenizer. Excludes startup and tool/reasoning gaps. Network buffering and tokenizer differences can affect this estimate.",
        )
    }
}
