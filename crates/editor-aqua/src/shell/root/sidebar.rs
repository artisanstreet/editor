use super::{RootViews, reveal};
use crate::shell::host::Shell;
use aqua::prelude::AquaWindowId;
use aqua::{AppCtx, UiApp};
use aqua_ui::element::Element;
use std::rc::Rc;

impl RootViews {
    pub(super) fn release_sidebar_views(&mut self, ui: &mut UiApp) {
        self.sidebar_overlays.dispose(ui);
        if let Some(preview) = self.sidebar_drag.take() {
            preview.dispose(ui);
        }
        self.drag_position = None;
        self.navigation.dispose();
        self.transcript_viewport = None;
        self.mounted_reveals.borrow_mut().rebind(None);
        self.row_cache.borrow_mut().rebind(None);
    }

    pub(super) fn release_workspace_views(&mut self, ui: &mut UiApp) {
        self.replace_rows(None);
        self.stored.borrow_mut().drain_cleanup(ui);
        self.composer_height = None;
        self.composer_measurement.set(None);
        self.release_sidebar_views(ui);
        self.context_reading = None;
        self.observations.clear();
        drop(self.media.take());
        self.dispose_color_menu(ui);
        if let Some(dialog) = self.settings.take() {
            dialog.dispose(ui);
        }
        self.close_palette(ui);
        self.close_resource(ui);
    }

    pub(super) fn sync_sidebar(
        &mut self,
        shell: &mut Shell,
        ui: &mut UiApp,
        ctx: &mut AppCtx<'_>,
        window: AquaWindowId,
    ) -> bool {
        let _tm = crate::shell::application::Tm::new("v.sidebar");
        let Some(workspace) = shell.entry.workspace_mut() else {
            self.release_sidebar_views(ui);
            return false;
        };
        let now = ctx.now().as_secs_f64();
        let mut changed = false;
        changed |= workspace.dispatch_sidebar_actions(&mut shell.core);
        let Some(owner) = ui.find_debug_selector("workspace-root") else {
            return changed;
        };
        let snapshot = ui.drag_snapshot();
        let instance = snapshot.as_ref().and_then(|snapshot| {
            let payload = snapshot.payload.get::<crate::core::DraggedThread>()?;
            workspace.sidebar.drag_instance(&shell.core, payload)
        });
        let mut preview_changed = false;
        for effect in workspace.sidebar.drain_effects() {
            match effect {
                crate::sidebar::controller::SidebarEffect::ShowDragPreview { payload, .. } => {
                    if let Some(preview) = self.sidebar_drag.take() {
                        preview.dispose(ui);
                    }
                    if let Some((snapshot, instance)) =
                        snapshot.as_ref().zip(instance).filter(|(snapshot, _)| {
                            snapshot.payload.get::<crate::core::DraggedThread>() == Some(&payload)
                        })
                    {
                        match crate::sidebar::drag_preview::DragPreviewHost::open(
                            ui,
                            owner,
                            instance,
                            snapshot,
                            shell.preferences.theme(),
                        ) {
                            Ok(preview) => self.sidebar_drag = preview,
                            Err(error) => tracing::warn!(%error, "could not open drag preview"),
                        }
                    }
                    preview_changed = true;
                }
                crate::sidebar::controller::SidebarEffect::HideDragPreview => {
                    if let Some(preview) = self.sidebar_drag.take() {
                        preview.dispose(ui);
                        preview_changed = true;
                    }
                }
                crate::sidebar::controller::SidebarEffect::Reveal(hit) => {
                    self.reveal = Some(reveal::Reveal::new(hit));
                    ctx.request_frame(window, aqua::prelude::FrameReason::ANIMATION);
                }
            }
        }
        if let Some(preview) = &mut self.sidebar_drag {
            match preview.sync(ui, instance, snapshot.as_ref()) {
                Ok(true) => {}
                Ok(false) => {
                    self.sidebar_drag = None;
                    preview_changed = true;
                }
                Err(error) => {
                    tracing::warn!(%error, "could not move drag preview");
                    if let Some(preview) = self.sidebar_drag.take() {
                        preview.dispose(ui);
                    }
                    preview_changed = true;
                }
            }
        }
        let position = snapshot.as_ref().map(|snapshot| snapshot.pointer.position);
        if preview_changed || self.sidebar_drag.is_some() && self.drag_position != position {
            ctx.request_frame(window, aqua::prelude::FrameReason::INPUT);
        }
        self.drag_position = position;
        if let Err(error) = self.sidebar_overlays.sync(
            ui,
            owner,
            &workspace.sidebar,
            &shell.core,
            shell.preferences.theme(),
            now,
            workspace.sidebar_emit(false),
        ) {
            tracing::warn!(%error, "could not synchronize sidebar overlays");
        }
        match self.sidebar_overlays.advance(ui, now) {
            Ok(true) => ctx.request_frame(window, aqua::prelude::FrameReason::ANIMATION),
            Ok(false) => {}
            Err(error) => tracing::warn!(%error, "could not advance sidebar overlays"),
        }
        if let Some(editor) = self.sidebar_overlays.rename_editor() {
            if let Some(key) = editor.text_key() {
                let key = aqua_ui::tree::hash_key(key);
                let nodes: Vec<_> = ui
                    .tree()
                    .traverse()
                    .into_iter()
                    .filter(|node| {
                        ui.tree()
                            .get(*node)
                            .is_some_and(|record| record.key == Some(key))
                    })
                    .collect();
                if let [node] = nodes.as_slice() {
                    if editor.sync(ui.tree_mut(), *node) {
                        ctx.request_frame(window, aqua::prelude::FrameReason::INPUT);
                    }
                }
            }
        }
        changed
    }

