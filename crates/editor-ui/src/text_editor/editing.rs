//! Editing, selection, clipboard, undo/redo, and keyboard/mouse navigation
//! for the native text editor, adapted from the composer's editing module.

#![forbid(unsafe_code)]

use std::ops::Range;

use crate::text_navigation::{WordBehavior, caret_index_for_position};
use gpui::{
    App, ClipboardEntry, ClipboardItem, Context, KeyBinding, Pixels, Point, Window, point, px,
};

use super::text::{
    floor_char_boundary, logical_column, logical_line_end, logical_line_start,
    logical_vertical_target, next_character_boundary, previous_character_boundary,
    utf16_offset_to_utf8, utf16_range_to_utf8, valid_bounds, valid_point,
};
use super::{
    Backspace, BackspaceWord, Copy, Cut, Delete, DeleteWord, Dismiss, DocumentEnd, DocumentHome,
    Down, End, HISTORY_BYTES, Home, InsertLineBreak, InsertNewline, InsertTab, Left, ListKey, Mode,
    Paste, PastedFiles, Redo, Right, SelectAll, SelectDocumentEnd, SelectDocumentHome, SelectDown,
    SelectEnd, SelectHome, SelectLeft, SelectRight, SelectUp, SelectWordLeft, SelectWordRight,
    Snapshot, TAB_TEXT, TEXT_EDITOR_KEY_CONTEXT, TextEditor, TextEditorEvent, UNDO_LIMIT, Undo, Up,
    WordLeft, WordRight,
};
use crate::text_navigation::SelectionUnit;

impl TextEditor {
    /// Registers the editor's key bindings. They apply only inside the
    /// [`TEXT_EDITOR_KEY_CONTEXT`] key context, so they never collide
    /// with the composer's or the workspace's bindings.
    pub fn bind_actions(cx: &mut App) {
        let context = Some(TEXT_EDITOR_KEY_CONTEXT);
        cx.bind_keys([
            KeyBinding::new("ctrl-z", Undo, context),
            KeyBinding::new("cmd-z", Undo, context),
            KeyBinding::new("ctrl-shift-z", Redo, context),
            KeyBinding::new("cmd-shift-z", Redo, context),
            KeyBinding::new("ctrl-y", Redo, context),
            KeyBinding::new("backspace", Backspace, context),
            KeyBinding::new("shift-backspace", Backspace, context),
            KeyBinding::new("delete", Delete, context),
            KeyBinding::new("left", Left, context),
            KeyBinding::new("right", Right, context),
            KeyBinding::new("shift-left", SelectLeft, context),
            KeyBinding::new("shift-right", SelectRight, context),
            // Word movement and deletion: ctrl on Windows and Linux, alt
            // (option) on macOS, as in Chrome.
            KeyBinding::new("ctrl-left", WordLeft, context),
            KeyBinding::new("ctrl-right", WordRight, context),
            KeyBinding::new("ctrl-shift-left", SelectWordLeft, context),
            KeyBinding::new("ctrl-shift-right", SelectWordRight, context),
            KeyBinding::new("ctrl-backspace", BackspaceWord, context),
            KeyBinding::new("ctrl-delete", DeleteWord, context),
            KeyBinding::new("alt-left", WordLeft, context),
            KeyBinding::new("alt-right", WordRight, context),
            KeyBinding::new("alt-shift-left", SelectWordLeft, context),
            KeyBinding::new("alt-shift-right", SelectWordRight, context),
            KeyBinding::new("alt-backspace", BackspaceWord, context),
            KeyBinding::new("alt-delete", DeleteWord, context),
            KeyBinding::new("cmd-a", SelectAll, context),
            KeyBinding::new("ctrl-a", SelectAll, context),
            KeyBinding::new("cmd-v", Paste, context),
            KeyBinding::new("ctrl-v", Paste, context),
            KeyBinding::new("cmd-c", Copy, context),
            KeyBinding::new("ctrl-c", Copy, context),
            KeyBinding::new("cmd-x", Cut, context),
            KeyBinding::new("ctrl-x", Cut, context),
            KeyBinding::new("home", Home, context),
            KeyBinding::new("end", End, context),
            KeyBinding::new("up", Up, context),
            KeyBinding::new("down", Down, context),
            KeyBinding::new("shift-home", SelectHome, context),
            KeyBinding::new("shift-end", SelectEnd, context),
            KeyBinding::new("shift-up", SelectUp, context),
            KeyBinding::new("shift-down", SelectDown, context),
            KeyBinding::new("ctrl-home", DocumentHome, context),
            KeyBinding::new("ctrl-end", DocumentEnd, context),
            KeyBinding::new("ctrl-shift-home", SelectDocumentHome, context),
            KeyBinding::new("ctrl-shift-end", SelectDocumentEnd, context),
            KeyBinding::new("cmd-home", DocumentHome, context),
            KeyBinding::new("cmd-end", DocumentEnd, context),
            KeyBinding::new("cmd-shift-home", SelectDocumentHome, context),
            KeyBinding::new("cmd-shift-end", SelectDocumentEnd, context),
            KeyBinding::new("cmd-up", DocumentHome, context),
            KeyBinding::new("cmd-down", DocumentEnd, context),
            KeyBinding::new("cmd-shift-up", SelectDocumentHome, context),
            KeyBinding::new("cmd-shift-down", SelectDocumentEnd, context),
            KeyBinding::new("enter", InsertNewline, context),
            KeyBinding::new("shift-enter", InsertLineBreak, context),
            KeyBinding::new("tab", InsertTab, context),
            KeyBinding::new("escape", Dismiss, context),
        ]);
    }

