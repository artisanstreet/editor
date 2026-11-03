//! Bounded text editing with grapheme movement, cached word boundaries, IME and byte-budgeted undo.
//!
//! Single-line fields normalize newlines and tabs. Prompt mode emits submit/list/pasted-file events.
//! All modes share MAX_BYTES; rejected input shows a limit notice. Programmatic replacement is UTF-8 clipped and also reports the limit.

#![forbid(unsafe_code)]
#![expect(
    clippy::derive_partial_eq_without_eq,
    reason = "gpui::actions! derives PartialEq without Eq for its unit actions"
)]

use std::ops::Range;
use std::path::PathBuf;
use std::rc::Rc;

use gpui::{
    App, Bounds, Context, EventEmitter, FocusHandle, Focusable, HighlightStyle, Pixels,
    ScrollHandle, SharedString, TextLayout, Window, actions, size,
};

use crate::text_navigation::SelectionUnit;

mod editing;
mod render;
mod text;

/// The window position of `index`, preferring the start of the next visual
/// row when `index` sits exactly on a soft-wrap boundary.
///
/// `TextLayout::position_for_index` resolves a wrap boundary to the end of
/// the row above, which would paint the caret there and stop Up from leaving
/// a wrapped row's first column. Must only be called after the layout was
/// painted.
pub(crate) fn visual_position(layout: &TextLayout, index: usize) -> Option<gpui::Point<Pixels>> {
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
    text_editor,
    [
        Backspace,
        Delete,
        Left,
        Right,
        SelectLeft,
        SelectRight,
        WordLeft,
        WordRight,
        SelectWordLeft,
        SelectWordRight,
        BackspaceWord,
        DeleteWord,
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
        InsertLineBreak,
        InsertTab,
        Dismiss,
        Undo,
        Redo,
    ]
);

/// Key context of the editor surface; bindings registered by
/// [`TextEditor::bind_actions`] only apply inside it.
pub const TEXT_EDITOR_KEY_CONTEXT: &str = "TextEditor";
/// Debug selector of the editor root.
pub const TEXT_EDITOR_SELECTOR: &str = "text-editor";
/// Debug selector of the placeholder, painted only while the text is empty.
pub const TEXT_EDITOR_PLACEHOLDER_SELECTOR: &str = "text-editor-placeholder";

/// Undo snapshots kept per editor; the oldest is dropped first.
const UNDO_LIMIT: usize = 64;
const HISTORY_BYTES: usize = 1024 * 1024;
/// Text inserted by the Tab key.
const TAB_TEXT: &str = "  ";

/// Events emitted by [`TextEditor`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TextEditorEvent {
    /// The user changed the text (typing, deletion, paste, cut, undo, redo).
    Changed,
    /// The user pressed Enter in a single-line editor or a prompt.
    Submitted,
}

/// A key meant for the list an owner shows over the editor; see
/// [`TextEditor::set_list_open`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ListKey {
    Up,
    Down,
    /// Enter or Tab.
    Accept,
    /// Escape.
    Dismiss,
}

/// A range of the text its owner recognises, such as a mention: painted in
/// `style` and edited as one unit (a delete at its edge removes it whole,
/// the arrows step over it).
///
/// A token with a `glyph` paints that icon in place of its first character
/// (a mention's sigil): the character stays in the text but paints
/// transparent, widened so the glyph and a gap fit where it was.
#[derive(Clone, Debug, PartialEq)]
pub struct Token {
    pub range: Range<usize>,
    pub style: HighlightStyle,
    pub glyph: Option<crate::AssetId>,
}

/// Finds the [`Token`]s of a text, in order and never overlapping.
pub type Tokenizer = Rc<dyn Fn(&str) -> Vec<Token>>;

/// Images or files the user pasted into a [`TextEditor::prompt`].
#[derive(Clone, Debug)]
pub struct PastedFiles {
    pub images: Vec<gpui::Image>,
    pub paths: Vec<PathBuf>,
}

