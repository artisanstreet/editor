use super::RootViews;
use crate::shell::host::Shell;
use aqua::UiApp;
use std::rc::Rc;

impl RootViews {
    pub(super) fn close_palette(&mut self, ui: &mut UiApp) {
        if let Some(dialog) = self.palette_dialog.take() {
            dialog.dispose(ui);
        }
        if let Some(palette) = self.palette.take() {
            palette.close();
        }
        self.palette_content = None;
        self.palette_events.borrow_mut().clear();
    }

    pub(super) fn palette_emit(&self) -> Rc<dyn Fn(crate::shell::palette::view::Event)> {
        let events = Rc::clone(&self.palette_events);
        Rc::new(move |event| events.borrow_mut().push_back(event))
    }

    pub(super) fn dispatch_palette(&mut self, shell: &mut Shell, ui: &mut UiApp) -> bool {
        use crate::shell::palette::view::Effect;
        let events = std::mem::take(&mut *self.palette_events.borrow_mut());
        let mut changed = false;
        for event in events {
            let outcome = match (self.palette.as_ref(), shell.entry.workspace_mut()) {
                (Some(view), Some(workspace)) => match workspace.palette.as_mut() {
                    Some(palette) => view.dispatch(palette, event),
                    None => continue,
                },
                _ => continue,
            };
            changed |= outcome.changed;
            match outcome.effect {
                Effect::None => {}
                Effect::Search(query) => {
                    if let Some(workspace) = shell.entry.workspace_mut() {
                        workspace.search_palette(&shell.core, query);
                    }
                }
                Effect::Close(screen) => {
                    self.close_palette(ui);
                    shell.dismiss_palette();
                    if let Some(screen) = screen {
                        shell.navigate(screen);
                    }
                }
            }
        }
        changed
    }

    pub(super) fn mount_palette(&mut self, shell: &Shell, ui: &mut UiApp) {
        let Some(content) = self.palette_content.take() else {
            return;
        };
        if let Some(dialog) = &self.palette_dialog {
            if dialog.is_open(ui) {
                dialog.set_theme(shell.preferences.theme());
                if let Err(error) = dialog.refresh_content(ui, content) {
                    tracing::warn!(%error, "could not refresh command palette");
                }
                return;
            }
            self.palette_dialog = None;
        }
        let Some(owner) = ui.find_debug_selector("workspace-root") else {
            self.palette_content = Some(content);
            return;
        };
        let emit = self.palette_emit();
        let session = self.palette.as_ref().map(|view| view.session());
        let dialog = crate::ui::dialog::Dialog::new(
            "command-palette",
            shell.preferences.theme(),
            "Search threads and commands",
            content,
        )
        .on_dismiss(move |_| {
            if let Some(session) = session {
                emit(crate::shell::palette::view::Event {
                    session,
                    action: crate::shell::palette::view::Action::Dismiss,
                });
            }
        });
        match crate::ui::dialog::DialogHost::open(ui, owner, dialog) {
            Ok(dialog) => {
                self.palette_dialog = Some(dialog);
                if let Some(view) = &self.palette {
                    view.sync_query(ui);
                    view.focus_query(ui);
                }
            }
            Err(error) => tracing::warn!(%error, "could not open command palette"),
        }
    }
}
