//! Editing actions, selection, and keyboard/mouse navigation for the native
//! composer text surface.
//!
//! Extracted verbatim from `native_composer.rs` during the module split;
//! visibility was widened to `pub(super)` for parent- and render-owned calls.

#![forbid(unsafe_code)]

use super::*;

impl NativeComposer {
    pub(super) fn undo_action(&mut self, _: &Undo, _: &mut Window, cx: &mut Context<Self>) {
        if self.state.is_disabled() || self.state.is_submitting() || self.marked_range.is_some() {
            return;
        }
        if let Some((draft, selection)) = self.undo.pop() {
            self.redo
                .push((self.state.draft().to_owned(), self.selection.clone()));
            self.state.set_draft(draft);
            self.draft_revision = self.draft_revision.saturating_add(1);
            self.selection = selection;
            self.selection_reversed = false;
            self.selection_anchor = None;
            self.clear_vertical_goal();
            self.selection_dragging = false;
            self.advance_selection_revision();
            self.layout = None;
            self.note_draft_change();
            cx.notify();
        }
    }

    pub(super) fn redo_action(&mut self, _: &Redo, _: &mut Window, cx: &mut Context<Self>) {
        if self.state.is_disabled() || self.state.is_submitting() || self.marked_range.is_some() {
            return;
        }
        if let Some((draft, selection)) = self.redo.pop() {
            self.undo
                .push((self.state.draft().to_owned(), self.selection.clone()));
            self.state.set_draft(draft);
            self.draft_revision = self.draft_revision.saturating_add(1);
            self.selection = selection;
            self.selection_reversed = false;
            self.selection_anchor = None;
            self.clear_vertical_goal();
            self.selection_dragging = false;
            self.advance_selection_revision();
            self.layout = None;
            self.note_draft_change();
            cx.notify();
        }
    }

    pub(super) fn request_send(&mut self, cx: &mut Context<Self>) {
        if self.send_ready() {
            cx.emit(NativeComposerEvent::SendRequested);
        }
    }

    pub(super) fn request_send_action(
        &mut self,
        _: &RequestSend,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.request_send(cx);
    }

    pub(super) fn replace_range(
        &mut self,
        range: Range<usize>,
        replacement: &str,
        marked_selection: Option<Range<usize>>,
        cx: &mut Context<Self>,
    ) {
        if self.state.is_disabled() {
            return;
        }
        let Some(next) =
            replace_text_preserving_raw(self.state.draft(), range.clone(), replacement)
        else {
            return;
        };
        let selected_offsets = match marked_selection {
            Some(selected_range) if selected_range.start <= selected_range.end => Some((
                match utf16_offset_to_utf8(replacement, selected_range.start) {
                    Some(offset) => offset,
                    None => return,
                },
                match utf16_offset_to_utf8(replacement, selected_range.end) {
                    Some(offset) => offset,
                    None => return,
                },
            )),
            Some(_) => return,
            None => None,
        };
        let changed = next != self.state.draft();
        if changed && self.marked_range.is_none() {
            if self.undo.len() >= 64 {
                self.undo.remove(0);
            }
            self.undo
                .push((self.state.draft().to_owned(), self.selection.clone()));
            self.redo.clear();
        }
        self.state.set_draft(next);
        if !replacement.is_empty() || !range.is_empty() {
            self.authored_text_present = true;
        }
        if changed {
            self.draft_revision = self.draft_revision.saturating_add(1);
        }
        self.advance_selection_revision();
        self.layout = None;
        self.painted_bounds = None;
        self.selection_anchor = None;
        self.clear_vertical_goal();
        self.selection_dragging = false;
        let replacement_end = range.start.saturating_add(replacement.len());
        if let Some((start, end)) = selected_offsets {
            self.selection = range.start + start..range.start + end;
            self.selection_reversed = false;
            self.marked_range = Some(range.start..replacement_end);
        } else {
            self.selection = replacement_end..replacement_end;
            self.selection_reversed = false;
            self.marked_range = None;
        }
        if changed {
            self.note_draft_change();
        }
        cx.notify();
    }