/// What kind of field the editor is.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Mode {
    /// A document: Enter breaks the line, Tab indents.
    Document,
    SingleLine,
    /// A message prompt; see [`TextEditor::prompt`].
    Prompt,
}

/// One undo or redo step: the whole text and the selection it had.
#[derive(Clone, Debug)]
struct Snapshot {
    text: String,
    selection: Range<usize>,
}

/// Native GPUI multiline plain-text editor.
#[expect(
    clippy::struct_excessive_bools,
    reason = "disabled, reversed-selection, dragging and caret-follow are independent editor flags; an enum per flag would add nothing"
)]
pub struct TextEditor {
    text: String,
    /// Shared copy of `text` handed to the text element; rebuilt only after
    /// an edit so an unchanged document is not copied on every render.
    shared_text: Option<SharedString>,
    word_boundaries: Option<Rc<crate::text_navigation::WordBoundaries>>,
    limit_notice: bool,
    placeholder: SharedString,
    /// Shown instead of the placeholder in an empty prompt, with the Tab
    /// key that writes it in.
    suggestion: Option<SharedString>,
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
    /// The unit the current mouse selection started with and the range its
    /// click selected; a drag grows the selection by that unit.
    drag_origin: (SelectionUnit, Range<usize>),
    marked_range: Option<Range<usize>>,
    layout: Option<TextLayout>,
    painted_bounds: Option<Bounds<Pixels>>,
    mode: Mode,
    /// Scroll offset of a prompt that outgrew its height.
    scroll: ScrollHandle,
    /// Scroll the caret into view after the next paint.
    follow_caret: bool,
    tokenizer: Option<Tokenizer>,
    /// The tokens of `text`; dropped with every edit.
    tokens: Option<Rc<[Token]>>,
    /// An owner's list takes the navigation keys.
    list_open: bool,
    /// The owner draws the field's box; see [`TextEditor::bare`].
    bare: bool,
    inset: bool,
}

impl EventEmitter<TextEditorEvent> for TextEditor {}
impl EventEmitter<PastedFiles> for TextEditor {}
impl EventEmitter<ListKey> for TextEditor {}

impl Focusable for TextEditor {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl TextEditor {
    /// Largest document the editor holds, in UTF-8 bytes.
    pub const MAX_BYTES: usize = 64 * 1024;

    /// Creates an editor holding `text` (truncated to [`Self::MAX_BYTES`])
    /// with the caret at its end.
    pub fn new(text: &str, placeholder: impl Into<SharedString>, cx: &mut Context<Self>) -> Self {
        let limit_notice = text.len() > Self::MAX_BYTES;
        let text = text::truncate_to_bytes(text, Self::MAX_BYTES).to_owned();
        let end = text.len();
        Self {
            text,
            shared_text: None,
            word_boundaries: None,
            limit_notice,
            placeholder: placeholder.into(),
            suggestion: None,
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
            drag_origin: (SelectionUnit::Character, end..end),
            marked_range: None,
            layout: None,
            painted_bounds: None,
            mode: Mode::Document,
            scroll: ScrollHandle::new(),
            follow_caret: false,
            tokenizer: None,
            tokens: None,
            list_open: false,
            bare: false,
            inset: false,
        }
    }

    /// Makes the editor a compact field for one line of text: Enter
    /// submits, and line breaks become spaces.
    #[must_use]
    pub fn single_line(mut self) -> Self {
        self.mode = Mode::SingleLine;
        self.text = self.text.replace(['\r', '\n', '\t'], " ");
        self
    }

    /// Leaves out the field's border and fill, for an owner that draws its
    /// own box around the editor (the Marketplace's search).
    #[must_use]
    pub const fn bare(mut self) -> Self {
        self.bare = true;
        self
    }

    #[must_use]
    pub const fn inset(mut self) -> Self {
        self.inset = true;
        self
    }

