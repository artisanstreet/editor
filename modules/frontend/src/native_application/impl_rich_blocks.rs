//! Host handlers for what rich blocks ask of the app.
//!
//! Blocks in a reply dispatch [`OpenReference`] when the reader clicks a
//! code path, and [`ShowCopyMenu`](artisan_ui::markdown_renderer::ShowCopyMenu)
//! on a right-click (`impl_pointer_menu.rs`; see `docs/rich-blocks.md`).
//! Without a handler the blocks fall back to copying. Relative media paths
//! resolve against the open project through [`MediaRoot`].

use artisan_ui::markdown_renderer::{MediaFetcher, MediaRoot, OpenReference};

use super::*;

impl NativeApplication {
    /// Points image and audio files at the open thread: fetched from the
    /// Forge's machine, where the agent that named them runs, with the
    /// project root kept for blocks shown before a thread opens.
    pub(super) fn sync_media_root(&self, cx: &mut Context<Self>) {
        let (root, thread) = match self.route() {
            NativeRoute::Thread { project, thread }
            | NativeRoute::Editor {
                project, thread, ..
            } => (
                self.project_navigation.roots.get(project).cloned(),
                Some(thread.clone()),
            ),
            _ => (None, None),
        };
        if cx.try_global::<MediaRoot>().map(|current| &current.0) != Some(&root) {
            MediaRoot::set(cx, root);
        }
        let current = cx
            .try_global::<MediaFetcher>()
            .map(|fetcher| fetcher.scope().to_string());
        if current.as_deref() != thread.as_ref().map(ThreadId::as_str) {
            let fetcher = thread
                .as_ref()
                .map(|thread| self.media_files.fetcher(thread));
            MediaFetcher::set(cx, fetcher);
        }
    }

    /// Opens the referenced file in the thread's editor surface. The route
    /// carries no line, so the file opens at its top.
    pub(super) fn open_rich_reference(
        &mut self,
        action: &OpenReference,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (project, thread) = match self.route() {
            NativeRoute::Thread { project, thread }
            | NativeRoute::Editor {
                project, thread, ..
            } => (project.clone(), thread.clone()),
            _ => return,
        };
        let file = action.path.trim_start_matches("./").to_owned();
        self.navigate(
            NativeRoute::Editor {
                project,
                thread,
                file: Some(file),
            },
            cx,
        );
    }
}
