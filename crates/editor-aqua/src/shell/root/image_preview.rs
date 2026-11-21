use super::{ImagePreview, PreviewOrigin, RootViews};
use crate::core::Screen;
use crate::shell::host::Shell;
use aqua::prelude::AquaWindowId;
use aqua::{AppCtx, UiApp};

impl RootViews {
    pub(super) fn image_preview_stale(&self, shell: &Shell) -> bool {
        self.media_views
            .image_preview
            .as_ref()
            .is_some_and(|preview| match &preview.origin {
                PreviewOrigin::Conversation(scope) => {
                    !matches!(shell.core.screen(), Screen::Thread { .. })
                        || !shell.entry.workspace().is_some_and(|workspace| {
                            scope.current(&workspace.conversation, &shell.core)
                        })
                }
                PreviewOrigin::Composer => {
                    !matches!(
                        shell.core.screen(),
                        Screen::Thread { .. } | Screen::NewThread { .. }
                    ) || !preview.view.is_live(&shell.core)
                }
            })
    }

    pub(super) fn open_composer_image(&mut self, shell: &mut Shell, ui: &mut UiApp) -> bool {
        use crate::composer::view::ComposerEffect;
        let Some(workspace) = shell.entry.workspace_mut() else {
            return false;
        };
        let opened = workspace
            .take_composer_effects()
            .into_iter()
            .filter_map(|effect| match effect {
                ComposerEffect::OpenImage(key) => workspace.composer_image(key),
                ComposerEffect::EditorChanged
                | ComposerEffect::Stopping(..)
                | ComposerEffect::StopFailed(..)
                | ComposerEffect::FocusEditor
                | ComposerEffect::NewProject
                | ComposerEffect::OpenQuestionSource(_)
                | ComposerEffect::RetryQuestionPage
                | ComposerEffect::CloseNote
                | ComposerEffect::RetryThreadSettings => None,
            })
            .last();
        let Some((host, instance, image, opener_key)) = opened else {
            return false;
        };
        self.media_views.preview_generation = self.media_views.preview_generation.wrapping_add(1);
        let view = crate::ui::media::preview::Preview::from_ready(
            self.media_views.preview_generation,
            host,
            instance,
            image,
            opener_key,
            shell.core.clock().now(),
        );
        self.menus.dispose(ui);
        self.media_views.image_preview = Some(ImagePreview {
            origin: PreviewOrigin::Composer,
            owner: None,
            view,
            focus_pending: true,
        });
        true
    }

    pub(super) fn close_image_preview(&mut self, ui: &mut UiApp, restore_focus: bool) {
        if let Some(preview) = self.media_views.image_preview.take()
            && restore_focus
            && let Some(key) = preview.view.opener_key()
        {
            ui.focus_key(key.to_owned());
        }
        self.media_views.preview_actions.borrow_mut().clear();
    }

    pub(super) fn preview_root(ui: &UiApp) -> Option<aqua_core::NodeId> {
        let key = aqua_ui::tree::hash_key(aqua_ui::element::Key::from("image-preview"));
        ui.tree().sole_node_with_key(key)
    }

    pub(super) fn cycle_preview_focus(ui: &mut UiApp, backwards: bool) -> bool {
        let Some(root) = Self::preview_root(ui) else {
            return false;
        };
        let mut subtree = std::collections::HashSet::new();
        let mut pending = vec![root];
        while let Some(node) = pending.pop() {
            if subtree.insert(node)
                && let Some(record) = ui.tree().get(node)
            {
                pending.extend(record.children.iter().copied());
            }
        }
        let order: Vec<_> = aqua_ui::events::focusable_nodes(ui.tree())
            .into_iter()
            .filter(|node| subtree.contains(node))
            .collect();
        if order.is_empty() {
            return false;
        }
        let index = ui
            .tree()
            .focused()
            .and_then(|focused| order.iter().position(|node| *node == focused));
        let next = match (index, backwards) {
            (Some(0) | None, true) => order.len() - 1,
            (Some(index), true) => index - 1,
            (Some(index), false) => (index + 1) % order.len(),
            (None, false) => 0,
        };
        ui.tree_mut().set_focused(Some(order[next]))
    }

    pub(super) fn sync_image_preview(
        &mut self,
        shell: &Shell,
        ui: &mut UiApp,
        ctx: &mut AppCtx<'_>,
        window: AquaWindowId,
    ) -> bool {
        if self.image_preview_stale(shell) {
            self.close_image_preview(ui, false);
            return true;
        }
        let reduced = ui.host().motion().reduced_motion() == aqua_ui::motion::ReducedMotion::Reduce;
        let now = shell.core.clock().now();
        let events = std::mem::take(&mut *self.media_views.preview_actions.borrow_mut());
        let mut changed = false;
        for (generation, action) in events {
            let Some(preview) = &mut self.media_views.image_preview else {
                continue;
            };
            if generation != preview.view.generation() {
                continue;
            }
            if let crate::ui::media::preview::Action::CycleFocus(backwards) = action {
                changed |= Self::cycle_preview_focus(ui, backwards);
            } else {
                changed |= preview
                    .view
                    .dispatch(generation, action, &shell.core, now, reduced);
            }
        }
        let Some(preview) = &mut self.media_views.image_preview else {
            return changed;
        };
        ctx.bind_mailbox(window, preview.view.wake_mailbox());
        changed |= preview.view.refresh(&shell.core);
        let animating = preview.view.animating(now, reduced);
        preview.view.advance(now, reduced);
        if preview.view.finished(now, reduced) {
            self.close_image_preview(ui, true);
            return true;
        }
        if preview.focus_pending && ui.focus_key("image-preview") {
            preview.focus_pending = false;
            changed = true;
        }
        if animating {
            ctx.request_frame(window, aqua::prelude::FrameReason::ANIMATION);
        }
        changed || animating
    }
}
