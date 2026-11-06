use super::RootViews;
use crate::conversation::controller::Conversation;
use crate::core::Core;
use crate::shell::host::Shell;
use crate::shell::thread_navigation::{self, Refresh};
use aqua::prelude::{AquaWindowId, FrameReason};
use aqua::{AppCtx, UiApp};
use aqua_ui::collection_runtime::CollectionViewport;
use std::time::Instant;

pub(super) struct Rail<'a> {
    pub(super) navigation: &'a mut thread_navigation::ThreadNavigation,
    pub(super) marker: &'a mut Option<usize>,
    pub(super) transcript_viewport: Option<CollectionViewport>,
    pub(super) composer_height: Option<f32>,
}

impl Rail<'_> {
    fn refresh_in_place(
        &mut self,
        ui: &mut UiApp,
        ctx: &mut AppCtx<'_>,
        window: AquaWindowId,
        conversation: &Conversation,
        core: &Core,
        viewport: CollectionViewport,
    ) -> bool {
        match self.navigation.refresh(
            ui,
            conversation,
            core,
            viewport,
            self.composer_height,
            Instant::now(),
        ) {
            Ok(Refresh::InPlace) => {
                ctx.request_frame(window, FrameReason::ANIMATION);
                false
            }
            Ok(Refresh::Rebuild) => true,
            Err(error) => {
                tracing::warn!(%error, "could not refresh turn navigator");
                true
            }
        }
    }

    pub(super) fn refresh(
        &mut self,
        ui: &mut UiApp,
        ctx: &mut AppCtx<'_>,
        window: AquaWindowId,
        conversation: &Conversation,
        core: &Core,
        viewport: CollectionViewport,
        reduced: bool,
    ) -> bool {
        let marker = thread_navigation::active_marker(conversation, core, viewport);
        let moves = thread_navigation::viewport_moves_navigation(
            self.transcript_viewport,
            viewport,
            *self.marker,
            marker,
        );
        let shifted = moves
            && thread_navigation::shifted_in_place(
                self.navigation,
                self.transcript_viewport,
                viewport,
                marker,
                Instant::now(),
                reduced,
            );
        if shifted {
            ctx.request_frame(window, FrameReason::ANIMATION);
        }
        let rebuild = moves
            && !shifted
            && self.refresh_in_place(ui, ctx, window, conversation, core, viewport);
        *self.marker = marker;
        rebuild
    }
}

impl RootViews {
    pub(super) fn refresh_navigation(
        &mut self,
        shell: &mut Shell,
        ui: &mut UiApp,
        ctx: &mut AppCtx<'_>,
        window: AquaWindowId,
    ) -> bool {
        let (Some(workspace), Some(viewport)) =
            (shell.entry.workspace(), self.transcript_viewport)
        else {
            return false;
        };
        Rail {
            navigation: &mut self.navigation,
            marker: &mut self.navigation_marker,
            transcript_viewport: self.transcript_viewport,
            composer_height: self.composer_height,
        }
        .refresh_in_place(ui, ctx, window, &workspace.conversation, &shell.core, viewport)
    }
}
