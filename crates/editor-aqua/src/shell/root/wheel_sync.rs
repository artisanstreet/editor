use super::RootViews;
use crate::shell::host::Shell;
use aqua::prelude::AquaWindowId;
use aqua::{AppCtx, UiApp};

impl RootViews {
    pub(super) fn follow_tail_early(
        &mut self,
        conversation: &mut crate::conversation::controller::Conversation,
        ui: &mut UiApp,
    ) {
        use crate::conversation::adapter::ScrollDirective;
        self.scroll_requests.extend(conversation.scroll_actions());
        let reduced = ui.host().motion().reduced_motion() == aqua_ui::motion::ReducedMotion::Reduce;
        let now = std::time::Instant::now();
        while let Some(action) = self.scroll_requests.front() {
            let Some(directive @ ScrollDirective::FollowEnd) =
                crate::conversation::adapter::scroll_directive(conversation, action.clone(), reduced)
            else {
                break;
            };
            let Some(_) = crate::conversation::scroll::apply_directive(
                ui,
                &mut self.smooth_scroll,
                &directive,
                now,
                0.0,
            ) else {
                break;
            };
            self.scroll_requests.pop_front();
        }
    }

    pub(super) fn sync_wheel(
        &mut self,
        shell: &mut Shell,
        ui: &mut UiApp,
        ctx: &mut AppCtx<'_>,
        window: AquaWindowId,
    ) -> bool {
        let advance = self.wheel.advance(ui);
        if advance.transcript_moved {
            self.observations.note_reader_move();
        }
        if advance.threads_moved {
            if let Some(workspace) = shell.entry.workspace_mut() {
                workspace.sidebar.page_at_viewport(&mut shell.core, true);
            }
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
        let advance = self.wheel.advance(ui);
        if advance.threads_moved {
            if let Some(workspace) = shell.entry.workspace_mut() {
                workspace.sidebar.page_at_viewport(&mut shell.core, true);
            }
        }
        advance.active
    }
}
