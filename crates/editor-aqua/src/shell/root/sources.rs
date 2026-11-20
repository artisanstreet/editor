use super::{ImagePreview, PreviewOrigin, RootViews};
use crate::shell::host::Shell;
use aqua::prelude::AquaWindowId;
use aqua::{AppCtx, UiApp};

impl RootViews {
    pub(super) fn source_export_is_current(
        shell: &Shell,
        export: &crate::conversation::view::row_sources::RowSourceExport,
    ) -> bool {
        let Some(workspace) = shell.entry.workspace() else {
            return false;
        };
        export.scope().current(&workspace.conversation, &shell.core)
            && workspace
                .conversation
                .view
                .as_ref()
                .and_then(|view| view.item(export.item()))
                .is_some_and(|item| export.is_current(&shell.core, export.scope(), item))
    }

    pub(super) fn finish_source_dialog(
        shell: &Shell,
        export: crate::conversation::view::row_sources::RowSourceExport,
        result: Result<Option<std::path::PathBuf>, String>,
    ) -> bool {
        if !Self::source_export_is_current(shell, &export) {
            return false;
        }
        let Some(item) = shell
            .entry
            .workspace()
            .and_then(|workspace| workspace.conversation.view.as_ref())
            .and_then(|view| view.item(export.item()))
        else {
            return false;
        };
        let scope = export.scope().clone();
        export.finish(&shell.core, &scope, item, result);
        true
    }

    pub(super) fn source_effect(
        &mut self,
        shell: &Shell,
        ctx: &mut AppCtx<'_>,
        window: AquaWindowId,
        effect: crate::conversation::view::row_sources::RowSourceEffect,
    ) -> bool {
        use crate::conversation::view::row_sources::RowSourceEffect;
        match effect {
            RowSourceEffect::Clipboard(copied) => {
                copied.write(&shell.core, |text| ctx.clipboard().set_text(text))
                    == crate::sources::Copied::Failed
            }
            RowSourceEffect::SavePath(export) => {
                if !Self::source_export_is_current(shell, &export) {
                    return false;
                }
                self.source_dialogs
                    .open(*export, ctx, &shell.dialogs, window);
                false
            }
        }
    }

    pub(super) fn bind_row_sources(&self, ui: &mut UiApp, ctx: &AppCtx<'_>, window: AquaWindowId) {
        let _tm = crate::shell::application::Tm::new("v.bind_sources");
        self.stored.borrow().bind(ctx, window);
        self.stored.borrow_mut().drain_cleanup(ui);
        let owners = self.row_cache.borrow().live_owners();
        for owner in owners.into_iter().filter_map(|owner| owner.upgrade()) {
            owner.bind_sources(ctx, window);
        }
        self.row_cache.borrow().drain_cleanup(ui);
        self.source_dialogs.bind(ctx, window);
    }

    pub(super) fn drain_row_sources(
        &mut self,
        shell: &Shell,
        ctx: &mut AppCtx<'_>,
        window: AquaWindowId,
    ) -> bool {
        let Some(workspace) = shell.entry.workspace() else {
            return false;
        };
        let Some(scope) = crate::conversation::presentation::PresentationScope::capture(
            &workspace.conversation,
            &shell.core,
        ) else {
            return false;
        };
        let Some(provider) = shell.core.source_provider(&scope.host, scope.instance) else {
            return false;
        };
        let Some(view) = &workspace.conversation.view else {
            return false;
        };
        let owners = self.row_cache.borrow().live_owners();
        let mut changed = false;
        let mut effects = Vec::new();
        for owner in owners.into_iter().filter_map(|owner| owner.upgrade()) {
            if owner
                .identity()
                .is_none_or(|(_, current)| current != &scope)
            {
                continue;
            }
            let (source_changed, source_effects) =
                owner.drain_sources(&shell.core, &scope, &provider, |id| view.shared_item(id));
            changed |= source_changed;
            effects.extend(source_effects);
        }
        for effect in effects {
            changed |= self.source_effect(shell, ctx, window, effect);
        }
        changed
    }

    pub(super) fn dispatch_blob(
        &mut self,
        shell: &mut Shell,
        ui: &mut UiApp,
        scope: crate::conversation::presentation::PresentationScope,
        id: &artisan_api::ItemId,
        revision: u64,
        action: crate::conversation::view::factory::BlobAction,
    ) -> bool {
        use crate::conversation::view::factory::BlobAction;
        let Some(workspace) = shell.entry.workspace() else {
            return false;
        };
        if !scope.current(&workspace.conversation, &shell.core) {
            return false;
        }
        let hash = match &action {
            BlobAction::Load { hash, .. } | BlobAction::Open { hash, .. } => hash,
        };
        let current = workspace
            .conversation
            .view
            .as_ref()
            .and_then(|view| view.item(id))
            .is_some_and(|item| {
                item.seq == revision
                    && (matches!(&item.kind, artisan_api::ItemKind::UserMessage { images, .. } if images.contains(hash))
                        || matches!(&item.kind, artisan_api::ItemKind::Image { blob, .. } if blob == hash))
            });
        if !current {
            return false;
        }
        match action {
            BlobAction::Load { hash, retry } => {
                let Some(host) = shell.core.host_mut_if_instance(&scope.host, scope.instance)
                else {
                    return false;
                };
                let before = host.image(&hash);
                host.load_image(hash.clone(), retry);
                before.as_ref().map(std::mem::discriminant)
                    != host.image(&hash).as_ref().map(std::mem::discriminant)
            }
            BlobAction::Open {
                image, opener_key, ..
            } => {
                self.preview_generation = self.preview_generation.wrapping_add(1);
                let view = crate::ui::media::preview::Preview::from_ready(
                    self.preview_generation,
                    scope.host.clone(),
                    scope.instance,
                    image,
                    opener_key,
                    shell.core.clock().now(),
                );
                self.dispose_color_menu(ui);
                self.image_preview = Some(ImagePreview {
                    origin: PreviewOrigin::Conversation(scope),
                    owner: None,
                    view,
                    focus_pending: true,
                });
                true
            }
        }
    }
}
