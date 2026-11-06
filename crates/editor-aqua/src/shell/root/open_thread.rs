use super::RootViews;
use crate::shell::host::Shell;
use aqua::AppCtx;
use aqua::prelude::AquaWindowId;
use std::time::Instant;

#[derive(Default)]
pub(super) struct PendingStall {
    epoch: u64,
    streak: u32,
}

impl PendingStall {
    const LIMIT: u32 = 4;

    pub(super) fn observe(&mut self, pending: bool, epoch: u64) -> bool {
        self.streak = match pending && self.epoch == epoch {
            true => self.streak.saturating_add(1),
            false => 0,
        };
        self.epoch = epoch;
        pending && self.streak < Self::LIMIT
    }
}

impl RootViews {
    pub(super) fn request_landing_deadline(
        &self,
        shell: &Shell,
        ctx: &mut AppCtx<'_>,
        window: AquaWindowId,
        now: Instant,
    ) {
        if let Some(landing) = shell
            .entry
            .workspace()
            .and_then(|workspace| workspace.conversation.landing)
            .filter(|landing| *landing > now)
        {
            ctx.request_animation_deadline(
                window,
                ctx.now() + landing.saturating_duration_since(now),
            );
        }
    }
}