    /// Makes the editor a message prompt; see the module docs.
    #[must_use]
    pub const fn prompt(mut self) -> Self {
        self.mode = Mode::Prompt;
        self
    }

    /// Fields and prompts share a visible 64 KiB bound; large source belongs in attachments.
    const fn max_bytes() -> usize {
        Self::MAX_BYTES
    }

    fn word_boundaries(&mut self) -> Rc<crate::text_navigation::WordBoundaries> {
        Rc::clone(self.word_boundaries.get_or_insert_with(|| {
            Rc::new(crate::text_navigation::WordBoundaries::new(&self.text))
        }))
    }

    /// Selects the whole text, as for replacing it outright.
    pub fn select_whole_text(&mut self, cx: &mut Context<Self>) {
        self.selection = 0..self.text.len();
        self.selection_reversed = false;
        self.selection_anchor = None;
        self.reset_transient_state();
        cx.notify();
    }

    /// The current text.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Whether the editor is read-only.
    #[must_use]
    pub const fn is_disabled(&self) -> bool {
        self.disabled
    }

    /// Replaces the whole text as a fresh document: undo and redo history
    /// are cleared and the caret moves to the end. Does not emit
    /// [`TextEditorEvent::Changed`].
    pub fn set_text(&mut self, text: &str, cx: &mut Context<Self>) {
        let normalized;
        let text = if self.mode == Mode::SingleLine {
            normalized = text.replace(['\r', '\n', '\t'], " ");
            normalized.as_str()
        } else {
            text
        };
        self.limit_notice = text.len() > Self::max_bytes();
        text::truncate_to_bytes(text, Self::max_bytes()).clone_into(&mut self.text);
        self.shared_text = None;
        self.word_boundaries = None;
        self.tokens = None;
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

    /// Replaces the words shown while the text is empty.
    pub fn set_placeholder(
        &mut self,
        placeholder: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) {
        let placeholder = placeholder.into();
        if self.placeholder != placeholder {
            self.placeholder = placeholder;
            cx.notify();
        }
    }

    /// Offers a prompt's likely next message: an empty prompt shows it
    /// instead of the placeholder, and Tab writes it in.
    pub fn set_suggestion(&mut self, suggestion: Option<SharedString>, cx: &mut Context<Self>) {
        if self.suggestion != suggestion {
            self.suggestion = suggestion;
            if self.text.is_empty() {
                cx.notify();
            }
        }
    }

    /// The suggestion an empty prompt shows now.
    #[must_use]
    pub fn shown_suggestion(&self) -> Option<&SharedString> {
        self.suggestion
            .as_ref()
            .filter(|_| self.mode == Mode::Prompt && self.text.is_empty() && !self.disabled)
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

    /// The caret offset while the selection is collapsed and no IME
    /// composition is open.
    #[must_use]
    pub fn caret(&self) -> Option<usize> {
        (self.selection.is_empty() && self.marked_range.is_none()).then_some(self.selection.end)
    }

    /// Replaces `range` with `text` as one user edit (undoable, emits
    /// [`TextEditorEvent::Changed`]), leaving the caret after it. A range
    /// outside the text or off a character boundary is ignored.
    pub fn replace(&mut self, range: Range<usize>, text: &str, cx: &mut Context<Self>) {
        let valid = range.start <= range.end
            && range.end <= self.text.len()
            && self.text.is_char_boundary(range.start)
            && self.text.is_char_boundary(range.end);
        if valid {
            self.typing_group_end = None;
            self.replace_range(range, text, None, cx);
        }
    }

    /// Sets what recognises [`Token`]s in the text; `None` paints and
    /// edits it as plain text.
    pub fn set_tokenizer(&mut self, tokenizer: Option<Tokenizer>, cx: &mut Context<Self>) {
        self.tokenizer = tokenizer;
        self.tokens = None;
        cx.notify();
    }

    /// While `open`, Up, Down, Enter, Tab and Escape are emitted as
    /// [`ListKey`]s for the owner's list instead of acting on the text.
    pub const fn set_list_open(&mut self, open: bool) {
        self.list_open = open;
    }

    /// The tokens of the current text. Tokens the tokenizer placed out of
    /// order, overlapping, or off a character boundary are dropped.
    fn tokens(&mut self) -> Rc<[Token]> {
        if let Some(tokens) = &self.tokens {
            return Rc::clone(tokens);
        }
        let mut end = 0;
        let tokens: Rc<[Token]> = self
            .tokenizer
            .as_ref()
            .map(|tokenize| tokenize(&self.text))
            .unwrap_or_default()
            .into_iter()
            .filter(|token| {
                let range = &token.range;
                let valid = end <= range.start
                    && range.start < range.end
                    && range.end <= self.text.len()
                    && self.text.is_char_boundary(range.start)
                    && self.text.is_char_boundary(range.end);
                if valid {
                    end = range.end;
                }
                valid
            })
            .collect();
        self.tokens = Some(Rc::clone(&tokens));
        tokens
    }

    /// Forgets the vertical goal and the open typing undo group.
    const fn reset_transient_state(&mut self) {
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
    fn caret_quad(&self, window: &Window, color: gpui::Hsla) -> Option<gpui::PaintQuad> {
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
            color,
        ))
    }
}

#[cfg(test)]
mod reform_tests {
    use super::*;
    use gpui::{AppContext as _, TestAppContext};

