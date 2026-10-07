//! A reusable multiline plain-text editor for long documents.
//!
//! GPUI has no editable multiline widget, and the message composer's editor
//! is bound to message-sending state. This entity adapts the composer's text
//! machinery (UTF-8 selection with an anchor, IME marked text through
//! [`gpui::EntityInputHandler`], soft-wrapped vertical movement with a goal
//! x, snapshot undo/redo) into a standalone editor with no send semantics:
//! Enter inserts a newline and Tab inserts two spaces.
//!
//! The document is bounded to [`NativeTextEditor::MAX_BYTES`]; input that
//! would exceed it is truncated at a character boundary, or refused when
//! nothing fits. Every user edit emits [`NativeTextEditorEvent::Changed`];
//! programmatic [`NativeTextEditor::set_text`] does not.

#![forbid(unsafe_code)]

use std::ops::Range;

use gpui::{
    App, Bounds, Context, EventEmitter, FocusHandle, Focusable, Pixels, SharedString, TextLayout,
    Window, actions, size,
};

use artisan_ui::theme::DesktopTheme;

mod editing;
mod render;
mod text;

#[cfg(test)]
mod tests;

/// The window position of `index`, preferring the start of the next visual
/// row when `index` sits exactly on a soft-wrap boundary.
///
/// `TextLayout::position_for_index` resolves a wrap boundary to the end of
/// the row above, which would paint the caret there and stop Up from leaving
/// a wrapped row's first column. Must only be called after the layout was
/// painted.
fn visual_position(layout: &TextLayout, index: usize) -> Option<gpui::Point<Pixels>> {
    let position = layout.position_for_index(index)?;
    let line_height = layout.line_height();
    let bounds = layout.bounds();
    let next_row_y = position.y + line_height + line_height / 2.0;
    if position.x > bounds.left() && next_row_y < bounds.bottom() {
        let next_row_start = layout.index_for_position(gpui::point(bounds.left(), next_row_y));
        if matches!(next_row_start, Ok(start) | Err(start) if start == index) {
            return Some(gpui::point(bounds.left(), position.y + line_height));
        }
    }
    Some(position)
}

use text::{valid_bounds, valid_point};

actions!(
    native_text_editor,
    [
        Backspace,
        Delete,
        Left,
        Right,
        SelectLeft,
        SelectRight,
        SelectAll,
        Paste,
        Copy,
        Cut,
        Home,
        End,
        Up,
        Down,
        SelectHome,
        SelectEnd,
        SelectUp,
        SelectDown,
        DocumentHome,
        DocumentEnd,
        SelectDocumentHome,
        SelectDocumentEnd,
        InsertNewline,
        InsertTab,
        Undo,
        Redo,
    ]
);

/// Key context of the editor surface; bindings registered by
/// [`NativeTextEditor::bind_actions`] only apply inside it.
pub const NATIVE_TEXT_EDITOR_KEY_CONTEXT: &str = "NativeTextEditor";
/// Debug selector of the editor root.
pub const NATIVE_TEXT_EDITOR_SELECTOR: &str = "native-text-editor";
/// Debug selector of the placeholder, painted only while the text is empty.
pub const NATIVE_TEXT_EDITOR_PLACEHOLDER_SELECTOR: &str = "native-text-editor-placeholder";

/// Undo snapshots kept per editor; the oldest is dropped first.
const UNDO_LIMIT: usize = 64;
/// Text inserted by the Tab key.
const TAB_TEXT: &str = "  ";

/// Events emitted by [`NativeTextEditor`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeTextEditorEvent {
    /// The user changed the text (typing, deletion, paste, cut, undo, redo).
    Changed,
}

/// One undo or redo step: the whole text and the selection it had.
#[derive(Clone, Debug)]
struct Snapshot {
    text: String,
    selection: Range<usize>,
}