    /// Replaces `range` with `replacement` as one user edit.
    ///
    /// Reject the whole replacement when it exceeds the shared byte cap. `marked_selection` (UTF-16, relative to the replacement)
    /// marks the inserted text as an IME composition.
    pub(super) fn replace_range(
        &mut self,
        range: Range<usize>,
        replacement: &str,
        marked_selection: Option<Range<usize>>,
        cx: &mut Context<Self>,
    ) {
        if self.disabled {
            return;
        }
        // A line break or tab is one byte, as is the space replacing it, so
        // marked offsets into the replacement still hold.
        let flattened;
        let replacement =
            if self.mode == Mode::SingleLine && replacement.contains(['\r', '\n', '\t']) {
                flattened = replacement.replace(['\r', '\n', '\t'], " ");
                flattened.as_str()
            } else {
                replacement
            };
        let retained = self.text.len().saturating_sub(range.len());
        if replacement.len() > self.max_bytes().saturating_sub(retained) {
            self.limit_notice = true;
            cx.notify();
            return;
        }
        self.limit_notice = false;
        let fitted = replacement;
        if range.start > range.end || self.text.get(range.clone()).is_none() {
            return;
        }
        let selected_offsets = match marked_selection.clone() {
            Some(selected) if selected.start <= selected.end => {
                let (Some(start), Some(end)) = (
                    utf16_offset_to_utf8(fitted, selected.start),
                    utf16_offset_to_utf8(fitted, selected.end),
                ) else {
                    return;
                };
                Some((start, end))
            }
            Some(_) => return,
            None => None,
        };
        let changed = self.text.get(range.clone()) != Some(fitted);
        let replacement_end = range.start + fitted.len();
        let single_character = {
            let mut characters = fitted.chars();
            characters.next().is_some() && characters.next().is_none()
        };
        let typing = range.is_empty()
            && marked_selection.is_none()
            && single_character
            && !fitted.contains(char::is_whitespace);
        let continues_group = typing && self.typing_group_end == Some(range.start);
        if changed && self.marked_range.is_none() && !continues_group {
            self.push_undo();
            self.redo.clear();
        } else if changed {
            self.redo.clear();
        }

        self.text.replace_range(range.clone(), fitted);
        self.shared_text = None;
        self.word_boundaries = None;
        self.tokens = None;
        self.layout = None;
        self.painted_bounds = None;
        self.selection_anchor = None;
        self.selection_dragging = false;
        self.reset_transient_state();
        if let Some((start, end)) = selected_offsets {
            self.selection = range.start + start..range.start + end;
            self.marked_range = Some(range.start..replacement_end);
        } else {
            self.selection = replacement_end..replacement_end;
            self.marked_range = None;
            // Whitespace starts a new undo step; the word typed after it
            // joins that step, so undo removes " word" at a time.
            if range.is_empty() && single_character && fitted != "\n" {
                self.typing_group_end = Some(replacement_end);
            }
        }
        self.selection_reversed = false;
        self.follow_caret = true;
        if changed {
            cx.emit(TextEditorEvent::Changed);
        }
        cx.notify();
    }

