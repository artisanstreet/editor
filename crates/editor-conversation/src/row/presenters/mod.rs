//! Stateless canonical Item presenters and their typed Conversation actions.
//! These modules borrow current owners; none retains a row model or starts a cache.
pub(super) mod agent;
pub(super) mod media;
pub(super) mod request;
pub(super) mod text;
pub(super) mod tool;

use crate::{Conversation, row::Shared, viewport::Input};
use artisan_api::{Answer, BlobHash, ItemId, RunId};
use editor_ui::{
    button::{Button, ButtonVariant},
    theme::ArtisanTheme,
};
use gpui::{AnyElement, App, IntoElement, SharedString, WeakEntity};

pub(super) enum Action {
    OpenAgent(ItemId),
    RetryImage(BlobHash),
    LoadImage(BlobHash),
    ToggleWork { run: RunId, open: bool },
    ToggleChain { id: ItemId, open: bool },
    ToggleItem { id: ItemId, expanded: bool },
    Answer { id: ItemId, answer: Answer },
    LoadNewer,
}
#[derive(Clone)]
pub(super) struct Actions {
    target: WeakEntity<Conversation>,
    /// Immutable selection identity established by Conversation::update_selection_scope.
    /// Its host/thread/agent key is copied while building the presenter, before callbacks escape.
    origin: SharedString,
}
impl Actions {
    pub(super) fn new(shared: &Shared) -> Self {
        // Read the already-established shared identity rather than borrowing the
        // parent Conversation during RowView render.
        Self {
            target: shared.conversation.clone(),
            origin: shared.selection_scope.borrow().clone(),
        }
    }
    fn dispatch(&self, action: &Action, cx: &mut App) {
        let _ = self.target.update(cx, |this, cx| {
            // A closed Conversation retains the last selection key. Check open
            // state as well, so old actions cannot run after the scope closes.
            if this.open.is_none()
                || self.origin.is_empty()
                || *this.shared.selection_scope.borrow() != self.origin
            {
                return;
            }
            match action {
                Action::OpenAgent(id) => this.open_agent(Some(id.clone()), cx),
                Action::RetryImage(hash) => this.retry_image(hash, cx),
                Action::LoadImage(hash) => this.load_image(hash, cx),
                Action::ToggleWork { run, open } => this.toggle_work(run, *open, cx),
                Action::ToggleChain { id, open } => this.toggle_chain(id, *open, cx),
                Action::ToggleItem { id, expanded } => this.toggle_item(id, *expanded, cx),
                Action::Answer { id, answer } => this.answer(id.clone(), answer.clone(), cx),
                Action::LoadNewer => this.viewport(vec![Input::BottomReached], cx),
            }
        });
    }
    pub(super) fn handler(&self, action: Action) -> impl Fn(&mut App) + 'static + use<> {
        let actions = self.clone();
        move |cx| actions.dispatch(&action, cx)
    }
    pub(super) fn button(
        &self,
        theme: ArtisanTheme,
        id: impl Into<SharedString>,
        label: &str,
        variant: ButtonVariant,
        disabled: bool,
        action: Action,
    ) -> AnyElement {
        let activate = self.handler(action);
        let id: SharedString = id.into();
        Button::text(id, theme, label.to_owned())
            .variant(variant)
            .disabled(disabled)
            .on_activate(move |_, _, cx| activate(cx))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thread::ThreadView;
    use artisan_api::ThreadId;
    use editor_core::{Core, EditorFiles, HostId};
    use gpui::{AppContext as _, TestAppContext};

    #[gpui::test]
    fn retained_actions_reject_new_host_thread_and_agent_scopes_but_current_actions_dispatch(
        cx: &mut TestAppContext,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let core = cx.new(|cx| {
            Core::new(
                EditorFiles {
                    dir: dir.path().to_owned(),
                },
                cx,
            )
            .unwrap()
        });
        let (conversation, cx) =
            cx.add_window_view(|window, cx| Conversation::new(core, window, cx));
        let host = HostId("origin-host".to_owned());
        let thread = ThreadId::new();
        let reused_id = ItemId::new();
        conversation.update(cx, |this, _| {
            this.open = Some((host.clone(), thread.clone()));
            this.view = Some(ThreadView::open(thread.clone()).0);
            this.update_selection_scope();
        });
        let old = conversation.read_with(cx, |this, _| {
            Actions::new(&this.shared).handler(Action::ToggleItem {
                id: reused_id.clone(),
                expanded: true,
            })
        });
        cx.update(|_, cx| old(cx));
        conversation.read_with(cx, |this, _| {
            assert_eq!(
                this.view.as_ref().unwrap().ui[&reused_id].expanded,
                Some(true)
            )
        });
        for (current_host, current_thread, agent) in [
            (HostId("replacement-host".to_owned()), thread.clone(), None),
            (host.clone(), ThreadId::new(), None),
            (host.clone(), thread.clone(), Some(ItemId::new())),
        ] {
            conversation.update(cx, |this, _| {
                this.open = Some((current_host.clone(), current_thread.clone()));
                this.agent = agent;
                this.view = Some(ThreadView::open(current_thread).0);
                this.update_selection_scope();
            });
            cx.update(|_, cx| old(cx));
            conversation.read_with(cx, |this, _| {
                assert!(!this.view.as_ref().unwrap().ui.contains_key(&reused_id))
            });
            let current = conversation.read_with(cx, |this, _| {
                Actions::new(&this.shared).handler(Action::ToggleItem {
                    id: reused_id.clone(),
                    expanded: true,
                })
            });
            cx.update(|_, cx| current(cx));
            conversation.read_with(cx, |this, _| {
                assert_eq!(
                    this.view.as_ref().unwrap().ui[&reused_id].expanded,
                    Some(true)
                )
            });
        }
        let before_close = conversation.read_with(cx, |this, _| {
            Actions::new(&this.shared).handler(Action::ToggleItem {
                id: reused_id.clone(),
                expanded: true,
            })
        });
        conversation.update(cx, |this, _| {
            this.open = None;
            this.view.as_mut().unwrap().ui.clear();
        });
        cx.update(|_, cx| before_close(cx));
        conversation.read_with(cx, |this, _| {
            assert!(!this.view.as_ref().unwrap().ui.contains_key(&reused_id))
        });
    }
}