/// Native GPUI multiline plain-text editor.
pub struct NativeTextEditor {
    text: String,
    /// Shared copy of `text` handed to the text element; rebuilt only after
    /// an edit so an unchanged document is not copied on every render.
    shared_text: Option<SharedString>,
    placeholder: SharedString,
    disabled: bool,
    focus_handle: FocusHandle,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    /// Caret offset after the last typed insertion; the next insertion at
    /// exactly this offset joins the same undo step.
    typing_group_end: Option<usize>,
    selection: Range<usize>,
    selection_reversed: bool,
    selection_anchor: Option<usize>,
    vertical_goal_x: Option<Pixels>,
    vertical_goal_column: Option<usize>,
    selection_dragging: bool,
    marked_range: Option<Range<usize>>,
    layout: Option<TextLayout>,
    painted_bounds: Option<Bounds<Pixels>>,
}

impl EventEmitter<NativeTextEditorEvent> for NativeTextEditor {}

impl Focusable for NativeTextEditor {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl NativeTextEditor {
    /// Largest document the editor holds, in UTF-8 bytes.
    pub const MAX_BYTES: usize = 64 * 1024;

    /// Creates an editor holding `text` (truncated to [`Self::MAX_BYTES`])
    /// with the caret at its end.
    pub fn new(text: &str, placeholder: impl Into<SharedString>, cx: &mut Context<Self>) -> Self {
        let text = text::truncate_to_bytes(text, Self::MAX_BYTES).to_owned();
        let end = text.len();
        Self {
            text,
            shared_text: None,
            placeholder: placeholder.into(),
            disabled: false,
            focus_handle: cx.focus_handle().tab_stop(true),
            undo: Vec::new(),
            redo: Vec::new(),
            typing_group_end: None,
            selection: end..end,
            selection_reversed: false,
            selection_anchor: None,
            vertical_goal_x: None,
            vertical_goal_column: None,
            selection_dragging: false,
            marked_range: None,
            layout: None,
            painted_bounds: None,
        }
    }

    /// The current text.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Whether the editor is read-only.
    #[must_use]
    pub fn is_disabled(&self) -> bool {
        self.disabled
    }

    /// Replaces the whole text as a fresh document: undo and redo history
    /// are cleared and the caret moves to the end. Does not emit
    /// [`NativeTextEditorEvent::Changed`].
    pub fn set_text(&mut self, text: &str, cx: &mut Context<Self>) {
        text::truncate_to_bytes(text, Self::MAX_BYTES).clone_into(&mut self.text);
        self.shared_text = None;
        self.undo.clear();
        self.redo.clear();
        let end = self.text.len();
        self.selection = end..end;
        self.selection_reversed = false;
        self.selection_anchor = None;
        self.selection_dragging = false;
        self.marked_range = None;
        self.reset_transient_state();
        self.layout = None;
        self.painted_bounds = None;
        cx.notify();
    }

    /// Makes the editor read-only (no editing, no caret, dimmed) or
    /// editable again.
    pub fn set_disabled(&mut self, disabled: bool, cx: &mut Context<Self>) {
        if self.disabled == disabled {
            return;
        }
        self.disabled = disabled;
        self.selection_dragging = false;
        self.marked_range = None;
        self.reset_transient_state();
        cx.notify();
    }

    /// Forgets the vertical goal and the open typing undo group.
    fn reset_transient_state(&mut self) {
        self.vertical_goal_x = None;
        self.vertical_goal_column = None;
        self.typing_group_end = None;
    }

    fn shared_text(&mut self) -> SharedString {
        self.shared_text
            .get_or_insert_with(|| SharedString::from(self.text.clone()))
            .clone()
    }

    /// Returns the caret quad for the focused, enabled, collapsed selection.
    fn caret_quad(&self, window: &Window) -> Option<gpui::PaintQuad> {
        if self.disabled || !self.focus_handle.is_focused(window) {
            return None;
        }
        let offset = text::caret_offset(&self.selection, &self.text)?;
        self.painted_bounds.filter(valid_bounds)?;
        let layout = self.layout.as_ref()?;
        let line_height = layout.line_height();
        let origin = visual_position(layout, offset).unwrap_or_else(|| layout.bounds().origin);
        if !valid_point(origin) || line_height <= Pixels::ZERO {
            return None;
        }
        Some(gpui::fill(
            Bounds::new(origin, size(gpui::px(2.0), line_height)),
            DesktopTheme::neutral_dark().foreground,
        ))
    }
}