    pub(super) fn replacement_range(&self, range: Option<Range<usize>>) -> Option<Range<usize>> {
        let draft = self.state.draft();
        let range = match range {
            Some(range) => utf16_range_to_utf8(draft, range),
            None => self
                .marked_range
                .clone()
                .or_else(|| Some(self.selection.clone())),
        }?;
        (range.start <= range.end
            && range.end <= draft.len()
            && draft.is_char_boundary(range.start)
            && draft.is_char_boundary(range.end))
        .then_some(range)
    }

    pub(super) fn current_selection(&self) -> Range<usize> {
        self.selection.clone()
    }

    pub(super) fn begin_selection_drag(
        &mut self,
        point: Point<Pixels>,
        extend: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(byte_index) = self.byte_index_for_global_point(point) else {
            return;
        };
        if extend {
            self.select_to(byte_index);
        } else {
            self.selection = byte_index..byte_index;
            self.selection_reversed = false;
            self.selection_anchor = None;
            self.advance_selection_revision();
        }
        self.marked_range = None;
        self.clear_vertical_goal();
        self.selection_dragging = true;
        cx.notify();
    }

    pub(super) fn update_selection_drag(&mut self, point: Point<Pixels>, cx: &mut Context<Self>) {
        if !self.selection_dragging {
            return;
        }
        let Some(byte_index) = self.byte_index_for_drag_point(point) else {
            return;
        };
        let old_selection = self.selection.clone();
        let old_reversed = self.selection_reversed;
        self.select_to(byte_index);
        self.marked_range = None;
        self.clear_vertical_goal();
        if old_selection != self.selection || old_reversed != self.selection_reversed {
            cx.notify();
        }
    }

    pub(super) fn end_selection_drag(&mut self) {
        self.selection_dragging = false;
    }

