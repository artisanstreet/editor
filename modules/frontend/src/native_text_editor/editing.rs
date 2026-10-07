//! Editing, selection, clipboard, undo/redo, and keyboard/mouse navigation
//! for the native text editor, adapted from the composer's editing module.

#![forbid(unsafe_code)]

use std::ops::Range;

use gpui::{App, ClipboardItem, Context, KeyBinding, Pixels, Point, Window, point, px};

use super::text::{
    floor_char_boundary, logical_column, logical_line_end, logical_line_start,
    logical_vertical_target, next_character_boundary, previous_character_boundary, splice,
    truncate_to_bytes, utf16_offset_to_utf8, utf16_range_to_utf8, valid_bounds, valid_point,
    word_range_at,
};
use super::{
    Backspace, Copy, Cut, Delete, DocumentEnd, DocumentHome, Down, End, Home, InsertNewline,
    InsertTab, Left, NATIVE_TEXT_EDITOR_KEY_CONTEXT, NativeTextEditor, NativeTextEditorEvent,
    Paste, Redo, Right, SelectAll, SelectDocumentEnd, SelectDocumentHome, SelectDown, SelectEnd,
    SelectHome, SelectLeft, SelectRight, SelectUp, Snapshot, TAB_TEXT, UNDO_LIMIT, Undo, Up,
};

impl NativeTextEditor {
    /// Registers the editor's key bindings. They apply only inside the
    /// [`NATIVE_TEXT_EDITOR_KEY_CONTEXT`] key context, so they never collide
    /// with the composer's or the workspace's bindings.
    pub fn bind_actions(cx: &mut App) {
        let context = Some(NATIVE_TEXT_EDITOR_KEY_CONTEXT);
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
            KeyBinding::new("shift-enter", InsertNewline, context),
            KeyBinding::new("tab", InsertTab, context),
        ]);
    }

    /// Replaces `range` with `replacement` as one user edit.
    ///
    /// The replacement is truncated at a character boundary so the text
    /// stays within [`Self::MAX_BYTES`]; an edit where nothing fits is
    /// refused. `marked_selection` (UTF-16, relative to the replacement)
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
        let retained = self.text.len().saturating_sub(range.len());
        let fitted = truncate_to_bytes(replacement, Self::MAX_BYTES.saturating_sub(retained));
        if fitted.is_empty() && !replacement.is_empty() {
            return;
        }
        // A truncated composition is committed rather than left marked.
        let marked_selection = marked_selection.filter(|_| fitted.len() == replacement.len());
        let Some(next) = splice(&self.text, range.clone(), fitted) else {
            return;
        };
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
        let changed = next != self.text;
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

        self.text = next;
        self.shared_text = None;
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
        if changed {
            cx.emit(NativeTextEditorEvent::Changed);
        }
        cx.notify();
    }

    fn push_undo(&mut self) {
        if self.undo.len() >= UNDO_LIMIT {
            self.undo.remove(0);
        }
        self.undo.push(Snapshot {
            text: self.text.clone(),
            selection: self.selection.clone(),
        });
    }

    fn restore(&mut self, snapshot: Snapshot, cx: &mut Context<Self>) {
        self.text = snapshot.text;
        self.selection = snapshot.selection;
        self.shared_text = None;
        self.selection_reversed = false;
        self.selection_anchor = None;
        self.selection_dragging = false;
        self.marked_range = None;
        self.reset_transient_state();
        self.layout = None;
        self.painted_bounds = None;
        cx.emit(NativeTextEditorEvent::Changed);
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
        self.insert("\n", cx);
    }

    pub(super) fn insert_tab(&mut self, _: &InsertTab, _: &mut Window, cx: &mut Context<Self>) {
        self.insert(TAB_TEXT, cx);
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
        cx.notify();
    }

    pub(super) fn move_left(&mut self, extend: bool, cx: &mut Context<Self>) {
        if self.disabled {
            return;
        }
        if extend {
            let target = previous_character_boundary(&self.text, self.cursor_offset());
            self.extend_to(target, cx);
        } else if self.selection.is_empty() {
            let target = previous_character_boundary(&self.text, self.cursor_offset());
            self.move_to(target, cx);
        } else {
            self.move_to(self.selection.start, cx);
        }
    }

    pub(super) fn move_right(&mut self, extend: bool, cx: &mut Context<Self>) {
        if self.disabled {
            return;
        }
        if extend {
            let target = next_character_boundary(&self.text, self.cursor_offset());
            self.extend_to(target, cx);
        } else if self.selection.is_empty() {
            let target = next_character_boundary(&self.text, self.cursor_offset());
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

    fn delete_toward(&mut self, forward: bool, cx: &mut Context<Self>) {
        if self.disabled {
            return;
        }
        let range = self
            .marked_range
            .clone()
            .filter(|range| !range.is_empty())
            .or_else(|| (!self.selection.is_empty()).then(|| self.selection.clone()))
            .or_else(|| {
                let cursor = self.cursor_offset();
                if forward {
                    (cursor < self.text.len())
                        .then(|| cursor..next_character_boundary(&self.text, cursor))
                } else {
                    (cursor > 0).then(|| previous_character_boundary(&self.text, cursor)..cursor)
                }
            });
        if let Some(range) = range {
            self.replace_range(range, "", None, cx);
        }
    }

    pub(super) fn backspace(&mut self, _: &Backspace, _: &mut Window, cx: &mut Context<Self>) {
        self.delete_toward(false, cx);
    }

    pub(super) fn delete(&mut self, _: &Delete, _: &mut Window, cx: &mut Context<Self>) {
        self.delete_toward(true, cx);
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
        self.move_vertical(true, false, cx);
    }

    pub(super) fn down(&mut self, _: &Down, _: &mut Window, cx: &mut Context<Self>) {
        self.move_vertical(false, false, cx);
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
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
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
    /// a double click selects a word and a triple click a line.
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
        match click_count {
            0 | 1 => {
                if shift {
                    self.select_to(index);
                } else {
                    self.selection = index..index;
                    self.selection_reversed = false;
                    self.selection_anchor = None;
                }
                self.selection_dragging = true;
            }
            2 => {
                self.selection = word_range_at(&self.text, index);
                self.selection_reversed = false;
                self.selection_anchor = None;
                self.selection_dragging = false;
            }
            _ => {
                let start = logical_line_start(&self.text, index);
                let end = logical_line_end(&self.text, index);
                self.selection = start..end;
                self.selection_reversed = false;
                self.selection_anchor = None;
                self.selection_dragging = false;
            }
        }
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
        self.select_to(index);
        if before != (self.selection.clone(), self.selection_reversed) {
            cx.notify();
        }
    }

    pub(super) fn mouse_up(&mut self) {
        self.selection_dragging = false;
    }

    /// The text offset under a window point. A drag clamps points outside
    /// the field to its edges; a click outside the field has no offset.
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
            match layout.index_for_position(position) {
                Ok(index) | Err(index) => index,
            }
        };
        Some(floor_char_boundary(&self.text, index))
    }
}
