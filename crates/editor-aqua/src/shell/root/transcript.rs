use super::RootViews;
use crate::core::Screen;
use crate::shell::host::Shell;
use aqua::prelude::AquaWindowId;
use aqua::{AppCtx, UiApp};
use std::rc::Rc;

impl RootViews {
    pub(crate) fn replace_rows(
        &mut self,
        rows: Option<Rc<crate::conversation::view::factory::RowFactory>>,
    ) {
        if let Some(previous) = self.rows.take() {
            previous.dispose();
        }
        if rows.is_none() {
            if let Some(token) = self.row_token.take() {
                token.set(false);
            }
            self.row_settled.borrow_mut().clear();
            self.row_cache.borrow_mut().rebind(None);
            self.stored.borrow_mut().rebind(None);
        }
        self.rows = rows;
    }

    pub(super) fn sync_context(
        &mut self,
        shell: &Shell,
        ui: &mut UiApp,
        ctx: &mut AppCtx<'_>,
        window: AquaWindowId,
    ) {
        let _tm = crate::shell::application::Tm::new("v.context");
        self.ui_now = ctx.now();
        let reduced = ui.host().motion().reduced_motion() == aqua_ui::motion::ReducedMotion::Reduce;
        let now = shell.core.clock().now();
        if let Some(deadline) = [
            self.connection.deadline(now, reduced),
            self.thread_loading.deadline(now, reduced),
            self.thread_landing.deadline(now, reduced),
        ]
        .into_iter()
        .flatten()
        .min()
        {
            ctx.request_animation_deadline(
                window,
                ctx.now() + deadline.saturating_duration_since(now),
            );
        }
        self.request_landing_deadline(shell, ctx, window, now);
        if self.connection.animating(now, reduced)
            || self.thread_loading.animating(now, reduced)
            || self.thread_landing.animating(now, reduced)
        {
            ctx.request_frame(window, aqua::prelude::FrameReason::ANIMATION);
        }
    }

    pub(super) fn sync_scroll(
        &mut self,
        conversation: &mut crate::conversation::controller::Conversation,
        ui: &mut UiApp,
        ctx: &mut AppCtx<'_>,
        window: AquaWindowId,
    ) {
        let _tm = crate::shell::application::Tm::new("v.scroll");
        use crate::conversation::scroll::Step;
        self.take_scroll_request(conversation);
        let Some(node) = crate::shell::wheel::keyed_node(ui, "conversation-transcript") else {
            return;
        };
        let Some(rect) = ui.tree().rect(node) else {
            return;
        };
        let Some(content) = ui.tree().content_size(node) else {
            return;
        };
        if rect.h <= 0.0 {
            return;
        }
        let maximum = (content.h - rect.h).max(0.0);
        let reduced = ui.host().motion().reduced_motion() == aqua_ui::motion::ReducedMotion::Reduce;
        let now = std::time::Instant::now();
        let mut changed = false;
        if let Some(action) = &self.scroll_request {
            use crate::conversation::adapter::ScrollResolution;
            match crate::conversation::adapter::resolve_scroll(conversation, action, reduced) {
                ScrollResolution::Cancelled => self.scroll_request = None,
                ScrollResolution::AwaitingRows => {}
                ScrollResolution::Ready(directive) => {
                    let x = ui.tree().scroll_offset(node).map_or(0.0, |offset| offset.x);
                    if let Some(moved) = crate::conversation::scroll::apply_directive(
                        ui,
                        &mut self.smooth_scroll,
                        &directive,
                        now,
                        x,
                    ) {
                        changed |= moved;
                        self.scroll_request = None;
                    }
                }
            }
        }
        let current = ui.tree().scroll_offset(node).unwrap_or_default();
        let following = conversation
            .view
            .as_ref()
            .is_some_and(|view| view.viewport.state().is_following());
        let step = self.smooth_scroll.step(
            following,
            reduced,
            conversation.is_landing(),
            !conversation.disclosures.disclosures.is_empty(),
            crate::conversation::scroll::following_tail(ui),
            -current.y,
            maximum,
            now,
        );
        let animating = matches!(
            step,
            Step::Scroll {
                continue_animation: true,
                ..
            }
        );
        changed |= crate::conversation::scroll::apply_step(ui, step, current.x);
        if changed || animating {
            ctx.request_frame(
                window,
                if animating {
                    aqua::prelude::FrameReason::ANIMATION
                } else {
                    aqua::prelude::FrameReason::EXPLICIT
                },
            );
        }
    }