    pub(crate) fn bind_actions(cx: &mut App) {
        cx.bind_keys([
            KeyBinding::new("ctrl-z", Undo, Some(NATIVE_COMPOSER_KEY_CONTEXT)),
            KeyBinding::new("cmd-z", Undo, Some(NATIVE_COMPOSER_KEY_CONTEXT)),
            KeyBinding::new("ctrl-shift-z", Redo, Some(NATIVE_COMPOSER_KEY_CONTEXT)),
            KeyBinding::new("cmd-shift-z", Redo, Some(NATIVE_COMPOSER_KEY_CONTEXT)),
            KeyBinding::new("ctrl-y", Redo, Some(NATIVE_COMPOSER_KEY_CONTEXT)),
            KeyBinding::new("backspace", Backspace, Some(NATIVE_COMPOSER_KEY_CONTEXT)),
            KeyBinding::new("delete", Delete, Some(NATIVE_COMPOSER_KEY_CONTEXT)),
            KeyBinding::new("left", Left, Some(NATIVE_COMPOSER_KEY_CONTEXT)),
            KeyBinding::new("right", Right, Some(NATIVE_COMPOSER_KEY_CONTEXT)),
            KeyBinding::new("shift-left", SelectLeft, Some(NATIVE_COMPOSER_KEY_CONTEXT)),
            KeyBinding::new(
                "shift-right",
                SelectRight,
                Some(NATIVE_COMPOSER_KEY_CONTEXT),
            ),
            KeyBinding::new("cmd-a", SelectAll, Some(NATIVE_COMPOSER_KEY_CONTEXT)),
            KeyBinding::new("ctrl-a", SelectAll, Some(NATIVE_COMPOSER_KEY_CONTEXT)),
            KeyBinding::new("cmd-v", Paste, Some(NATIVE_COMPOSER_KEY_CONTEXT)),
            KeyBinding::new("ctrl-v", Paste, Some(NATIVE_COMPOSER_KEY_CONTEXT)),
            KeyBinding::new("cmd-c", Copy, Some(NATIVE_COMPOSER_KEY_CONTEXT)),
            KeyBinding::new("ctrl-c", Copy, Some(NATIVE_COMPOSER_KEY_CONTEXT)),
            KeyBinding::new("cmd-x", Cut, Some(NATIVE_COMPOSER_KEY_CONTEXT)),
            KeyBinding::new("ctrl-x", Cut, Some(NATIVE_COMPOSER_KEY_CONTEXT)),
            KeyBinding::new("home", Home, Some(NATIVE_COMPOSER_KEY_CONTEXT)),
            KeyBinding::new("end", End, Some(NATIVE_COMPOSER_KEY_CONTEXT)),
            KeyBinding::new("up", Up, Some(NATIVE_COMPOSER_KEY_CONTEXT)),
            KeyBinding::new("down", Down, Some(NATIVE_COMPOSER_KEY_CONTEXT)),
            KeyBinding::new("shift-home", SelectHome, Some(NATIVE_COMPOSER_KEY_CONTEXT)),
            KeyBinding::new("shift-end", SelectEnd, Some(NATIVE_COMPOSER_KEY_CONTEXT)),
            KeyBinding::new("shift-up", SelectUp, Some(NATIVE_COMPOSER_KEY_CONTEXT)),
            KeyBinding::new("shift-down", SelectDown, Some(NATIVE_COMPOSER_KEY_CONTEXT)),
            KeyBinding::new("ctrl-home", DocumentHome, Some(NATIVE_COMPOSER_KEY_CONTEXT)),
            KeyBinding::new("ctrl-end", DocumentEnd, Some(NATIVE_COMPOSER_KEY_CONTEXT)),
            KeyBinding::new(
                "ctrl-shift-home",
                SelectDocumentHome,
                Some(NATIVE_COMPOSER_KEY_CONTEXT),
            ),
            KeyBinding::new(
                "ctrl-shift-end",
                SelectDocumentEnd,
                Some(NATIVE_COMPOSER_KEY_CONTEXT),
            ),
            KeyBinding::new("cmd-home", DocumentHome, Some(NATIVE_COMPOSER_KEY_CONTEXT)),
            KeyBinding::new("cmd-end", DocumentEnd, Some(NATIVE_COMPOSER_KEY_CONTEXT)),
            KeyBinding::new(
                "cmd-shift-home",
                SelectDocumentHome,
                Some(NATIVE_COMPOSER_KEY_CONTEXT),
            ),
            KeyBinding::new(
                "cmd-shift-end",
                SelectDocumentEnd,
                Some(NATIVE_COMPOSER_KEY_CONTEXT),
            ),
            KeyBinding::new("cmd-up", DocumentHome, Some(NATIVE_COMPOSER_KEY_CONTEXT)),
            KeyBinding::new("cmd-down", DocumentEnd, Some(NATIVE_COMPOSER_KEY_CONTEXT)),
            KeyBinding::new(
                "cmd-shift-up",
                SelectDocumentHome,
                Some(NATIVE_COMPOSER_KEY_CONTEXT),
            ),
            KeyBinding::new(
                "cmd-shift-down",
                SelectDocumentEnd,
                Some(NATIVE_COMPOSER_KEY_CONTEXT),
            ),
            KeyBinding::new("enter", RequestSend, Some(NATIVE_COMPOSER_KEY_CONTEXT)),
            KeyBinding::new(
                "shift-enter",
                InsertNewline,
                Some(NATIVE_COMPOSER_KEY_CONTEXT),
            ),
        ]);
    }

    fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selection.start
        } else {
            self.selection.end
        }
    }

    pub(super) fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.selection = offset..offset;
        self.selection_reversed = false;
        self.selection_anchor = None;
        self.clear_vertical_goal();
        self.marked_range = None;
        self.selection_dragging = false;
        self.advance_selection_revision();
        cx.notify();
    }

    fn select_to(&mut self, offset: usize) {
        let anchor = if let Some(anchor) = self.selection_anchor {
            anchor
        } else {
            let anchor = if self.selection.is_empty() {
                self.cursor_offset()
            } else if self.selection_reversed {
                self.selection.end
            } else {
                self.selection.start
            };
            self.selection_anchor = Some(anchor);
            anchor
        };
        if offset < anchor {
            self.selection = offset..anchor;
            self.selection_reversed = true;
        } else {
            self.selection = anchor..offset;
            self.selection_reversed = false;
        }
        self.advance_selection_revision();
    }

    pub(super) fn move_left(&mut self, extend: bool, cx: &mut Context<Self>) {
        let target = if !extend && !self.selection.is_empty() {
            self.selection.start
        } else {
            previous_character_boundary(self.state.draft(), self.cursor_offset())
        };
        if extend {
            self.select_to(target);
            self.marked_range = None;
            self.clear_vertical_goal();
            self.selection_dragging = false;
            cx.notify();
        } else {
            self.move_to(target, cx);
        }
    }

    pub(super) fn move_right(&mut self, extend: bool, cx: &mut Context<Self>) {
        let target = if !extend && !self.selection.is_empty() {
            self.selection.end
        } else {
            next_character_boundary(self.state.draft(), self.cursor_offset())
        };
        if extend {
            self.select_to(target);
            self.marked_range = None;
            self.clear_vertical_goal();
            self.selection_dragging = false;
            cx.notify();
        } else {
            self.move_to(target, cx);
        }
    }

    pub(super) fn delete_backward(
        &mut self,
        _: &Backspace,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.state.is_disabled() {
            return;
        }
        let range = self
            .marked_range
            .clone()
            .filter(|range| !range.is_empty())
            .or_else(|| (!self.selection.is_empty()).then_some(self.selection.clone()))
            .or_else(|| {
                let cursor = self.cursor_offset();
                (cursor > 0)
                    .then(|| previous_character_boundary(self.state.draft(), cursor)..cursor)
            });
        if let Some(range) = range {
            self.replace_range(range, "", None, cx);
        }
    }

    pub(super) fn delete_forward(&mut self, _: &Delete, _: &mut Window, cx: &mut Context<Self>) {
        if self.state.is_disabled() {
            return;
        }
        let range = self
            .marked_range
            .clone()
            .filter(|range| !range.is_empty())
            .or_else(|| (!self.selection.is_empty()).then_some(self.selection.clone()))
            .or_else(|| {
                let cursor = self.cursor_offset();
                (cursor < self.state.draft().len())
                    .then(|| cursor..next_character_boundary(self.state.draft(), cursor))
            });
        if let Some(range) = range {
            self.replace_range(range, "", None, cx);
        }
    }

    pub(super) fn move_left_action(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        if !self.state.is_disabled() {
            self.move_left(false, cx);
        }
    }

    pub(super) fn move_right_action(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        if !self.state.is_disabled() {
            self.move_right(false, cx);
        }
    }

    pub(super) fn select_left_action(
        &mut self,
        _: &SelectLeft,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.state.is_disabled() {
            self.move_left(true, cx);
        }
    }

    pub(super) fn select_right_action(
        &mut self,
        _: &SelectRight,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.state.is_disabled() {
            self.move_right(true, cx);
        }
    }

    pub(super) fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        if !self.state.is_disabled() {
            self.selection = 0..self.state.draft().len();
            self.selection_reversed = false;
            self.selection_anchor = None;
            self.clear_vertical_goal();
            self.selection_dragging = false;
            self.marked_range = None;
            self.advance_selection_revision();
            cx.notify();
        }
    }

    pub(super) fn move_home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        if !self.state.is_disabled() {
            let cursor = self.cursor_offset();
            let line_start = logical_line_start(self.state.draft(), cursor);
            self.move_to(line_start, cx);
        }
    }

    pub(super) fn move_end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        if !self.state.is_disabled() {
            let cursor = self.cursor_offset();
            let line_end = logical_line_end(self.state.draft(), cursor);
            self.move_to(line_end, cx);
        }
    }

    pub(super) fn move_up_action(&mut self, _: &Up, window: &mut Window, cx: &mut Context<Self>) {
        self.move_vertical(-1, window, false, cx);
    }

    pub(super) fn move_down_action(
        &mut self,
        _: &Down,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_vertical(1, window, false, cx);
    }

    pub(super) fn select_home_action(
        &mut self,
        _: &SelectHome,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.state.is_disabled() {
            return;
        }
        let target = logical_line_start(self.state.draft(), self.cursor_offset());
        self.select_to(target);
        self.marked_range = None;
        self.clear_vertical_goal();
        self.selection_dragging = false;
        cx.notify();
    }

    pub(super) fn select_end_action(
        &mut self,
        _: &SelectEnd,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.state.is_disabled() {
            return;
        }
        let target = logical_line_end(self.state.draft(), self.cursor_offset());
        self.select_to(target);
        self.marked_range = None;
        self.clear_vertical_goal();
        self.selection_dragging = false;
        cx.notify();
    }

    pub(super) fn move_document_home_action(
        &mut self,
        _: &DocumentHome,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.state.is_disabled() {
            self.move_to(0, cx);
        }
    }

    pub(super) fn move_document_end_action(
        &mut self,
        _: &DocumentEnd,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.state.is_disabled() {
            self.move_to(self.state.draft().len(), cx);
        }
    }

    pub(super) fn select_document_home_action(
        &mut self,
        _: &SelectDocumentHome,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.state.is_disabled() {
            return;
        }
        self.select_to(0);
        self.marked_range = None;
        self.clear_vertical_goal();
        self.selection_dragging = false;
        cx.notify();
    }

    pub(super) fn select_document_end_action(
        &mut self,
        _: &SelectDocumentEnd,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.state.is_disabled() {
            return;
        }
        self.select_to(self.state.draft().len());
        self.marked_range = None;
        self.clear_vertical_goal();
        self.selection_dragging = false;
        cx.notify();
    }

    pub(super) fn select_up_action(
        &mut self,
        _: &SelectUp,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_vertical(-1, window, true, cx);
    }

    pub(super) fn select_down_action(
        &mut self,
        _: &SelectDown,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_vertical(1, window, true, cx);
    }

    fn move_vertical(
        &mut self,
        direction: i32,
        _window: &mut Window,
        extend: bool,
        cx: &mut Context<Self>,
    ) {
        if self.state.is_disabled() {
            return;
        }

        let cursor = self.cursor_offset();
        let target = self.vertical_target(cursor, direction);
        if extend {
            self.select_to(target);
            self.marked_range = None;
            self.selection_dragging = false;
            cx.notify();
        } else {
            // A vertical move keeps its x/column goal across repeated Up/Down
            // presses, even when an intermediate line is shorter.
            self.selection = target..target;
            self.selection_reversed = false;
            self.selection_anchor = None;
            self.marked_range = None;
            self.selection_dragging = false;
            self.advance_selection_revision();
            cx.notify();
        }
    }

    #[expect(
        clippy::cast_precision_loss,
        reason = "the vertical direction is exactly ±1, which f32 represents exactly"
    )]
    fn vertical_target(&mut self, cursor: usize, direction: i32) -> usize {
        let draft = self.state.draft().to_owned();
        let goal_column = self.vertical_goal_column.unwrap_or_else(|| {
            let line_start = logical_line_start(&draft, cursor);
            utf8_offset_to_utf16(&draft, cursor)
                .unwrap_or_default()
                .saturating_sub(utf8_offset_to_utf16(&draft, line_start).unwrap_or_default())
        });
        self.vertical_goal_column = Some(goal_column);

        if self.painted_bounds.is_some()
            && let Some(layout) = self.layout.clone()
            && let Some(position) = layout.position_for_index(cursor)
        {
            let goal_x = self.vertical_goal_x.unwrap_or(position.x);
            self.vertical_goal_x = Some(goal_x);
            // `TextLayout::index_for_position` treats the exact bottom edge of
            // a row as belonging to that row. Aim at the center of the target
            // row so Down/Up cannot accidentally resolve back to the source
            // row at the shared line boundary.
            let target_y =
                position.y + layout.line_height() * direction as f32 + layout.line_height() / 2.0;
            let layout_bounds = layout.bounds();
            if target_y < layout_bounds.top() || target_y >= layout_bounds.bottom() {
                return cursor;
            }
            let target = match layout.index_for_position(point(goal_x, target_y)) {
                Ok(index) | Err(index) => index,
            };
            return previous_char_boundary(&draft, target.min(draft.len()));
        }

        logical_vertical_target(&draft, cursor, direction, goal_column)
    }

    pub(super) fn insert_newline(
        &mut self,
        _: &InsertNewline,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.state.is_disabled() {
            return;
        }
        let Some(range) = self.replacement_range(None) else {
            return;
        };
        self.replace_range(range, "\n", None, cx);
    }

    pub(super) fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        if self.state.is_disabled() {
            return;
        }
        self.prune_attachment_tasks();
        let scope = PasteScope {
            thread: self.draft_thread.clone(),
            draft_generation: self.draft_generation,
            selection_revision: self.selection_revision,
            replacement_range: self.replacement_range(None),
        };
        let clipboard = cx.read_from_clipboard_async();
        let task = cx.spawn(async move |this, cx| {
            let Ok(Some(item)) = clipboard.await else {
                return;
            };
            match native_composer_attachments::classify_clipboard(item) {
                ClipboardInput::Images(candidates) => {
                    this.update(cx, |composer, composer_cx| {
                        if !composer.paste_scope_is_current(&scope) {
                            return;
                        }
                        composer.enqueue_clipboard_images(candidates, composer_cx);
                    })
                    .ok();
                }
                ClipboardInput::Files(paths) => {
                    this.update(cx, |composer, composer_cx| {
                        if !composer.paste_scope_is_current(&scope) {
                            return;
                        }
                        composer.enqueue_file_drop(paths, composer_cx);
                    })
                    .ok();
                }
                ClipboardInput::Text(text) => {
                    let Some(range) = scope.replacement_range.clone() else {
                        return;
                    };
                    this.update(cx, |composer, composer_cx| {
                        if !composer.paste_scope_is_current(&scope) {
                            return;
                        }
                        composer.replace_range(range, &text, None, composer_cx);
                    })
                    .ok();
                }
                ClipboardInput::Empty => {}
            }
        });
        self.attachment_tasks.push(task);
    }

    pub(super) fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if self.state.is_disabled() || self.selection.is_empty() {
            return;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(
            self.state.draft()[self.selection.clone()].to_owned(),
        ));
    }

    pub(super) fn cut(&mut self, _: &Cut, _: &mut Window, cx: &mut Context<Self>) {
        if self.state.is_disabled() || self.selection.is_empty() {
            return;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(
            self.state.draft()[self.selection.clone()].to_owned(),
        ));
        let range = self.selection.clone();
        self.replace_range(range, "", None, cx);
    }

    pub(super) fn byte_index_for_global_point(&self, point: Point<Pixels>) -> Option<usize> {
        let bounds = self.painted_bounds.as_ref()?;
        // TextLayout positions are already relative to the window in GPUI's
        // prepaint pass. Keep the editor hit-test guard, but do not localize
        // the point a second time before asking the layout for its index.
        if !valid_bounds(bounds) || !valid_point(point) || !bounds.contains(&point) {
            return None;
        }
        let layout = self.layout.as_ref()?;
        let layout_bounds = layout.bounds();
        let index = if point.y < layout_bounds.top() {
            0
        } else if point.y >= layout_bounds.bottom() {
            self.state.draft().len()
        } else {
            match layout.index_for_position(point) {
                Ok(index) | Err(index) => index,
            }
        };
        (index <= self.state.draft().len())
            .then(|| previous_char_boundary(self.state.draft(), index))
    }

    fn byte_index_for_drag_point(&self, global_point: Point<Pixels>) -> Option<usize> {
        let bounds = self.painted_bounds.as_ref()?;
        if !valid_bounds(bounds) || !valid_point(global_point) {
            return None;
        }

        // Keep the endpoint inside the layout viewport while the pointer is
        // outside the field. This gives ordinary editor behavior at the top
        // and bottom edges without manufacturing an offset outside the text.
        let left = bounds.left();
        let top = bounds.top();
        let right = bounds.right();
        let bottom = bounds.bottom();
        let x = if global_point.x < left {
            left
        } else if global_point.x >= right {
            if right > left { right - px(0.1) } else { left }
        } else {
            global_point.x
        };
        let y = if global_point.y < top {
            top
        } else if global_point.y >= bottom {
            if bottom > top { bottom - px(0.1) } else { top }
        } else {
            global_point.y
        };
        let point = point(x, y);
        let layout = self.layout.as_ref()?;
        let layout_bounds = layout.bounds();
        let index = if point.y < layout_bounds.top() {
            0
        } else if point.y >= layout_bounds.bottom() {
            self.state.draft().len()
        } else {
            match layout.index_for_position(point) {
                Ok(index) | Err(index) => index,
            }
        };
        (index <= self.state.draft().len())
            .then(|| previous_char_boundary(self.state.draft(), index))
    }
}
