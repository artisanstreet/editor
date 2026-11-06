use crate::core::Core;
use crate::sources::{ContentAction, ContentEffect, ContentText, ContentView};
use crate::ui::theme::ArtisanTheme;
use aqua::prelude::AquaWindowId;
use aqua::{AppCtx, UiApp};
use aqua_text::document::{SelectionDocumentId, SelectionSourceError, SelectionSourceId};
use std::rc::Rc;

type SourceDialogResult = Result<Option<std::path::PathBuf>, String>;

pub struct RowSourceDialogs {
    next: u64,
    pending: std::collections::HashMap<
        u64,
        crate::conversation::view::row_sources::RowSourceExport,
    >,
    picked: crate::core::delivery::Delivery<(u64, SourceDialogResult)>,
}

impl Default for RowSourceDialogs {
    fn default() -> Self {
        Self {
            next: 0,
            pending: Default::default(),
            picked: crate::core::delivery::Delivery::new(std::num::NonZeroUsize::MIN),
        }
    }
}

impl RowSourceDialogs {
    pub fn bind(&self, ctx: &AppCtx<'_>, window: AquaWindowId) {
        if let Some(mailbox) = self.picked.wake_mailbox() {
            ctx.bind_mailbox(window, mailbox);
        }
    }

    pub fn retain(
        &mut self,
        mut current: impl FnMut(&crate::conversation::view::row_sources::RowSourceExport) -> bool,
    ) {
        self.pending.retain(|_, export| current(export));
    }

    pub fn open(
        &mut self,
        export: crate::conversation::view::row_sources::RowSourceExport,
        ctx: &mut AppCtx<'_>,
        routes: &crate::shell::dialogs::DialogRoutes,
        window: AquaWindowId,
    ) -> Option<(
        crate::conversation::view::row_sources::RowSourceExport,
        SourceDialogResult,
    )> {
        let request = ctx.dialog(window, crate::shell::dialogs::save_file_spec("source.txt"));
        self.next = self.next.wrapping_add(1);
        let generation = self.next;
        let sender = self.picked.sender();
        routes.route(request, move |outcome| {
            let result = crate::shell::dialogs::chosen_path(outcome);
            crate::core::runtime::runtime().spawn(async move {
                let _ = sender.send((generation, result)).await;
            });
        });
        self.pending.insert(generation, export);
        None
    }

    pub fn drain(
        &mut self,
    ) -> Vec<(
        crate::conversation::view::row_sources::RowSourceExport,
        SourceDialogResult,
    )> {
        self.picked
            .drain()
            .into_iter()
            .filter_map(|(generation, result)| {
                self.pending
                    .remove(&generation)
                    .map(|export| (export, result))
            })
            .collect()
    }
}

pub struct SourceView {
    document: SelectionDocumentId,
    source: SelectionSourceId,
    picked: crate::core::delivery::Delivery<(u64, Result<Option<std::path::PathBuf>, String>)>,
}

impl SourceView {
    pub fn new(document: SelectionDocumentId, source: SelectionSourceId) -> Self {
        Self {
            document,
            source,
            picked: crate::core::delivery::Delivery::new(std::num::NonZeroUsize::MIN),
        }
    }

    pub fn bind(&self, content: &ContentText, ctx: &AppCtx<'_>, window: AquaWindowId) {
        ctx.bind_mailbox(window, content.action_wake());
        ctx.bind_mailbox(window, content.window().wake_mailbox());
        if let Some(mailbox) = self.picked.wake_mailbox() {
            ctx.bind_mailbox(window, mailbox);
        }
    }

    pub fn render(
        &self,
        ui: &mut UiApp,
        content: &ContentText,
        theme: ArtisanTheme,
        action: Rc<dyn Fn(ContentAction)>,
    ) -> Result<aqua_ui::element::Element, SelectionSourceError> {
        let ContentView { element, selection } =
            crate::sources::content_view(content, theme, self.document, self.source, action);
        if let Some(selection) = selection {
            ui.register_selection_source_in(self.document, selection)?;
        } else {
            ui.unregister_selection_source_in(self.document, self.source);
        }
        Ok(element)
    }

    pub fn release(&self, ui: &mut UiApp) {
        ui.unregister_selection_source_in(self.document, self.source);
    }

    pub fn action(
        &mut self,
        core: &Core,
        content: &mut ContentText,
        ctx: &mut AppCtx<'_>,
        routes: &crate::shell::dialogs::DialogRoutes,
        window: AquaWindowId,
        action: ContentAction,
    ) -> bool {
        let before = content.view_state();
        let (changed, effect) = crate::sources::dispatch_content_changed(content, core, action);
        if let Some(ContentEffect::SavePath(generation)) = effect {
            let request = ctx.dialog(window, crate::shell::dialogs::save_file_spec("source.txt"));
            let sender = self.picked.sender();
            routes.route(request, move |outcome| {
                let result = crate::shell::dialogs::chosen_path(outcome);
                crate::core::runtime::runtime().spawn(async move {
                    let _ = sender.send((generation, result)).await;
                });
            });
        }
        changed || content.view_state() != before
    }

    pub fn drain(&mut self, core: &Core, content: &mut ContentText, ctx: &mut AppCtx<'_>) -> bool {
        let before = content.view_state();
        for (generation, result) in self.picked.drain() {
            content.finish_export_result(core, generation, result);
        }
        let (changed, effect) = crate::sources::drain_content_changed(content, core);
        if let Some(ContentEffect::Clipboard(copied)) = effect {
            copied.write(core, |text| {
                if let Err(error) = ctx.clipboard().set_text(text) {
                    tracing::warn!(%error, "could not copy source");
                }
            });
        }
        changed || content.view_state() != before
    }
}
