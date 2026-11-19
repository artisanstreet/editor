use super::RootViews;
use crate::shell::host::Shell;
use aqua::prelude::AquaWindowId;
use aqua::{AppCtx, UiApp};

impl RootViews {
    pub(super) fn take_scroll_request(
        &mut self,
        conversation: &mut crate::conversation::controller::Conversation,
    ) {
        if let Some(latest) = conversation.scroll_actions().pop() {
            self.scroll_request = Some(latest);
        }
    }

    fn follow_tail_early(
        &mut self,
        conversation: &mut crate::conversation::controller::Conversation,
        ui: &mut UiApp,
    ) {
        use crate::conversation::adapter::{ScrollDirective, ScrollResolution, resolve_scroll};
        self.take_scroll_request(conversation);
        let Some(action) = &self.scroll_request else {
            return;
        };
        let reduced = ui.host().motion().reduced_motion() == aqua_ui::motion::ReducedMotion::Reduce;
        let landing = *action == crate::conversation::controller::ScrollAction::Tail;
        let directive = match resolve_scroll(conversation, action, reduced) {
            ScrollResolution::Ready(directive @ ScrollDirective::FollowEnd) => directive,
            ScrollResolution::Cancelled => {
                self.scroll_request = None;
                return;
            }
            ScrollResolution::Ready(ScrollDirective::EaseToEnd | ScrollDirective::ToRow { .. })
            | ScrollResolution::AwaitingRows => return,
        };
        if crate::conversation::scroll::apply_directive(
            ui,
            &mut self.smooth_scroll,
            &directive,
            std::time::Instant::now(),
            0.0,
        )
        .is_some()
        {
            if landing {
                self.wheel.cancel_transcript();
            }
            self.scroll_request = None;
        }
    }

    pub(super) fn sync_wheel(
        &mut self,
        shell: &mut Shell,
        ui: &mut UiApp,
        ctx: &mut AppCtx<'_>,
        window: AquaWindowId,
    ) -> bool {
        let advance = self.wheel.advance(ui, std::time::Instant::now());
        if advance.transcript_moved {
            self.observations.note_reader_move();
        }
        if advance.threads_moved
            && let Some(workspace) = shell.entry.workspace_mut()
        {
            workspace.sidebar.page_at_viewport(&mut shell.core, true);
        }
        if advance.active {
            ctx.request_frame(window, aqua::prelude::FrameReason::ANIMATION);
        }
        advance.threads_moved
    }

    pub(super) fn advance_wheel_glides(&mut self, shell: &mut Shell, ui: &mut UiApp) -> bool {
        if let Some(workspace) = shell.entry.workspace_mut() {
            self.follow_tail_early(&mut workspace.conversation, ui);
        }
        let advance = self.wheel.advance(ui, std::time::Instant::now());
        if advance.threads_moved
            && let Some(workspace) = shell.entry.workspace_mut()
        {
            workspace.sidebar.page_at_viewport(&mut shell.core, true);
        }
        advance.active
    }
}
