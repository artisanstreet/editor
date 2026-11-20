use super::RootViews;
use crate::shell::host::Shell;
use crate::shell::refresh::Cause;
use aqua::prelude::{AquaWindowId, InputEvent};
use aqua::{AppCtx, UiApp};

impl RootViews {
    pub(super) fn intercept_input(
        &mut self,
        shell: &Shell,
        ui: &mut UiApp,
        input: &InputEvent,
        now: aqua::Instant,
    ) -> Option<bool> {
        if let InputEvent::PointerDown(pointer, _) = input
            && self.navigation.dismiss(
                ui,
                pointer.position,
                shell.core.clock().now(),
                Self::reduced(ui),
            )
        {
            self.navigation_invalidated.set(true);
        }
        if let InputEvent::PointerDown(pointer, aqua_core::PointerButton::Secondary) = input
            && self.image_preview.is_none()
            && ui
                .selection_rects()
                .iter()
                .any(|rect| rect.contains(pointer.position))
            && let Some(text) = ui.selected_text().filter(|text| !text.trim().is_empty())
        {
            self.selection_menu = Some((pointer.position, text));
            return Some(true);
        }
        match self.account_popup.dismiss(ui, input) {
            Ok(true) => {
                (self.sidebar.emit())(crate::frame::sidebar::Event::Account(
                    crate::ui::chrome::menu::Event::Dismissed,
                ));
                return Some(true);
            }
            Ok(false) => {}
            Err(error) => tracing::warn!(%error, "could not dismiss the account menu"),
        }
        let dismissal = {
            let _tm = crate::shell::application::Tm::new("pre.overlays");
            self.sidebar_overlays.request_dismissal(ui, input)
        };
        match dismissal {
            Ok(true) => return Some(true),
            Ok(false) => {}
            Err(error) => tracing::warn!(%error, "could not dismiss sidebar overlay"),
        }
        let intercepted = self
            .color_menu
            .as_mut()
            .map_or(Ok(false), |menu| menu.intercept(ui, input));
        match intercepted {
            Ok(true) => return Some(true),
            Ok(false) => {}
            Err(error) => tracing::warn!(%error, "could not close color copy menu"),
        }
        if let InputEvent::KeyDown(event) = input
            && self.palette_dialog.is_none()
            && self.resource_dialog.is_none()
            && self.settings.is_none()
            && self.image_preview.is_none()
            && self.navigation.key(event)
        {
            self.navigation_invalidated.set(true);
            return Some(true);
        }
        let cycled = {
            let _tm = crate::shell::application::Tm::new("pre.cycle");
            crate::ui::menu_keys::cycle(ui, input)
        };
        if cycled {
            return Some(true);
        }
        let _tm = crate::shell::application::Tm::new("pre.wheel");
        self.wheel.offer(ui, input, now).map(|_| true)
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one input router trying each overlay and surface in priority order"
    )]
    pub(super) fn route_input(
        &mut self,
        shell: &mut Shell,
        ui: &mut UiApp,
        ctx: &mut AppCtx<'_>,
        window: AquaWindowId,
        input: &InputEvent,
        consumed: bool,
    ) -> bool {
        self.sync_context(shell, ui, ctx, window);
        if self.sync_navigation(ui, ctx, window, shell.core.clock().now()) {
            crate::core::diagnostics::rebuild_stats::record_event("inv_nav_input");
            self.navigation_invalidated.set(true);
        }
        if let InputEvent::Scroll { at, delta } = input
            && self.image_preview.is_none()
            && (delta.x != 0.0 || delta.y != 0.0)
            && crate::shell::wheel::keyed_node(ui, "conversation-transcript")
                .and_then(|node| ui.tree().visual_rect(node))
                .is_some_and(|clip| clip.contains(*at))
        {
            self.observations.note_reader_move();
            if delta.y != 0.0 {
                self.observations.note_reader_direction(delta.y > 0.0);
            }
            self.smooth_scroll.cancel();
            if delta.y < 0.0 {
                crate::conversation::scroll::pause_tail(ui);
            }
        }
        if let Some(dialog) = &self.palette_dialog {
            dialog.sync(ui);
        }
        if let Some(dialog) = &self.resource_dialog {
            dialog.sync(ui);
        }
        let wheel_changed = {
            let _tm = crate::shell::application::Tm::new("input.views.wheel");
            self.sync_wheel(shell, ui, ctx, window)
        };
        let palette = {
            let _tm = crate::shell::application::Tm::new("input.views.palette");
            self.dispatch_palette(shell, ui) | self.dispatch_resource(shell, ui, ctx)
        };
        let conversation = {
            let _tm = crate::shell::application::Tm::new("input.views.conversation");
            self.dispatch_conversation(shell, ui, ctx, window)
        };
        let sidebar = {
            let _tm = crate::shell::application::Tm::new("input.views.sidebar");
            self.sync_sidebar(shell, ui, ctx, window)
        };
        let palette_changed = self.noted.note(Cause::WheelInput, wheel_changed)
            | self.noted.note(Cause::PaletteInput, palette)
            | self.noted.note(Cause::ConversationInput, conversation)
            | self.noted.note(Cause::SidebarInput, sidebar);
        let failure_changed = if self.failure_dismissed.replace(false) {
            shell
                .entry
                .workspace_mut()
                .is_some_and(|workspace| workspace.failure.take().is_some())
        } else {
            false
        };
        let palette_changed = palette_changed | failure_changed;
        if let Some(dialog) = &self.settings {
            dialog.sync(ui);
        }
        if self.settings_dismissed.replace(false) {
            shell.core.close_settings();
            return true;
        }
        if consumed || self.image_preview.is_some() {
            return palette_changed;
        }
        let InputEvent::KeyDown(event) = input else {
            return palette_changed;
        };
        let modifiers = event.modifiers;
        let command = modifiers.contains(aqua_core::Modifiers::CONTROL)
            || modifiers.contains(aqua_core::Modifiers::LOGO);
        if command
            && !modifiers.contains(aqua_core::Modifiers::ALT)
            && !modifiers.contains(aqua_core::Modifiers::SHIFT)
            && matches!(event.key, aqua_core::Key::Char('q' | 'Q'))
        {
            ctx.quit();
            return false;
        }
        if shell.entry.workspace().is_none() {
            return palette_changed;
        }
        if modifiers.is_empty()
            && event.key == aqua_core::Key::Named(aqua_core::NamedKey::Escape)
            && self
                .navigation
                .close(shell.core.clock().now(), Self::reduced(ui))
        {
            self.navigation_invalidated.set(true);
            ctx.request_frame(window, aqua::prelude::FrameReason::ANIMATION);
            return true;
        }
        if modifiers == aqua_core::Modifiers::ALT
            && event.key == aqua_core::Key::Named(aqua_core::NamedKey::Left)
        {
            shell.go_back();
            return true;
        }
        if !command || modifiers.contains(aqua_core::Modifiers::SHIFT) {
            return palette_changed;
        }
        let alt = modifiers.contains(aqua_core::Modifiers::ALT);
        match event.key {
            aqua_core::Key::Char('b' | 'B') if alt => {
                if let Some(workspace) = shell.entry.workspace_mut() {
                    workspace.side_panel = workspace.side_panel.toggled();
                    return true;
                }
            }
            aqua_core::Key::Char('b' | 'B') if !alt => {
                (self.sidebar.emit())(crate::frame::sidebar::Event::Toggle);
                ctx.request_frame(window, aqua::prelude::FrameReason::ANIMATION);
                return true;
            }
            aqua_core::Key::Char(',') if !alt => {
                shell.open_settings();
                return true;
            }
            aqua_core::Key::Char('n' | 'N') if !alt => {
                shell.new_thread();
                return true;
            }
            aqua_core::Key::Char('k' | 'K') if !alt => {
                shell.open_palette(ctx, window);
                return true;
            }
            aqua_core::Key::Char(_) | aqua_core::Key::Named(_) => {}
        }
        palette_changed
    }

    pub(super) fn wake_views(
        &mut self,
        shell: &mut Shell,
        ui: &mut UiApp,
        ctx: &mut AppCtx<'_>,
        window: AquaWindowId,
    ) -> bool {
        self.sync_context(shell, ui, ctx, window);
        if self.work_clock.is_none() {
            let now = shell.core.clock().now();
            self.tick_work_clock(shell, ctx, window, now);
        }
        if self.sync_navigation(ui, ctx, window, shell.core.clock().now()) {
            crate::core::diagnostics::rebuild_stats::record_event("inv_nav_a");
            self.navigation_invalidated.set(true);
        }
        let palette_changed = self.dispatch_palette(shell, ui)
            | self.dispatch_resource(shell, ui, ctx)
            | self.dispatch_conversation(shell, ui, ctx, window)
            | self.sync_sidebar(shell, ui, ctx, window);
        let link_changed = match shell.failures.drain(ui) {
            Some(crate::shell::action_failure::Effect::CopyUrl(url)) => {
                if let Err(error) = ctx.clipboard().set_text(url) {
                    shell
                        .failures
                        .copy(crate::shell::action_failure::CopySubject::Link, &error);
                }
                true
            }
            None => false,
        };
        if self.settings_dismissed.replace(false) {
            shell.core.close_settings();
            true
        } else {
            palette_changed | link_changed
        }
    }
}
