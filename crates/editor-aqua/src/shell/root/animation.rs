use super::{RootViews, reveal};
use crate::core::Screen;
use crate::shell::host::Shell;
use crate::shell::refresh::Cause;
use aqua::prelude::AquaWindowId;
use aqua::{AppCtx, UiApp};

impl RootViews {
    pub(super) fn tick_work_clock(
        &mut self,
        shell: &Shell,
        ctx: &mut AppCtx<'_>,
        window: AquaWindowId,
        now: std::time::Instant,
    ) -> bool {
        let working = match shell.core.screen() {
            Screen::Thread { host, thread } => shell
                .core
                .host(host)
                .is_some_and(|host| host.is_working(thread)),
            _ => false,
        };
        if !working {
            self.work_clock = None;
            return false;
        }
        let due = self.work_clock.is_none_or(|at| now >= at);
        let next = match (due, self.work_clock) {
            (false, Some(at)) => at,
            _ => {
                let into_second = shell
                    .core
                    .clock()
                    .timestamp()
                    .rem_euclid(1_000)
                    .unsigned_abs();
                now + std::time::Duration::from_millis(1_000 - into_second)
            }
        };
        self.work_clock = Some(next);
        ctx.request_animation_deadline(window, ctx.now() + next.saturating_duration_since(now));
        due
    }

    pub(super) fn reduced(ui: &UiApp) -> bool {
        ui.host().motion().reduced_motion() == aqua_ui::motion::ReducedMotion::Reduce
    }

    pub(super) fn step_animation(
        &mut self,
        shell: &mut Shell,
        ui: &mut UiApp,
        ctx: &mut AppCtx<'_>,
        window: AquaWindowId,
    ) -> bool {
        let reduced = Self::reduced(ui);
        let now = shell.core.clock().now();
        let signing_in = matches!(
            shell.core.entry_gate(),
            crate::core::EntryGate::Restoring | crate::core::EntryGate::SignedOut
        );
        let signing =
            (signing_in && shell.entry.sign_in.tick(now)) | std::mem::take(&mut self.sign_in_dirty);
        if signing_in && shell.entry.sign_in.animating() {
            ctx.request_frame(window, aqua::prelude::FrameReason::ANIMATION);
        }
        let sidebar_resizing = self.sidebar.resizing();
        let revealing = self.advance_reveal(shell, ui);
        if revealing != reveal::Progress::Idle {
            ctx.request_animation_deadline(window, ctx.now() + reveal::POLL);
        }
        if revealing == reveal::Progress::Acted {
            self.conversation_invalidated.set(true);
            ctx.request_frame(window, aqua::prelude::FrameReason::EXPLICIT);
        }
        let sidebar_events = self.sidebar.take_events(now, reduced);
        let clock = self.tick_work_clock(shell, ctx, window, now);
        let sidebar_moved = self.sidebar.tick(now, reduced);
        let sidebar = match (sidebar_resizing, self.sidebar.resizing()) {
            (true, false) => Cause::SidebarSettled,
            (true | false, true) => Cause::SidebarResize,
            (false, false) => Cause::SidebarAnimation,
        };
        let causes = [
            (Cause::RevealActed, revealing == reveal::Progress::Acted),
            (Cause::SidebarEvents, sidebar_events),
            (Cause::WorkClock, clock),
            (sidebar, sidebar_moved),
            (Cause::Signing, signing),
        ];
        let mut changed = false;
        for (cause, fired) in causes {
            if fired {
                crate::core::diagnostics::rebuild_stats::record_event(cause.label());
            }
            changed |= self.noted.note(cause, fired);
        }
        self.apply_account_effects(shell, ctx, window, reduced);
        if self.sidebar.menu().open() && sidebar_moved {
            self.sync_account_menu(shell, ui, ctx, window);
        }

        match self.account_popup.advance(ui, ctx.now().as_secs_f64()) {
            Ok(true) => ctx.request_frame(window, aqua::prelude::FrameReason::ANIMATION),
            Ok(false) => {}
            Err(error) => tracing::warn!(%error, "could not animate the account menu"),
        }
        self.schedule_sidebar(ctx, window);
        changed
    }
}
