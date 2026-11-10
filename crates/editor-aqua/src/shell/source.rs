use aqua::prelude::AquaWindowId;
use aqua::AppCtx;

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
        routes: &crate::ui::dialogs::DialogRoutes,
        window: AquaWindowId,
    ) -> Option<(
        crate::conversation::view::row_sources::RowSourceExport,
        SourceDialogResult,
    )> {
        let request = ctx.dialog(window, crate::ui::dialogs::save_file_spec("source.txt"));
        self.next = self.next.wrapping_add(1);
        let generation = self.next;
        let sender = self.picked.sender();
        routes.route(request, move |outcome| {
            let result = crate::ui::dialogs::chosen_path(outcome);
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

pub use crate::sources::SourceView;
