use super::{ConversationAction, ImagePreview, PreviewOrigin, RootViews};
use crate::shell::host::Shell;
use crate::shell::refresh::Cause;
use aqua::prelude::AquaWindowId;
use aqua::{AppCtx, UiApp};
use std::rc::Rc;

impl RootViews {
    #[expect(
        clippy::too_many_lines,
        reason = "one dispatch loop with an arm per conversation action"
    )]
    pub(super) fn dispatch_conversation(
        &mut self,
        shell: &mut Shell,
        ui: &mut UiApp,
        ctx: &mut AppCtx<'_>,
        window: AquaWindowId,
    ) -> bool {
        use crate::ui::color_copy_menu::ColorCopyEffect;
        use crate::ui::markdown::visual::LinkAction;
        let _tm = crate::shell::application::Tm::new("v.dispatch_conv");
        self.connection.dispatch(&mut shell.core);
        let invalidated = self.conversation_invalidated.replace(false);
        let mut changed = self.noted.note(Cause::ConversationInvalidated, invalidated);
        if self.navigation_invalidated.replace(false) {
            let navigated = self.refresh_navigation(shell, ui, ctx, window);
            changed |= self.noted.note(Cause::Navigation, navigated);
        }
        self.bind_row_sources(ui, ctx, window);
        if let Some(workspace) = shell.entry.workspace_mut() {
            let published = self
                .stored
                .borrow_mut()
                .drain_publish(&mut workspace.conversation, &shell.core);
            changed |= self.noted.note(Cause::StoredRows, published);
        }
        self.stored.borrow_mut().drain_cleanup(ui);
        let sourced = self.drain_row_sources(shell, ctx, window);
        changed |= self.noted.note(Cause::RowSources, sourced);
        self.source_dialogs
            .retain(|export| Self::source_export_is_current(shell, export));
        for (export, result) in self.source_dialogs.drain() {
            changed |= Self::finish_source_dialog(shell, export, result);
        }
        let row_actions = std::mem::take(&mut *self.row_actions.borrow_mut());
        if let Some(workspace) = shell.entry.workspace_mut() {
            let reduced =
                ui.host().motion().reduced_motion() == aqua_ui::motion::ReducedMotion::Reduce;
            for action in row_actions {
                changed |= action.dispatch(
                    &mut workspace.conversation,
                    &mut self.stored.borrow_mut(),
                    &shell.core,
                    reduced,
                    shell.core.clock().now(),
                );
            }
        }
        let actions = std::mem::take(&mut *self.conversation_actions.borrow_mut());
        for action in actions {
            let scope = match &action {
                ConversationAction::Source(action) => &action.scope,
                ConversationAction::Link(scope, _)
                | ConversationAction::Color(scope, _)
                | ConversationAction::Copy(scope, _)
                | ConversationAction::Media(scope, _, _, _)
                | ConversationAction::Blob(scope, _, _, _)
                | ConversationAction::RetryControls(scope)
                | ConversationAction::RetryThread(scope) => scope,
            };
            if !shell
                .entry
                .workspace()
                .is_some_and(|workspace| scope.current(&workspace.conversation, &shell.core))
            {
                continue;
            }
            let action = match action {
                ConversationAction::Source(action) => {
                    let Some(item) = shell
                        .entry
                        .workspace()
                        .and_then(|workspace| workspace.conversation.view.as_ref())
                        .and_then(|view| view.item(&action.item))
                    else {
                        continue;
                    };
                    let scope = action.scope.clone();
                    let (source_changed, effect) = action.dispatch(&shell.core, &scope, item);
                    changed |= source_changed;
                    if let Some(effect) = effect {
                        changed |= self.source_effect(shell, ctx, window, effect);
                    }
                    continue;
                }
                ConversationAction::Media(scope, key, revision, effect) => {
                    let Some(workspace) = shell.entry.workspace() else {
                        continue;
                    };
                    if workspace
                        .conversation
                        .view
                        .as_ref()
                        .and_then(|view| view.item(&key.item))
                        .is_none_or(|item| item.seq != revision)
                    {
                        continue;
                    }
                    match effect {
                        crate::ui::media::preview::Effect::Open(request) => {
                            let owner = std::sync::Arc::clone(request.fetcher.owner());
                            self.preview_generation = self.preview_generation.wrapping_add(1);
                            if let Some(view) = crate::ui::media::preview::Preview::from_request(
                                self.preview_generation,
                                scope.host.clone(),
                                scope.instance,
                                workspace.images.as_ref().clone(),
                                request,
                                shell.core.clock().now(),
                            ) {
                                self.dispose_color_menu(ui);
                                self.image_preview = Some(ImagePreview {
                                    origin: PreviewOrigin::Conversation(scope),
                                    owner: Some(owner),
                                    view,
                                    focus_pending: true,
                                });
                                changed = true;
                            }
                            continue;
                        }
                        crate::ui::media::preview::Effect::ShowCopyMenu(request) => {
                            ConversationAction::Color(scope, request)
                        }
                        crate::ui::media::preview::Effect::Clipboard(text) => {
                            if let Err(error) = ctx.clipboard().set_text(text) {
                                tracing::warn!(%error, "could not copy media path");
                            }
                            continue;
                        }
                    }
                }
                action @ (ConversationAction::Link(..)
                | ConversationAction::Color(..)
                | ConversationAction::Copy(..)
                | ConversationAction::RetryControls(_)
                | ConversationAction::RetryThread(_)
                | ConversationAction::Blob(..)) => action,
            };
            match action {
                ConversationAction::Media(_, _, _, _) | ConversationAction::Source(_) => {}
                ConversationAction::Blob(scope, item, revision, action) => {
                    changed |= self.dispatch_blob(shell, ui, scope, &item, revision, action);
                }
                ConversationAction::RetryControls(scope) => {
                    if let Some(host) = shell.core.host_mut_if_instance(&scope.host, scope.instance)
                    {
                        host.load_thread_summary(&scope.thread, true);
                        changed = true;
                    }
                }
                ConversationAction::RetryThread(scope) => {
                    if let Some(host) = shell.core.host_mut_if_instance(&scope.host, scope.instance)
                    {
                        host.ensure_thread(&scope.thread);
                        changed = true;
                    }
                }
                ConversationAction::Copy(_, source) => {
                    if let Err(error) = ctx.clipboard().set_text(source.into_text()) {
                        tracing::warn!(%error, "could not copy full content");
                    }
                }
                ConversationAction::Link(_, LinkAction::OpenReference(reference)) => {
                    let scope = reference
                        .scope
                        .map(|scope| (crate::core::HostId(scope.host), scope.thread));
                    shell.open_reference(ctx, window, reference.path, reference.line, scope);
                    changed = true;
                }
                ConversationAction::Link(_, LinkAction::OpenUrl(url)) => {
                    if let Err(error) = crate::shell::open_url(&url) {
                        tracing::warn!(
                            error = %::diagnostics::chain(&error),
                            "could not open URL"
                        );
                        self.link_failure.show(url, &error);
                        changed = true;
                    }
                }
                ConversationAction::Color(scope, request) => {
                    self.dispose_color_menu(ui);
                    if let Some(owner) = ui.find_debug_selector("workspace-root") {
                        let effects = Rc::clone(&self.color_effects);
                        let reduced = ui.host().motion().reduced_motion()
                            == aqua_ui::motion::ReducedMotion::Reduce;
                        match crate::ui::color_copy_menu::ColorCopyMenuHost::open(
                            ui,
                            owner,
                            request,
                            None,
                            shell.preferences.theme(),
                            reduced,
                            ctx.now().as_secs_f64(),
                            move |effect| effects.borrow_mut().push_back(effect),
                        ) {
                            Ok(menu) => {
                                self.color_scope = menu.as_ref().map(|_| scope);
                                self.color_menu = menu;
                            }
                            Err(error) => tracing::warn!(%error, "could not open color copy menu"),
                        }
                    }
                }
            }
        }
        let previewed = self.sync_image_preview(shell, ui, ctx, window);
        changed |= self.noted.note(Cause::ImagePreview, previewed);
        self.warm_browser(shell);
        changed |= self.open_selection_menu(shell, ui, ctx);
        let effects = std::mem::take(&mut *self.color_effects.borrow_mut());
        for effect in effects {
            match effect {
                ColorCopyEffect::Clipboard(text) => {
                    if let Err(error) = ctx.clipboard().set_text(text) {
                        tracing::warn!(%error, "could not copy color");
                    }
                }
                ColorCopyEffect::Search(text) => {
                    let browser = self
                        .browser
                        .as_ref()
                        .and_then(|resolved| resolved.browser.as_ref());
                    if let Some(browser) = browser
                        && let Err(error) = browser.search(&text)
                    {
                        tracing::warn!(
                            %error,
                            browser = %browser.name,
                            program = %browser.program.display(),
                            "could not start a browser search"
                        );
                    }
                }
                ColorCopyEffect::RequestFrame => {
                    ctx.request_frame(window, aqua::prelude::FrameReason::ANIMATION);
                }
                ColorCopyEffect::Dismissed => {
                    self.color_menu = None;
                    self.color_scope = None;
                }
            }
        }
        changed
    }
}
