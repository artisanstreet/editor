use super::RootViews;
use crate::shell::action_failure::CopySubject;
use crate::shell::host::Shell;
use aqua::prelude::AquaWindowId;
use aqua::{AppCtx, UiApp};
use std::rc::Rc;

impl RootViews {
    pub(super) fn sync_resource_jump(
        &mut self,
        ui: &mut UiApp,
        ctx: &mut AppCtx<'_>,
        window: AquaWindowId,
    ) {
        let _tm = crate::shell::application::Tm::new("v.resource_jump");
        let Some((_, state)) = &mut self.dialogs.resource else {
            return;
        };
        if state.pending_jump().is_none() {
            return;
        }
        let changed = state.sync_jump(ui);
        if changed {
            ctx.request_frame(window, aqua::prelude::FrameReason::EXPLICIT);
        }
    }

    pub(super) fn resource_emit(&self) -> Rc<dyn Fn(crate::shell::resource_preview::view::Action)> {
        let events = Rc::clone(&self.dialogs.resource_events);
        Rc::new(move |event| events.borrow_mut().push_back(event))
    }

    pub(super) fn dispatch_resource(
        &mut self,
        shell: &mut Shell,
        ui: &mut UiApp,
        ctx: &mut AppCtx<'_>,
    ) -> bool {
        use crate::shell::resource_preview::view::Action;
        let events = std::mem::take(&mut *self.dialogs.resource_events.borrow_mut());
        let mut changed = false;
        for event in events {
            let generation = match &event {
                Action::CopyPath { generation, .. }
                | Action::CopyFile { generation, .. }
                | Action::Dismiss { generation }
                | Action::Retry { generation } => *generation,
            };
            if self.dialogs.resource.as_ref().map(|(current, _)| *current) != Some(generation)
                || !shell.entry.workspace().is_some_and(|workspace| {
                    workspace.resource_preview.is_some()
                        && workspace.resource_preview_generation() == generation
                })
            {
                continue;
            }
            match event {
                Action::CopyPath { text, .. } => {
                    if let Err(error) = ctx.clipboard().set_text(text) {
                        shell.failures.copy(CopySubject::ResourcePath, &error);
                        changed = true;
                    }
                }
                Action::CopyFile { text, .. } => {
                    if let Err(error) = ctx.clipboard().set_text(text.to_string()) {
                        shell.failures.copy(CopySubject::File, &error);
                        changed = true;
                    }
                }
                Action::Dismiss { .. } => {
                    self.dialogs.close_resource(ui);
                    shell.dismiss_reference();
                    changed = true;
                }
                Action::Retry { .. } => changed |= shell.retry_reference(),
            }
        }
        changed
    }

    pub(super) fn mount_resource(&mut self, shell: &Shell, ui: &mut UiApp) {
        let Some((content, title)) = self.dialogs.resource_content.take() else {
            return;
        };
        if let Some(dialog) = &self.dialogs.resource_dialog {
            if dialog.is_open(ui) {
                dialog.set_theme(shell.preferences.theme());
                if let Err(error) = dialog.refresh_content(ui, content) {
                    tracing::warn!(error = %diagnostics::chain(&error), "could not refresh resource preview");
                }
                return;
            }
            self.dialogs.resource_dialog = None;
        }
        let Some(owner) = ui.find_debug_selector("workspace-root") else {
            self.dialogs.resource_content = Some((content, title));
            return;
        };
        let emit = self.resource_emit();
        let generation = self
            .dialogs
            .resource
            .as_ref()
            .map(|(generation, _)| *generation);
        let dialog = crate::ui::dialog::Dialog::new(
            "resource-preview",
            shell.preferences.theme(),
            title,
            content,
        )
        .max_width(1100.0)
        .on_dismiss(move |_| {
            if let Some(generation) = generation {
                emit(crate::shell::resource_preview::view::Action::Dismiss { generation });
            }
        });
        match crate::ui::dialog::DialogHost::open(ui, owner, dialog) {
            Ok(dialog) => self.dialogs.resource_dialog = Some(dialog),
            Err(error) => {
                tracing::warn!(error = %diagnostics::chain(&error), "could not open resource preview");
            }
        }
    }
}