    #[gpui::test]
    fn all_modes_reject_oversized_user_edits_without_losing_existing_text(cx: &mut TestAppContext) {
        let input = "x".repeat(TextEditor::MAX_BYTES);
        let editor = cx.new(|cx| TextEditor::new(&input, "", cx).prompt());
        editor.update(cx, |editor, cx| {
            editor.replace(0..0, "extra", cx);
            assert_eq!(editor.text(), input);
            assert!(editor.limit_notice);
            assert!(editor.undo.is_empty());
            editor.replace(0..1, "a", cx);
            assert!(!editor.limit_notice);
            assert_eq!(editor.text().len(), TextEditor::MAX_BYTES);
        });
    }

    #[gpui::test]
    fn shift_up_on_the_first_line_selects_to_the_start_like_chrome(cx: &mut TestAppContext) {
        let editor = cx.new(|cx| TextEditor::new("one two\nthree", "", cx));
        editor.update(cx, |editor, cx| {
            editor.selection = 4..4;
            editor.move_vertical(true, true, cx);
            assert_eq!(editor.selection, 0..4);
            editor.selection = 10..10;
            editor.selection_anchor = None;
            editor.reset_transient_state();
            editor.move_vertical(false, true, cx);
            assert_eq!(editor.selection, 10..13);
            editor.move_vertical(true, false, cx);
            assert_eq!(editor.selection, 2..2);
        });
    }

    #[gpui::test]
    fn single_line_replacement_and_history_share_the_size_contract(cx: &mut TestAppContext) {
        let editor = cx.new(|cx| TextEditor::new("a\nb", "", cx).single_line());
        editor.update(cx, |editor, cx| {
            assert_eq!(editor.text(), "a b");
            editor.set_text("a\tb\rc", cx);
            assert_eq!(editor.text(), "a b c");
            editor.set_text(&"x".repeat(60_000), cx);
            let words = editor.word_boundaries();
            assert!(Rc::ptr_eq(&words, &editor.word_boundaries()));
            for index in 0..100 {
                editor.replace(0..1, if index % 2 == 0 { "a" } else { "b" }, cx);
            }
            assert!(!Rc::ptr_eq(&words, &editor.word_boundaries()));
            assert!(
                editor
                    .undo
                    .iter()
                    .chain(&editor.redo)
                    .map(|s| s.text.len())
                    .sum::<usize>()
                    <= HISTORY_BYTES
            );
        });
    }
}
