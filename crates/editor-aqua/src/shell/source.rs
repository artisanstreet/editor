use aqua::AppCtx;
use aqua::prelude::AquaWindowId;

type SourceDialogResult = Result<Option<std::path::PathBuf>, String>;

pub struct RowSourceDialogs {
    next: u64,
    pending:
        std::collections::HashMap<u64, crate::conversation::view::row_sources::RowSourceExport>,
    picked: crate::core::delivery::Delivery<(u64, SourceDialogResult)>,
}

impl std::fmt::Debug for RowSourceDialogs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RowSourceDialogs").finish_non_exhaustive()
    }
}

impl Default for RowSourceDialogs {
    fn default() -> Self {
        Self {
            next: 0,
            pending: std::collections::HashMap::default(),
            picked: crate::core::delivery::Delivery::new(std::num::NonZeroUsize::MIN),
        }
    }
}

impl RowSourceDialogs {
    pub(crate) fn bind(&self, ctx: &AppCtx<'_>, window: AquaWindowId) {
        if let Some(mailbox) = self.picked.wake_mailbox() {
            ctx.bind_mailbox(window, mailbox);
        }
    }

    pub(crate) fn retain(
        &mut self,
        mut current: impl FnMut(&crate::conversation::view::row_sources::RowSourceExport) -> bool,
    ) {
        self.pending.retain(|_, export| current(export));
    }

    pub(crate) fn open(
        &mut self,
        export: crate::conversation::view::row_sources::RowSourceExport,
        ctx: &mut AppCtx<'_>,
        routes: &crate::ui::dialogs::DialogRoutes,
        window: AquaWindowId,
    ) {
        let request = ctx.dialog(window, crate::ui::dialogs::save_file_spec("source.txt"));
        self.next = self.next.wrapping_add(1);
        let generation = self.next;
        let sender = self.picked.sender();
        routes.route(request, move |outcome| {
            let result = crate::ui::dialogs::chosen_path(outcome);
            crate::core::runtime::runtime().spawn(async move {
                if sender.send((generation, result)).await.is_err() {
                    tracing::debug!(
                        generation,
                        "the source export closed before a path was chosen"
                    );
                }
            });
        });
        self.pending.insert(generation, export);
    }

    pub(crate) fn drain(
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