    fn push_undo(&mut self) {
        self.undo.push(Snapshot {
            text: self.text.clone(),
            selection: self.selection.clone(),
        });
        self.bound_history();
    }

    fn bound_history(&mut self) {
        let mut bytes = self
            .undo
            .iter()
            .chain(&self.redo)
            .map(|snapshot| snapshot.text.len())
            .sum::<usize>();
        while self.undo.len() + self.redo.len() > UNDO_LIMIT || bytes > HISTORY_BYTES {
            // Oldest undo, then furthest redo; never evict the next available operation first.
            let removed = if !self.undo.is_empty() {
                self.undo.remove(0)
            } else if !self.redo.is_empty() {
                self.redo.remove(0)
            } else {
                break;
            };
            bytes -= removed.text.len();
        }
    }

    fn restore(&mut self, snapshot: Snapshot, cx: &mut Context<Self>) {
        self.text = snapshot.text;
        self.selection = snapshot.selection;
        self.shared_text = None;
        self.word_boundaries = None;
        self.tokens = None;
        self.selection_reversed = false;
        self.selection_anchor = None;
        self.selection_dragging = false;
        self.marked_range = None;
        self.reset_transient_state();
        self.layout = None;
        self.painted_bounds = None;
        self.follow_caret = true;
        cx.emit(TextEditorEvent::Changed);
        cx.notify();
    }

    pub(super) fn undo_action(&mut self, _: &Undo, _: &mut Window, cx: &mut Context<Self>) {
        if self.disabled || self.marked_range.is_some() {
            return;
        }
        if let Some(snapshot) = self.undo.pop() {
            self.redo.push(Snapshot {
                text: self.text.clone(),
                selection: self.selection.clone(),
            });
            self.bound_history();
            self.restore(snapshot, cx);
        }
    }

    pub(super) fn redo_action(&mut self, _: &Redo, _: &mut Window, cx: &mut Context<Self>) {
        if self.disabled || self.marked_range.is_some() {
            return;
        }
        if let Some(snapshot) = self.redo.pop() {
            self.undo.push(Snapshot {
                text: self.text.clone(),
                selection: self.selection.clone(),
            });
            self.bound_history();
            self.restore(snapshot, cx);
        }
    }

    /// The UTF-8 range an input-handler edit replaces: the given UTF-16
    /// range, else the marked text, else the selection.
    pub(super) fn replacement_range(&self, range: Option<Range<usize>>) -> Option<Range<usize>> {
        let range = match range {
            Some(range) => utf16_range_to_utf8(&self.text, range),
            None => self
                .marked_range
                .clone()
                .or_else(|| Some(self.selection.clone())),
        }?;
        (range.start <= range.end
            && range.end <= self.text.len()
            && self.text.is_char_boundary(range.start)
            && self.text.is_char_boundary(range.end))
        .then_some(range)
    }

    fn insert(&mut self, text: &str, cx: &mut Context<Self>) {
        if self.disabled {
            return;
        }
        if let Some(range) = self.replacement_range(None) {
            self.replace_range(range, text, None, cx);
        }
    }

    pub(super) fn insert_newline(
        &mut self,
        _: &InsertNewline,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match self.mode {
            // Enter inside an IME composition commits it, not the message.
            Mode::Prompt if self.marked_range.is_some() => {}
            _ if self.list_open => cx.emit(ListKey::Accept),
            Mode::SingleLine | Mode::Prompt => cx.emit(TextEditorEvent::Submitted),
            Mode::Document => self.insert("\n", cx),
        }
    }