    pub(super) fn schedule_sidebar(&self, ctx: &mut AppCtx<'_>, window: AquaWindowId) {
        if self.sidebar.animating() {
            ctx.request_frame(window, aqua::prelude::FrameReason::ANIMATION);
        }
        if let Some(deadline) = self.sidebar.deadline() {
            let wait = deadline.saturating_duration_since(std::time::Instant::now());
            ctx.request_animation_deadline(window, ctx.now() + wait);
        }
    }

    pub(super) fn tree_emit(
        &self,
        workspace: &crate::shell::workspace::Workspace,
    ) -> Rc<dyn Fn(crate::sidebar::view::ViewAction)> {
        use crate::sidebar::view::ViewAction;
        let emit = workspace.sidebar_emit(false);
        if !self.sidebar.searching() {
            return emit;
        }
        let close = self.sidebar.emit();
        Rc::new(move |action| {
            if matches!(
                action,
                ViewAction::OpenThread { .. }
                    | ViewAction::OpenHit { .. }
                    | ViewAction::NewThread(_)
                    | ViewAction::Navigate(_)
            ) {
                close(crate::frame::sidebar::Event::Search(false));
            }
            emit(action);
        })
    }

    pub(super) fn search_parts(
        &self,
        workspace: &crate::shell::workspace::Workspace,
        theme: &crate::ui::theme::ArtisanTheme,
        tree: Option<Element>,
        now: std::time::Instant,
    ) -> (Option<Element>, Option<Element>, Element) {
        let search = tree.is_some().then(|| self.search_bar(workspace, theme));
        let ((search, tree), (bar, results)) = match self.sidebar.searching() {
            true => ((None, None), (search, tree)),
            false => ((search, tree), (None, None)),
        };
        let overlay = crate::frame::search::overlay(
            theme,
            crate::frame::search::Overlay {
                progress: self.sidebar.search(now),
                anchor: crate::frame::view::search_anchor(),
                bar,
                results,
                emit: self.sidebar.emit(),
            },
        );
        (search, tree, overlay)
    }

    pub(super) fn search_bar(
        &self,
        workspace: &crate::shell::workspace::Workspace,
        theme: &crate::ui::theme::ArtisanTheme,
    ) -> Element {
        crate::frame::search::bar(
            theme,
            crate::sidebar::view::search_editor(
                &workspace.sidebar,
                theme,
                workspace.sidebar_emit(false),
            ),
            self.sidebar.emit(),
        )
    }

    pub(super) fn sidebar_model(
        &self,
        theme: &crate::ui::theme::ArtisanTheme,
        search: Option<Element>,
        tree: Option<Element>,
        account: Option<&crate::ui::chrome::account::Account>,
        now: std::time::Instant,
    ) -> crate::frame::view::Sidebar {
        crate::frame::view::Sidebar {
            search,
            width: self.sidebar.width(now),
            open: self.sidebar.open(),
            wordmark: self.sidebar.wordmark(now),
            emit: self.sidebar.emit(),
            footer: account.map(|account| {
                let emit = self.sidebar.emit();
                crate::ui::chrome::account::trigger(
                    &theme,
                    crate::ui::chrome::account::Trigger {
                        account,
                        menu: self.sidebar.menu(),
                        now,
                        emit: Rc::new(move |event| {
                            emit(crate::frame::sidebar::Event::Account(event))
                        }),
                    },
                )
            }),
            tree,
        }
    }
}