    pub(super) fn sync_edges(
        &mut self,
        shell: &mut Shell,
        ui: &mut UiApp,
        ctx: &mut AppCtx<'_>,
        window: AquaWindowId,
    ) {
        let _tm = crate::shell::application::Tm::new("v.edges");
        use crate::ui::scroll_edge::EdgeSurface;
        let reduced = ui.host().motion().reduced_motion() == aqua_ui::motion::ReducedMotion::Reduce;
        let now = std::time::Instant::now();
        let theme = shell.preferences.theme();
        let Some(workspace) = shell.entry.workspace() else {
            return;
        };
        let mut moving = EdgeSurface {
            edges: &self.transcript_edges,
            scroller: "conversation-transcript",
            top: "conversation-edge-top",
            bottom: "conversation-edge-bottom",
            top_color: theme.colors.background,
            bottom_color: theme.colors.background,
            paint: crate::ui::scroll_edge::EdgePaint::Gradient,
        }
        .advance(ui, reduced, now);
        moving |= EdgeSurface {
            edges: &workspace.sidebar.threads_edges,
            scroller: crate::shell::wheel::THREADS,
            top: "sidebar-threads-edge-top",
            bottom: "sidebar-threads-edge-bottom",
            top_color: theme.colors.frame,
            bottom_color: theme.colors.frame,
            paint: crate::ui::scroll_edge::EdgePaint::Gradient,
        }
        .advance(ui, reduced, now);
        if let Some(composer) = &workspace.composer {
            moving |= composer.advance_edges(ui, &theme, reduced, now);
        }
        if moving {
            ctx.request_frame(window, aqua::prelude::FrameReason::ANIMATION);
        }
    }

    pub(super) fn sync_transcript_reveal(shell: &mut Shell, ui: &mut UiApp) -> bool {
        let Some(workspace) = shell.entry.workspace_mut() else {
            return false;
        };
        if Self::reduced(ui) {
            workspace.conversation.skip_reveal();
        }
        let reveal = workspace
            .conversation
            .reveal(workspace.conversation.clock().now());
        let Some(node) =
            ui.tree()
                .sole_node_with_key(aqua_ui::tree::hash_key(aqua_ui::element::Key::from(
                    crate::conversation::view::TRANSCRIPT_FRAME,
                )))
        else {
            return false;
        };
        let Some(mut paint) = ui.tree().get(node).map(|record| record.paint().clone()) else {
            return false;
        };
        if paint.opacity != Some(reveal) {
            paint.opacity = Some(reveal);
            ui.tree_mut().set_paint(node, paint);
        }
        reveal > 0.0 && reveal < 1.0
    }

    pub(super) fn transcript_refreshable(&self, shell: &Shell, ui: &UiApp) -> bool {
        self.image_preview.is_none()
            && Self::preview_root(ui).is_none()
            && self.color_scope.is_none()
            && self.rows.is_some()
            && matches!(shell.core.workspace_screen(), Screen::Thread { .. })
            && shell.entry.workspace().is_some_and(|workspace| {
                self.workspace_key == Some(workspace.selections.key)
                    && !workspace.conversation.rows.is_empty()
                    && !workspace.conversation.is_landing()
                    && workspace.conversation.host_is_current(&shell.core)
                    && crate::shell::thread_navigation::latest_visible(
                        &workspace.conversation,
                        &shell.core,
                        workspace.composer.is_some(),
                    ) == crate::shell::wheel::keyed_node(ui, "jump-latest").is_some()
                    && self.transcript_landing == Some(workspace.conversation.is_landing())
            })
    }
}