    /// Shift+Enter: a line break everywhere except a single-line field,
    /// where it submits like Enter.
    pub(super) fn insert_line_break(
        &mut self,
        _: &InsertLineBreak,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.mode == Mode::SingleLine {
            cx.emit(TextEditorEvent::Submitted);
            return;
        }
        self.insert("\n", cx);
    }

    pub(super) fn insert_tab(&mut self, _: &InsertTab, _: &mut Window, cx: &mut Context<Self>) {
        if self.list_open {
            cx.emit(ListKey::Accept);
            return;
        }
        match self.mode {
            Mode::Document => self.insert(TAB_TEXT, cx),
            Mode::SingleLine => {}
            // Tab moves focus out of a prompt.
            Mode::Prompt => cx.propagate(),
        }
    }

    /// Escape dismisses the owner's list, else goes to whatever is behind
    /// the editor.
    pub(super) fn dismiss(&mut self, _: &Dismiss, _: &mut Window, cx: &mut Context<Self>) {
        if self.list_open {
            cx.emit(ListKey::Dismiss);
        } else {
            cx.propagate();
        }
    }

    /// A caret target that landed inside a token, moved to the token's edge
    /// in the direction of travel.
    fn step_over_tokens(&mut self, target: usize, forward: bool) -> usize {
        self.tokens()
            .iter()
            .find(|token| token.range.start < target && target < token.range.end)
            .map_or(target, |token| {
                if forward {
                    token.range.end
                } else {
                    token.range.start
                }
            })
    }

    /// The token one delete at `cursor` removes whole: the one the caret
    /// ends (going backward) or starts (going forward), or is inside.
    fn token_deletion(&mut self, cursor: usize, forward: bool) -> Option<Range<usize>> {
        self.tokens()
            .iter()
            .find(|token| {
                let range = &token.range;
                let edge = if forward {
                    range.start == cursor
                } else {
                    range.end == cursor
                };
                edge || (range.start < cursor && cursor < range.end)
            })
            .map(|token| token.range.clone())
    }

    fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selection.start
        } else {
            self.selection.end
        }
    }

    pub(super) fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        let offset = floor_char_boundary(&self.text, offset);
        self.selection = offset..offset;
        self.selection_reversed = false;
        self.selection_anchor = None;
        self.reset_transient_state();
        self.marked_range = None;
        self.selection_dragging = false;
        self.follow_caret = true;
        cx.notify();
    }

    fn select_to(&mut self, offset: usize) {
        let offset = floor_char_boundary(&self.text, offset);
        let anchor = *self.selection_anchor.get_or_insert(
            if self.selection.is_empty() || self.selection_reversed {
                self.selection.end
            } else {
                self.selection.start
            },
        );
        if offset < anchor {
            self.selection = offset..anchor;
            self.selection_reversed = true;
        } else {
            self.selection = anchor..offset;
            self.selection_reversed = false;
        }
        self.typing_group_end = None;
    }

    /// Extends the selection to `offset` from a keyboard action.
    fn extend_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.select_to(offset);
        self.marked_range = None;
        self.vertical_goal_x = None;
        self.vertical_goal_column = None;
        self.selection_dragging = false;
        self.follow_caret = true;
        cx.notify();
    }

    pub(super) fn move_left(&mut self, extend: bool, cx: &mut Context<Self>) {
        if self.disabled {
            return;
        }
        let target = previous_character_boundary(&self.text, self.cursor_offset());
        let target = self.step_over_tokens(target, false);
        if extend {
            self.extend_to(target, cx);
        } else if self.selection.is_empty() {
            self.move_to(target, cx);
        } else {
            self.move_to(self.selection.start, cx);
        }
    }

    pub(super) fn move_right(&mut self, extend: bool, cx: &mut Context<Self>) {
        if self.disabled {
            return;
        }
        let target = next_character_boundary(&self.text, self.cursor_offset());
        let target = self.step_over_tokens(target, true);
        if extend {
            self.extend_to(target, cx);
        } else if self.selection.is_empty() {
            self.move_to(target, cx);
        } else {
            self.move_to(self.selection.end, cx);
        }
    }

    pub(super) fn left_action(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        self.move_left(false, cx);
    }

    pub(super) fn right_action(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        self.move_right(false, cx);
    }

    pub(super) fn select_left_action(
        &mut self,
        _: &SelectLeft,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_left(true, cx);
    }

    pub(super) fn select_right_action(
        &mut self,
        _: &SelectRight,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_right(true, cx);
    }

    /// Moves the caret one word left or right from the selection's head
    /// (Chrome's `ctrl`+arrow), or with `extend` moves only the head.
    pub(super) fn move_word(&mut self, forward: bool, extend: bool, cx: &mut Context<Self>) {
        if self.disabled {
            return;
        }
        let cursor = self.cursor_offset();
        let target = if forward {
            self.word_boundaries()
                .next(&self.text, cursor, WordBehavior::platform())
        } else {
            self.word_boundaries().previous(&self.text, cursor)
        };
        if extend {
            self.extend_to(target, cx);
        } else {
            self.move_to(target, cx);
        }
    }

    pub(super) fn word_left_action(
        &mut self,
        _: &WordLeft,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_word(false, false, cx);
    }

    pub(super) fn word_right_action(
        &mut self,
        _: &WordRight,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_word(true, false, cx);
    }

    pub(super) fn select_word_left_action(
        &mut self,
        _: &SelectWordLeft,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_word(false, true, cx);
    }

    pub(super) fn select_word_right_action(
        &mut self,
        _: &SelectWordRight,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_word(true, true, cx);
    }

    /// Deletes the marked text or the selection, else one character (or,
    /// with `word`, up to the next word stop) before or after the caret.
    fn delete_toward(&mut self, forward: bool, word: bool, cx: &mut Context<Self>) {
        if self.disabled {
            return;
        }
        let cursor = self.cursor_offset();
        let token = self.token_deletion(cursor, forward);
        let range = self
            .marked_range
            .clone()
            .filter(|range| !range.is_empty())
            .or_else(|| (!self.selection.is_empty()).then(|| self.selection.clone()))
            .or(token)
            .or_else(|| {
                let range = match (forward, word) {
                    (true, false) => cursor..next_character_boundary(&self.text, cursor),
                    (true, true) => {
                        cursor
                            ..self.word_boundaries().next(
                                &self.text,
                                cursor,
                                WordBehavior::platform(),
                            )
                    }
                    (false, false) => previous_character_boundary(&self.text, cursor)..cursor,
                    (false, true) => self.word_boundaries().previous(&self.text, cursor)..cursor,
                };
                (!range.is_empty()).then_some(range)
            });
        if let Some(range) = range {
            self.replace_range(range, "", None, cx);
        }
    }

    pub(super) fn backspace(&mut self, _: &Backspace, _: &mut Window, cx: &mut Context<Self>) {
        self.delete_toward(false, false, cx);
    }

    pub(super) fn delete(&mut self, _: &Delete, _: &mut Window, cx: &mut Context<Self>) {
        self.delete_toward(true, false, cx);
    }

    pub(super) fn backspace_word(
        &mut self,
        _: &BackspaceWord,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.delete_toward(false, true, cx);
    }

    pub(super) fn delete_word(&mut self, _: &DeleteWord, _: &mut Window, cx: &mut Context<Self>) {
        self.delete_toward(true, true, cx);
    }

    pub(super) fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        if self.disabled {
            return;
        }
        self.selection = 0..self.text.len();
        self.selection_reversed = false;
        self.selection_anchor = None;
        self.reset_transient_state();
        self.selection_dragging = false;
        self.marked_range = None;
        cx.notify();
    }

    pub(super) fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        if !self.disabled {
            self.move_to(logical_line_start(&self.text, self.cursor_offset()), cx);
        }
    }

    pub(super) fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        if !self.disabled {
            self.move_to(logical_line_end(&self.text, self.cursor_offset()), cx);
        }
    }

    pub(super) fn select_home(&mut self, _: &SelectHome, _: &mut Window, cx: &mut Context<Self>) {
        if !self.disabled {
            self.extend_to(logical_line_start(&self.text, self.cursor_offset()), cx);
        }
    }

    pub(super) fn select_end(&mut self, _: &SelectEnd, _: &mut Window, cx: &mut Context<Self>) {
        if !self.disabled {
            self.extend_to(logical_line_end(&self.text, self.cursor_offset()), cx);
        }
    }

    pub(super) fn document_home(
        &mut self,
        _: &DocumentHome,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.disabled {
            self.move_to(0, cx);
        }
    }

    pub(super) fn document_end(&mut self, _: &DocumentEnd, _: &mut Window, cx: &mut Context<Self>) {
        if !self.disabled {
            self.move_to(self.text.len(), cx);
        }
    }

    pub(super) fn select_document_home(
        &mut self,
        _: &SelectDocumentHome,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.disabled {
            self.extend_to(0, cx);
        }
    }

    pub(super) fn select_document_end(
        &mut self,
        _: &SelectDocumentEnd,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.disabled {
            self.extend_to(self.text.len(), cx);
        }
    }

    pub(super) fn up(&mut self, _: &Up, _: &mut Window, cx: &mut Context<Self>) {
        if self.list_open {
            cx.emit(ListKey::Up);
        } else {
            self.move_vertical(true, false, cx);
        }
    }

    pub(super) fn down(&mut self, _: &Down, _: &mut Window, cx: &mut Context<Self>) {
        if self.list_open {
            cx.emit(ListKey::Down);
        } else {
            self.move_vertical(false, false, cx);
        }
    }

    pub(super) fn select_up(&mut self, _: &SelectUp, _: &mut Window, cx: &mut Context<Self>) {
        self.move_vertical(true, true, cx);
    }

    pub(super) fn select_down(&mut self, _: &SelectDown, _: &mut Window, cx: &mut Context<Self>) {
        self.move_vertical(false, true, cx);
    }

    /// Moves one visual line up or down, keeping the goal x (or, before the
    /// first paint, the logical goal column) across repeated presses.
    pub(super) fn move_vertical(&mut self, up: bool, extend: bool, cx: &mut Context<Self>) {
        if self.disabled {
            return;
        }
        let target = self.vertical_target(self.cursor_offset(), up);
        if extend {
            self.select_to(target);
        } else {
            self.selection = target..target;
            self.selection_reversed = false;
            self.selection_anchor = None;
        }
        self.typing_group_end = None;
        self.marked_range = None;
        self.selection_dragging = false;
        self.follow_caret = true;
        cx.notify();
    }

    fn vertical_target(&mut self, cursor: usize, up: bool) -> usize {
        let goal_column = *self
            .vertical_goal_column
            .get_or_insert_with(|| logical_column(&self.text, cursor));

        if self.painted_bounds.is_some()
            && let Some(layout) = self.layout.clone()
            && let Some(position) = super::visual_position(&layout, cursor)
        {
            let goal_x = *self.vertical_goal_x.get_or_insert(position.x);
            let line_height = layout.line_height();
            // Aim at the centre of the target row: the exact bottom edge of
            // a row belongs to that row in `index_for_position`.
            let target_y = if up {
                position.y - line_height / 2.0
            } else {
                position.y + line_height + line_height / 2.0
            };
            let layout_bounds = layout.bounds();
            if target_y < layout_bounds.top() || target_y >= layout_bounds.bottom() {
                return cursor;
            }
            let target = match layout.index_for_position(point(goal_x, target_y)) {
                Ok(index) | Err(index) => index,
            };
            return floor_char_boundary(&self.text, target);
        }

        logical_vertical_target(&self.text, cursor, up, goal_column)
    }

    pub(super) fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        if self.disabled {
            return;
        }
        let Some(item) = cx.read_from_clipboard() else {
            return;
        };
        if self.mode == Mode::Prompt {
            let (mut images, mut paths) = (Vec::new(), Vec::new());
            for entry in item.entries() {
                match entry {
                    ClipboardEntry::Image(image) => images.push(image.clone()),
                    ClipboardEntry::ExternalPaths(external) => {
                        paths.extend(external.paths().iter().cloned());
                    }
                    ClipboardEntry::String(_) => {}
                }
            }
            if !images.is_empty() || !paths.is_empty() {
                cx.emit(PastedFiles { images, paths });
                return;
            }
        }
        let Some(text) = item.text() else {
            return;
        };
        let text = text.replace("\r\n", "\n");
        self.typing_group_end = None;
        self.insert(&text, cx);
    }

    pub(super) fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if self.selection.is_empty() {
            return;
        }
        if let Some(selected) = self.text.get(self.selection.clone()) {
            cx.write_to_clipboard(ClipboardItem::new_string(selected.to_owned()));
        }
    }

    pub(super) fn cut(&mut self, _: &Cut, _: &mut Window, cx: &mut Context<Self>) {
        if self.disabled || self.selection.is_empty() {
            return;
        }
        let range = self.selection.clone();
        if let Some(selected) = self.text.get(range.clone()) {
            cx.write_to_clipboard(ClipboardItem::new_string(selected.to_owned()));
            self.replace_range(range, "", None, cx);
        }
    }

    /// Starts a mouse selection: one click places the caret (shift extends),
    /// a double click selects a word and a triple click a paragraph; the
    /// drag that follows grows the selection by that unit.
    pub(super) fn mouse_down(
        &mut self,
        position: Point<Pixels>,
        shift: bool,
        click_count: usize,
        cx: &mut Context<Self>,
    ) {
        if self.disabled {
            return;
        }
        let Some(index) = self.index_for_point(position, false) else {
            return;
        };
        self.marked_range = None;
        self.reset_transient_state();
        let unit = SelectionUnit::from_click_count(click_count);
        if unit == SelectionUnit::Character && shift {
            self.select_to(index);
        } else {
            self.selection = if unit == SelectionUnit::Word {
                self.word_boundaries()
                    .selection_range(unit, &self.text, index, false)
            } else {
                unit.range_at(&self.text, index)
            };
            self.selection_reversed = false;
            self.selection_anchor = None;
        }
        self.drag_origin = (unit, self.selection.clone());
        self.selection_dragging = true;
        cx.notify();
    }

    pub(super) fn mouse_drag(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        if !self.selection_dragging {
            return;
        }
        let Some(index) = self.index_for_point(position, true) else {
            return;
        };
        let before = (self.selection.clone(), self.selection_reversed);
        let unit = self.drag_origin.0;
        if unit == SelectionUnit::Character {
            self.select_to(index);
        } else {
            let target = self
                .word_boundaries()
                .selection_range(unit, &self.text, index, true);
            let origin = &self.drag_origin.1;
            self.selection_reversed = target.start < origin.start;
            self.selection = if self.selection_reversed {
                target.start..origin.end
            } else {
                origin.start..target.end.max(origin.end)
            };
            self.selection_anchor = None;
        }
        if before != (self.selection.clone(), self.selection_reversed) {
            cx.notify();
        }
    }

    pub(super) fn mouse_up(&mut self) {
        self.selection_dragging = false;
    }

    /// The caret offset nearest a window point. A drag clamps points
    /// outside the field to its edges; a click outside the field has no
    /// offset.
    pub(super) fn index_for_point(&self, position: Point<Pixels>, clamp: bool) -> Option<usize> {
        let bounds = self.painted_bounds.filter(valid_bounds)?;
        if !valid_point(position) {
            return None;
        }
        let position = if clamp {
            let right = (bounds.right() - px(0.1)).max(bounds.left());
            let bottom = (bounds.bottom() - px(0.1)).max(bounds.top());
            point(
                position.x.clamp(bounds.left(), right),
                position.y.clamp(bounds.top(), bottom),
            )
        } else if bounds.contains(&position) {
            position
        } else {
            return None;
        };
        let layout = self.layout.as_ref()?;
        let layout_bounds = layout.bounds();
        let index = if position.y < layout_bounds.top() {
            0
        } else if position.y >= layout_bounds.bottom() {
            self.text.len()
        } else {
            caret_index_for_position(layout, position)
        };
        Some(floor_char_boundary(&self.text, index))
    }
}
