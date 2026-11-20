use super::{MOTION_REBUILD_INTERVAL, RootViews, rail_refresh, read_watch};
use crate::core::Screen;
use crate::shell::host::Shell;
use crate::shell::refresh::Cause;
use aqua::prelude::AquaWindowId;
use aqua::{AppCtx, UiApp};

impl RootViews {
    #[expect(
        clippy::too_many_lines,
        reason = "one frame-completion pass whose steps share the painted rows and timing"
    )]
    pub(super) fn complete_frame(
        &mut self,
        shell: &mut Shell,
        ui: &mut UiApp,
        ctx: &mut AppCtx<'_>,
        window: AquaWindowId,
        #[cfg_attr(
            not(feature = "qualification"),
            expect(
                unused_variables,
                reason = "the frame number only feeds the qualification paint collector"
            )
        )]
        frame: u64,
    ) -> bool {
        let lap_start = crate::shell::stopwatch::Stopwatch::start();
        let early_reveals = {
            let _tm = crate::shell::application::Tm::new("fc.reveals");
            match (&self.transcript.rows, shell.entry.workspace_mut()) {
                (Some(rows), Some(workspace)) => {
                    Some(rows.sync_reveals(ui, &mut workspace.conversation, &shell.core))
                }
                _ => None,
            }
        };
        aqua_core::checkpoint::reach();
        self.dialogs
            .source_dialogs
            .retain(|export| Self::source_export_is_current(shell, export));
        self.bind_row_sources(ui, ctx, window);
        self.sync_resource_jump(ui, ctx, window);
        self.sync_context(shell, ui, ctx, window);
        if let Some(owner) = shell
            .entry
            .workspace()
            .and_then(|workspace| workspace.composer.as_ref())
            && owner.mention_pointer(ui.tree().pointer_position())
        {
            self.transcript.conversation_invalidated.set(true);
            ctx.request_frame(window, aqua::prelude::FrameReason::EXPLICIT);
        }
        if let Some(workspace) = shell.entry.workspace()
            && matches!(shell.core.workspace_screen(), Screen::NewThread { .. })
            && self
                .navigation_views
                .composer_glide
                .awaits(&workspace.new_thread_posting())
        {
            self.transcript.conversation_invalidated.set(true);
            ctx.request_frame(window, aqua::prelude::FrameReason::EXPLICIT);
        }
        let glide = self
            .navigation_views
            .composer_glide
            .sync(ui, shell.core.clock().now());
        if glide.moving {
            ctx.request_frame(window, aqua::prelude::FrameReason::ANIMATION);
        }
        if glide.sky_gone {
            self.transcript.conversation_invalidated.set(true);
        }
        if Self::sync_transcript_reveal(shell, ui) {
            ctx.request_frame(window, aqua::prelude::FrameReason::ANIMATION);
        }
        let (moving, gone) = self
            .navigation_views
            .latest
            .sync(ui, shell.core.clock().now());
        if moving {
            ctx.request_frame(window, aqua::prelude::FrameReason::ANIMATION);
        }
        if gone {
            self.transcript.conversation_invalidated.set(true);
        }
        if self.sync_navigation(ui, ctx, window, shell.core.clock().now()) {
            crate::core::diagnostics::rebuild_stats::record_event("inv_nav_wake");
            self.navigation_views.navigation_invalidated.set(true);
        }
        if let Some(menu) = &mut self.menus.color_menu {
            match menu.advance(ui, ctx.now().as_secs_f64()) {
                Ok(true) => ctx.request_frame(window, aqua::prelude::FrameReason::ANIMATION),
                Ok(false) => {}
                Err(error) => tracing::warn!(%error, "could not advance color copy menu"),
            }
        }
        let reduced = ui.host().motion().reduced_motion() == aqua_ui::motion::ReducedMotion::Reduce;
        let now = shell.core.clock().now();
        aqua_core::checkpoint::reach();
        self.sync_edges(shell, ui, ctx, window);
        let wheel = self.sync_wheel(shell, ui, ctx, window);
        let dispatched = self.dispatch_conversation(shell, ui, ctx, window);
        let sidebar = self.sync_sidebar(shell, ui, ctx, window);
        let connecting = self.connection.animating(now, reduced);
        let loading = self.thread_loading.animating(now, reduced);
        let landing = self.thread_landing.animating(now, reduced);
        let composer_resized = if let Some(height) = self.composer_measurement.take()
            && self
                .composer_height
                .is_none_or(|previous| (previous - height).abs() >= 0.5)
        {
            self.composer_height = Some(height);
            true
        } else {
            false
        };
        let action_changed = self.noted.note(Cause::Wheel, wheel)
            | self.noted.note(Cause::Dispatch, dispatched)
            | self.noted.note(Cause::Sidebar, sidebar)
            | self.noted.note(Cause::ConnectionAnimation, connecting)
            | self.noted.note(Cause::LoadingAnimation, loading)
            | self.noted.note(Cause::LandingAnimation, landing)
            | self.noted.note(Cause::ComposerHeight, composer_resized);
        let Some(rows) = &self.transcript.rows else {
            return action_changed;
        };
        let Some(workspace) = shell.entry.workspace_mut() else {
            return action_changed;
        };
        let Some(reveals) = early_reveals else {
            return action_changed;
        };
        let mut changed = self.noted.note(Cause::Reveals, reveals.frontier_changed);
        let reveal_active = reveals.changed || reveals.animating;
        let retry = self.pending_stall.observe(
            reveals.pending && !reveals.changed && !reveals.animating,
            ui.tree().invalidation_epoch(),
        );
        if changed || reveal_active || retry {
            ctx.request_frame(window, aqua::prelude::FrameReason::ANIMATION);
        }
        aqua_core::checkpoint::reach();
        let tm_events = crate::shell::application::Tm::new("fc.events");
        let events = self.transcript.observations.drain();
        let painted_count = events.painted.len();
        #[cfg(feature = "qualification")]
        self.qualification_paint.begin_frame(rows.scope(), frame);
        if let Some(clip) = crate::shell::wheel::keyed_node(ui, "conversation-transcript")
            .and_then(|node| ui.tree().visual_rect(node))
        {
            let active = ctx
                .window_state(window)
                .is_some_and(|window| window.focused);
            let markdown = workspace.markdown.borrow();
            let visible_count = events
                .painted
                .iter()
                .filter(|painted| {
                    painted.snapshot.active.get()
                        && painted.bounds.h > 0.0
                        && !painted.bounds.intersect(clip).is_empty()
                })
                .count();
            let tm_capture = crate::shell::application::Tm::new("fc.capture");
            let candidates: Vec<_> = events
                .painted
                .into_iter()
                .filter_map(|painted| {
                    #[cfg(feature = "qualification")]
                    let observation = std::rc::Rc::clone(&painted);
                    let candidate = painted.capture(
                        &workspace.conversation,
                        &shell.core,
                        &markdown,
                        rows.scope(),
                        clip,
                        active,
                    );
                    #[cfg(feature = "qualification")]
                    {
                        let painted = observation;
                        if painted.snapshot.active.get()
                            && painted.bounds.w > 0.0
                            && painted.bounds.h > 0.0
                            && !painted.bounds.intersect(clip).is_empty()
                        {
                            let item = if let crate::conversation::fold::Row::Text { id, .. } =
                                painted.snapshot.row.semantic()
                            {
                                Some(id.to_string())
                            } else {
                                None
                            };
                            self.qualification_paint.record(
                                painted.serial,
                                crate::qualification::observation::PaintedRow {
                                    key: format!("{:?}", painted.snapshot.row.key()),
                                    item,
                                    item_seq: painted.snapshot.item_seq,
                                    observed_seq: candidate
                                        .as_ref()
                                        .map(editor_conversation::view::transcript::ObservedRead::observed_seq),
                                    bounds_bits: crate::qualification::observation::rect_bits(
                                        painted.bounds,
                                    ),
                                    clip_bits: crate::qualification::observation::rect_bits(clip),
                                    revealed: painted.snapshot.revealed,
                                },
                            );
                        }
                    }
                    candidate
                })
                .collect();
            crate::core::diagnostics::open_timing::gate(
                &crate::core::diagnostics::open_timing::Gate {
                    reason: reveals.reason,
                    reveal: crate::core::diagnostics::open_timing::Reveal {
                        pending: reveals.pending,
                        changed: reveals.changed,
                        animating: reveals.animating,
                    },
                    active,
                    painted: painted_count,
                    visible: visible_count,
                    candidates: candidates.len(),
                    elapsed: lap_start.measured(),
                },
            );
            drop(tm_capture);
            let tm_ack = crate::shell::application::Tm::new("fc.ack");
            for candidate in candidates {
                candidate.acknowledge(&workspace.conversation, &mut shell.core, &markdown);
            }
            let watch = read_watch::ReadWatch::of(&workspace.conversation, &shell.core, active);
            if watch.wants_recheck(self.read_watch.as_ref()) {
                for painted in self.transcript.observations.on_screen(ui) {
                    if let Some(candidate) = painted.capture(
                        &workspace.conversation,
                        &shell.core,
                        &markdown,
                        rows.scope(),
                        clip,
                        active,
                    ) {
                        candidate.acknowledge(&workspace.conversation, &mut shell.core, &markdown);
                    }
                }
            }
            self.read_watch = Some(watch);
            drop(tm_ack);
            let _tm_viewports = crate::shell::application::Tm::new("fc.viewports");
            let landing_before = workspace.conversation.landing.is_some();
            for event in events.viewports {
                if event.active.get() {
                    let rebuild = rail_refresh::Rail {
                        navigation: &mut self.navigation_views.navigation,
                        marker: &mut self.navigation_views.navigation_marker,
                        transcript_viewport: self.scroll.transcript_viewport,
                    }
                    .refresh(
                        ui,
                        ctx,
                        window,
                        &workspace.conversation,
                        &shell.core,
                        event.viewport,
                    );
                    changed |= self.noted.note(Cause::Viewport, rebuild);
                    self.scroll.transcript_viewport = Some(event.viewport);
                    let was_following = workspace
                        .conversation
                        .view
                        .as_ref()
                        .is_some_and(|view| view.viewport.state().is_following());
                    let previous = workspace.conversation.rows_epoch();
                    match self.transcript.observations.observe_viewport(
                        &mut workspace.conversation,
                        &shell.core,
                        ui,
                        &markdown,
                        rows.scope(),
                        event.viewport,
                        clip,
                    ) {
                        Ok(()) => {
                            changed |= self
                                .noted
                                .note(Cause::Rows, previous != workspace.conversation.rows_epoch());
                            changed |= was_following
                                != workspace
                                    .conversation
                                    .view
                                    .as_ref()
                                    .is_some_and(|view| view.viewport.state().is_following());
                        }
                        Err(error) => {
                            tracing::warn!(%error, "could not observe transcript viewport");
                        }
                    }
                }
            }
            if landing_before && workspace.conversation.landing.is_none() {
                changed |= self.noted.note(Cause::Landed, true);
            }
        }
        drop(tm_events);
        aqua_core::checkpoint::reach();
        let _tm_after = crate::shell::application::Tm::new("fc.after");
        crate::conversation::view::transcript::apply_heights(
            &mut workspace.conversation,
            events.heights,
        );
        changed |= self.noted.note(
            Cause::Projections,
            rows.publish_projections(&mut workspace.conversation, &shell.core),
        );
        let after_layout = rows.after_layout(ui);
        let rebuilt_at = shell.core.clock().now();
        let motion_due = after_layout.rebuild
            && self.motion_rebuilt.get().is_none_or(|at| {
                rebuilt_at.saturating_duration_since(at) >= MOTION_REBUILD_INTERVAL
            });
        if motion_due {
            self.motion_rebuilt.set(Some(rebuilt_at));
        }
        if after_layout.rebuild {
            ctx.request_frame(window, aqua::prelude::FrameReason::ANIMATION);
        }
        self.noted.note(Cause::LayoutMotion, after_layout.rebuild);
        changed |= self.noted.note(Cause::AfterLayout, motion_due);
        let disclosure = workspace.conversation.advance_disclosure_frame(
            &shell.core,
            shell.core.clock().now(),
            reduced,
        );
        changed |= self.noted.note(
            Cause::Disclosure,
            !disclosure.changed.is_empty() || disclosure.refold || disclosure.active,
        );
        if disclosure.active {
            ctx.request_frame(window, aqua::prelude::FrameReason::ANIMATION);
        }
        self.sync_scroll(&mut workspace.conversation, ui, ctx, window);
        changed | action_changed
    }
}
