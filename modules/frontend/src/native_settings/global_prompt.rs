//! The global prompt page: the editor, its saved base, and the decisions the
//! Forge asks the user for.
//!
//! The editor never loses the user's typing to a push: while it holds
//! unsaved edits, a newer version from the Forge is announced instead of
//! loaded, and the user decides whether to load it. A save names the
//! revision the edit started from, so the Forge refuses one that would
//! silently replace another edit.

use artisan_domain::{GlobalPrompt, GlobalPromptResolution, GlobalPromptRevision};
use gpui::{AppContext, Context, Entity, Subscription};

use super::{SettingsScreen, SettingsScreenEvent};
use crate::native_text_editor::{NativeTextEditor, NativeTextEditorEvent};

/// Placeholder shown in an empty editor.
const PLACEHOLDER: &str = "Write instructions every engine should follow, in Markdown. For example: how you like code reviewed, which tools to prefer, or how to phrase answers.";

/// The mounted global prompt page.
pub(super) struct GlobalPromptPage {
    pub(super) editor: Entity<NativeTextEditor>,
    _changes: Subscription,
    /// The Forge's prompt, once read or pushed.
    pub(super) prompt: Option<GlobalPrompt>,
    /// The revision and text the editor's content started from.
    pub(super) base_revision: GlobalPromptRevision,
    pub(super) base_text: String,
    /// Whether the editor holds the Forge's text yet.
    pub(super) loaded: bool,
    /// Whether a save or resolution is in flight.
    pub(super) pending: bool,
    /// The last refused change.
    pub(super) notice: Option<String>,
    /// A newer version arrived while the editor held unsaved edits.
    pub(super) newer: bool,
}

impl GlobalPromptPage {
    /// Mounts an empty, read-only editor that waits for the Forge.
    pub(super) fn new(cx: &mut Context<SettingsScreen>) -> Self {
        let editor = cx.new(|editor_cx| {
            let mut editor = NativeTextEditor::new("", PLACEHOLDER, editor_cx);
            editor.set_disabled(true, editor_cx);
            editor
        });
        let changes = cx.subscribe(&editor, |_, _, event: &NativeTextEditorEvent, cx| {
            if matches!(event, NativeTextEditorEvent::Changed) {
                cx.notify();
            }
        });
        Self {
            editor,
            _changes: changes,
            prompt: None,
            base_revision: GlobalPromptRevision::default(),
            base_text: String::new(),
            loaded: false,
            pending: false,
            notice: None,
            newer: false,
        }
    }

    /// Whether the editor differs from the version it started from.
    pub(super) fn dirty(&self, cx: &gpui::App) -> bool {
        self.loaded && self.editor.read(cx).text() != self.base_text
    }

    fn load(&mut self, prompt: &GlobalPrompt, cx: &mut Context<SettingsScreen>) {
        let text = prompt.text().to_owned();
        self.editor.update(cx, |editor, editor_cx| {
            if editor.text() != text {
                editor.set_text(&text, editor_cx);
            }
            editor.set_disabled(false, editor_cx);
        });
        self.base_revision = prompt.revision();
        self.base_text = text;
        self.loaded = true;
        self.newer = false;
    }
}

impl SettingsScreen {
    /// Applies the Forge's global prompt and the page's request state.
    pub fn set_global_prompt(
        &mut self,
        prompt: Option<GlobalPrompt>,
        pending: bool,
        notice: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let Some(mut page) = self.global_prompt.take() else {
            return;
        };
        page.pending = pending;
        page.notice = notice;
        if let Some(prompt) = prompt {
            let editor_text = page.editor.read(cx).text().to_owned();
            let dirty = page.loaded && editor_text != page.base_text;
            if !dirty || editor_text == prompt.text() {
                page.load(&prompt, cx);
            } else if prompt.revision() != page.base_revision {
                page.newer = true;
            }
            page.prompt = Some(prompt);
        }
        self.global_prompt = Some(page);
        cx.notify();
    }

    /// Saves the editor's text over the revision it started from.
    pub(super) fn save_global_prompt(&mut self, cx: &mut Context<Self>) {
        let Some(page) = &self.global_prompt else {
            return;
        };
        if page.pending || page.newer || !page.dirty(cx) {
            return;
        }
        cx.emit(SettingsScreenEvent::SaveGlobalPrompt {
            base_revision: page.base_revision,
            text: page.editor.read(cx).text().to_owned(),
        });
    }

    /// Discards unsaved edits.
    pub(super) fn revert_global_prompt(&mut self, cx: &mut Context<Self>) {
        let Some(page) = &self.global_prompt else {
            return;
        };
        let text = page.base_text.clone();
        page.editor
            .update(cx, |editor, editor_cx| editor.set_text(&text, editor_cx));
        cx.notify();
    }

    /// Replaces unsaved edits with the newest version from the Forge.
    pub(crate) fn load_latest_global_prompt(&mut self, cx: &mut Context<Self>) {
        let Some(mut page) = self.global_prompt.take() else {
            return;
        };
        if let Some(prompt) = page.prompt.clone() {
            page.load(&prompt, cx);
        }
        self.global_prompt = Some(page);
        cx.notify();
    }

    /// Asks the Forge to settle setup or a drifted engine.
    pub(super) fn resolve_global_prompt(
        &mut self,
        resolution: GlobalPromptResolution,
        cx: &mut Context<Self>,
    ) {
        if self
            .global_prompt
            .as_ref()
            .is_some_and(|page| !page.pending)
        {
            cx.emit(SettingsScreenEvent::ResolveGlobalPrompt(resolution));
        }
    }
}

#[cfg(test)]
impl SettingsScreen {
    /// The editor's text and whether a newer version waits, for tests.
    pub(crate) fn global_prompt_view(&self, cx: &gpui::App) -> Option<(String, bool, bool)> {
        self.global_prompt.as_ref().map(|page| {
            (
                page.editor.read(cx).text().to_owned(),
                page.dirty(cx),
                page.newer,
            )
        })
    }

    /// Replaces the editor's text as if the user typed it, for tests.
    pub(crate) fn edit_global_prompt_for_test(&mut self, text: &str, cx: &mut Context<Self>) {
        if let Some(page) = &self.global_prompt {
            page.editor
                .update(cx, |editor, editor_cx| editor.set_text(text, editor_cx));
        }
    }

    /// Presses Save, for tests.
    pub(crate) fn press_save_global_prompt(&mut self, cx: &mut Context<Self>) {
        self.save_global_prompt(cx);
    }
}
