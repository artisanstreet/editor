use std::cell::RefCell;
use std::ops::Range;
use std::path::PathBuf;
use std::rc::Rc;

use aqua_ui::input::{ClipboardPaste, PasteImage};

const MAX_BYTES: usize = 64 * 1024;
const UNDO_LIMIT: usize = 64;
const HISTORY_BYTES: usize = 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mode {
    Document,
    SingleLine,
    Prompt,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ListKey {
    Up,
    Down,
    Accept,
    Dismiss,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EditOutcome {
    Unchanged,
    Changed,
    LimitReached,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommandOutcome {
    Edit(EditOutcome),
    Submitted,
    List(ListKey),
    Propagate,
    Ignored,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PasteOutcome {
    Edit(EditOutcome),
    Files {
        images: Vec<PasteImage>,
        paths: Vec<PathBuf>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TokenRange {
    pub range: Range<usize>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StyledToken {
    pub range: Range<usize>,
    pub color: Option<crate::ui::theme::Oklch>,
    pub font_weight: Option<aqua_text::FontWeight>,
    pub glyph: Option<editor_assets::AssetId>,
}

pub type StyledTokenizer = Rc<dyn Fn(&str) -> Vec<StyledToken>>;

#[derive(Clone, Debug)]
struct Snapshot {
    text: String,
    selection: Range<usize>,
}

pub struct Editor {
    text: String,
    paint_revision: u64,
    mode: Mode,
    selection: Range<usize>,
    reversed: bool,
    anchor: Option<usize>,
    marked: Option<Range<usize>>,
    disabled: bool,
    limit_notice: bool,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    typing_group_end: Option<usize>,
    tokenizer: Option<StyledTokenizer>,
    token_cache: RefCell<Option<Vec<StyledToken>>>,
    list_open: bool,
    suggestion: Option<String>,
    drag_origin: Option<(aqua_text::navigation::SelectionUnit, Range<usize>)>,
    vertical_goal_column: Option<usize>,
}

impl Editor {
    pub fn new(text: &str, mode: Mode) -> Self {
        let limit_notice = text.len() > MAX_BYTES;
        let mut text = truncate(text, MAX_BYTES).to_owned();
        if mode == Mode::SingleLine {
            text = text.replace(['\r', '\n', '\t'], " ");
        }
        let end = text.len();
        Self {
            text,
            paint_revision: 0,
            mode,
            selection: end..end,
            reversed: false,
            anchor: None,
            marked: None,
            disabled: false,
            limit_notice,
            undo: Vec::new(),
            redo: Vec::new(),
            typing_group_end: None,
            tokenizer: None,
            token_cache: RefCell::new(None),
            list_open: false,
            suggestion: None,
            drag_origin: None,
            vertical_goal_column: None,
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }
    pub fn paint_revision(&self) -> u64 {
        self.paint_revision
    }
    pub fn is_disabled(&self) -> bool {
        self.disabled
    }
    pub fn selection(&self) -> Range<usize> {
        self.selection.clone()
    }
    pub fn selection_reversed(&self) -> bool {
        self.reversed
    }
    pub fn marked_range(&self) -> Option<Range<usize>> {
        self.marked.clone()
    }
    pub fn limit_notice(&self) -> bool {
        self.limit_notice
    }
    pub fn caret(&self) -> Option<usize> {
        (self.selection.is_empty() && self.marked.is_none()).then_some(self.selection.end)
    }
    pub fn set_list_open(&mut self, open: bool) {
        self.list_open = open;
    }
    pub fn set_suggestion(&mut self, suggestion: Option<String>) {
        self.suggestion = suggestion;
    }
    pub fn shown_suggestion(&self) -> Option<&str> {
        self.suggestion
            .as_deref()
            .filter(|_| self.mode == Mode::Prompt && self.text.is_empty() && !self.disabled)
    }
    pub fn set_disabled(&mut self, disabled: bool) {
        if self.disabled != disabled {
            self.disabled = disabled;
            self.drag_origin = None;
            self.marked = None;
            self.typing_group_end = None;
            self.vertical_goal_column = None;
        }
    }
    pub fn set_tokenizer(&mut self, tokenizer: Option<Rc<dyn Fn(&str) -> Vec<TokenRange>>>) {
        *self.token_cache.get_mut() = None;
        self.tokenizer = tokenizer.map(|tokenize| {
            Rc::new(move |text: &str| {
                tokenize(text)
                    .into_iter()
                    .map(|token| StyledToken {
                        range: token.range,
                        color: None,
                        font_weight: None,
                        glyph: None,
                    })
                    .collect()
            }) as StyledTokenizer
        });
    }
    pub fn set_styled_tokenizer(&mut self, tokenizer: Option<StyledTokenizer>) {
        self.tokenizer = tokenizer;
        *self.token_cache.get_mut() = None;
    }
    pub fn styled_tokens(&self) -> Vec<StyledToken> {
        if let Some(tokens) = self.token_cache.borrow().as_ref() {
            return tokens.clone();
        }
        let mut end = 0;
        let tokens: Vec<_> = self
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
        *self.token_cache.borrow_mut() = Some(tokens.clone());
        tokens
    }
    pub fn tokens(&self) -> Vec<TokenRange> {
        self.styled_tokens()
            .into_iter()
            .map(|token| TokenRange { range: token.range })
            .collect()
    }
    pub fn set_text(&mut self, text: &str) {
        let normalized;
        let text = if self.mode == Mode::SingleLine {
            normalized = text.replace(['\r', '\n', '\t'], " ");
            &normalized
        } else {
            text
        };
        self.limit_notice = text.len() > MAX_BYTES;
        self.text = truncate(text, MAX_BYTES).to_owned();
        self.paint_revision = self.paint_revision.wrapping_add(1);
        *self.token_cache.get_mut() = None;
        let end = self.text.len();
        self.selection = end..end;
        self.reversed = false;
        self.anchor = None;
        self.marked = None;
        self.typing_group_end = None;
        self.vertical_goal_column = None;
        self.undo.clear();
        self.redo.clear();
        self.drag_origin = None;
    }
    pub fn set_selection(&mut self, range: Range<usize>, reversed: bool) -> bool {
        if range.start > range.end || self.text.get(range.clone()).is_none() {
            return false;
        }
        self.selection = range;
        self.reversed = reversed;
        self.drag_origin = None;
        self.anchor = None;
        self.marked = None;
        self.typing_group_end = None;
        self.vertical_goal_column = None;
        true
    }
    pub fn select_all(&mut self) {
        if self.disabled {
            return;
        }
        self.marked = None;
        self.drag_origin = None;
        self.selection = 0..self.text.len();
        self.reversed = false;
        self.anchor = None;
        self.typing_group_end = None;
        self.vertical_goal_column = None;
    }
    pub fn replacement_range(&self, range: Option<Range<usize>>) -> Option<Range<usize>> {
        let range = match range {
            Some(range) => utf16_range_to_utf8(&self.text, range)?,
            None => self
                .marked
                .clone()
                .unwrap_or_else(|| self.selection.clone()),
        };
        (range.start <= range.end && self.text.get(range.clone()).is_some()).then_some(range)
    }
    pub fn replace_input(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        selected_utf16: Option<Range<usize>>,
    ) -> EditOutcome {
        match self.replacement_range(range) {
            Some(range) => self.replace_range(range, text, selected_utf16),
            None => EditOutcome::Unchanged,
        }
    }
    pub fn replace(&mut self, range: Range<usize>, text: &str) -> EditOutcome {
        if range.start > range.end || self.text.get(range.clone()).is_none() {
            return EditOutcome::Unchanged;
        }
        self.typing_group_end = None;
        self.vertical_goal_column = None;
        self.replace_range(range, text, None)
    }
    fn replace_range(
        &mut self,
        range: Range<usize>,
        replacement: &str,
        marked_selection: Option<Range<usize>>,
    ) -> EditOutcome {
        if self.disabled {
            return EditOutcome::Unchanged;
        }
        let normalized;
        let replacement =
            if self.mode == Mode::SingleLine && replacement.contains(['\r', '\n', '\t']) {
                normalized = replacement.replace(['\r', '\n', '\t'], " ");
                &normalized
            } else {
                replacement
            };
        let retained = self.text.len().saturating_sub(range.len());
        if replacement.len() > MAX_BYTES.saturating_sub(retained) {
            self.limit_notice = true;
            return EditOutcome::LimitReached;
        }
        self.limit_notice = false;
        if range.start > range.end || self.text.get(range.clone()).is_none() {
            return EditOutcome::Unchanged;
        }
        let selected = match marked_selection {
            Some(selected) => match utf16_range_to_utf8(replacement, selected) {
                Some(range) => Some(range),
                None => return EditOutcome::Unchanged,
            },
            None => None,
        };
        let changed = self.text.get(range.clone()) != Some(replacement);
        let replacement_end = range.start + replacement.len();
        let mut chars = replacement.chars();
        let single = chars.next().is_some() && chars.next().is_none();
        let typing = range.is_empty()
            && selected.is_none()
            && single
            && !replacement.contains(char::is_whitespace);
        if changed
            && self.marked.is_none()
            && !(typing && self.typing_group_end == Some(range.start))
        {
            self.undo.push(self.snapshot());
            self.bound_history();
        }
        if changed {
            self.redo.clear();
        }
        self.text.replace_range(range.clone(), replacement);
        self.paint_revision = self.paint_revision.wrapping_add(1);
        *self.token_cache.get_mut() = None;
        self.anchor = None;
        self.drag_origin = None;
        self.typing_group_end = None;
        self.vertical_goal_column = None;
        if let Some(selected) = selected {
            self.selection = range.start + selected.start..range.start + selected.end;
            self.marked = Some(range.start..replacement_end);
        } else {
            self.selection = replacement_end..replacement_end;
            self.marked = None;
            if range.is_empty() && single && replacement != "\n" {
                self.typing_group_end = Some(replacement_end);
            }
        }
        self.reversed = false;
        if changed {
            EditOutcome::Changed
        } else {
            EditOutcome::Unchanged
        }
    }
    pub fn unmark_text(&mut self) {
        self.marked = None;
    }
    pub fn selected_utf16(&self) -> Option<Range<usize>> {
        utf8_range_to_utf16(&self.text, self.selection.clone())
    }
    pub fn marked_utf16(&self) -> Option<Range<usize>> {
        utf8_range_to_utf16(&self.text, self.marked.clone()?)
    }
    pub fn text_for_utf16_range(&self, range: Range<usize>) -> Option<&str> {
        self.text.get(utf16_range_to_utf8(&self.text, range)?)
    }
    pub fn paste(&mut self, item: &ClipboardPaste) -> PasteOutcome {
        if self.disabled {
            return PasteOutcome::Edit(EditOutcome::Unchanged);
        }
        if self.mode == Mode::Prompt && (item.image.is_some() || !item.paths.is_empty()) {
            return PasteOutcome::Files {
                images: item.image.iter().cloned().collect(),
                paths: item.paths.to_vec(),
            };
        }
        let text = item
            .text
            .as_deref()
            .filter(|text| !text.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| {
                item.paths
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect()
            });
        if text.is_empty() {
            return PasteOutcome::Edit(EditOutcome::Unchanged);
        }
        let text = text.replace("\r\n", "\n");
        self.typing_group_end = None;
        self.vertical_goal_column = None;
        PasteOutcome::Edit(self.replace_input(None, &text, None))
    }
    pub fn copy(&self) -> Option<&str> {
        (!self.selection.is_empty())
            .then(|| self.text.get(self.selection.clone()))
            .flatten()
    }
    pub fn cut(&mut self) -> Option<String> {
        if self.disabled {
            return None;
        }
        let text = self.copy()?.to_owned();
        self.replace_range(self.selection.clone(), "", None);
        Some(text)
    }
    pub fn token_target(&self, target: usize, forward: bool) -> usize {
        self.tokens()
            .into_iter()
            .find(|token| token.range.start < target && target < token.range.end)
            .map_or(target, |token| {
                if forward {
                    token.range.end
                } else {
                    token.range.start
                }
            })
    }
    pub fn token_deletion(&self, cursor: usize, forward: bool) -> Option<Range<usize>> {
        self.tokens()
            .into_iter()
            .find(|token| {
                (if forward {
                    token.range.start == cursor
                } else {
                    token.range.end == cursor
                }) || (token.range.start < cursor && cursor < token.range.end)
            })
            .map(|token| token.range)
    }
    pub fn enter(&mut self, shift: bool) -> CommandOutcome {
        if self.disabled {
            return CommandOutcome::Ignored;
        }
        if shift {
            if self.mode == Mode::SingleLine {
                return CommandOutcome::Submitted;
            }
            return CommandOutcome::Edit(self.replace_input(None, "\n", None));
        }
        if self.mode == Mode::Prompt && self.marked.is_some() {
            return CommandOutcome::Ignored;
        }
        if self.list_open {
            return CommandOutcome::List(ListKey::Accept);
        }
        match self.mode {
            Mode::Document => CommandOutcome::Edit(self.replace_input(None, "\n", None)),
            Mode::SingleLine | Mode::Prompt => CommandOutcome::Submitted,
        }
    }
    pub fn tab(&mut self) -> CommandOutcome {
        if self.disabled {
            return CommandOutcome::Ignored;
        }
        if self.list_open {
            return CommandOutcome::List(ListKey::Accept);
        }
        if let Some(suggestion) = self.shown_suggestion().map(str::to_owned) {
            return CommandOutcome::Edit(self.replace_input(None, &suggestion, None));
        }
        match self.mode {
            Mode::Document => CommandOutcome::Edit(self.replace_input(None, "  ", None)),
            Mode::SingleLine => CommandOutcome::Ignored,
            Mode::Prompt => CommandOutcome::Propagate,
        }
    }
    pub fn list_vertical(&self, up: bool) -> Option<ListKey> {
        self.list_open
            .then_some(if up { ListKey::Up } else { ListKey::Down })
    }
    pub fn dismiss(&self) -> CommandOutcome {
        if self.disabled {
            CommandOutcome::Ignored
        } else if self.list_open {
            CommandOutcome::List(ListKey::Dismiss)
        } else {
            CommandOutcome::Propagate
        }
    }
    pub fn move_horizontal(&mut self, forward: bool, word: bool, extend: bool) {
        if self.disabled {
            return;
        }
        let cursor = if self.reversed {
            self.selection.start
        } else {
            self.selection.end
        };
        let target = if word {
            let boundaries = aqua_text::navigation::WordBoundaries::new(&self.text);
            if forward {
                boundaries.next(
                    &self.text,
                    cursor,
                    aqua_text::navigation::WordBehavior::platform(),
                )
            } else {
                boundaries.previous(&self.text, cursor)
            }
        } else {
            let model = aqua_text::TextModel::new(&self.text);
            let target = if forward {
                model.next_grapheme(cursor)
            } else {
                model.prev_grapheme(cursor)
            };
            self.token_target(target, forward)
        };
        let target = if !word && !extend && !self.selection.is_empty() {
            if forward {
                self.selection.end
            } else {
                self.selection.start
            }
        } else {
            target
        };
        if extend {
            let anchor =
                *self
                    .anchor
                    .get_or_insert(if self.selection.is_empty() || self.reversed {
                        self.selection.end
                    } else {
                        self.selection.start
                    });
            self.selection = target.min(anchor)..target.max(anchor);
            self.reversed = target < anchor;
        } else {
            self.selection = target..target;
            self.reversed = false;
            self.anchor = None;
        }
        self.marked = None;
        self.typing_group_end = None;
        self.vertical_goal_column = None;
    }
    fn select_to(&mut self, target: usize) {
        let target = self.text.floor_char_boundary(target.min(self.text.len()));
        let anchor = *self
            .anchor
            .get_or_insert(if self.selection.is_empty() || self.reversed {
                self.selection.end
            } else {
                self.selection.start
            });
        self.selection = target.min(anchor)..target.max(anchor);
        self.reversed = target < anchor;
        self.typing_group_end = None;
        self.vertical_goal_column = None;
    }
    pub fn pointer_down(&mut self, index: usize, shift: bool, click_count: usize) {
        if self.disabled {
            return;
        }
        let index = self.text.floor_char_boundary(index.min(self.text.len()));
        self.marked = None;
        self.typing_group_end = None;
        self.vertical_goal_column = None;
        let unit = aqua_text::navigation::SelectionUnit::from_click_count(click_count);
        if unit == aqua_text::navigation::SelectionUnit::Character && shift {
            self.select_to(index);
        } else {
            self.selection = if unit == aqua_text::navigation::SelectionUnit::Word {
                aqua_text::navigation::WordBoundaries::new(&self.text)
                    .selection_range(unit, &self.text, index, false)
            } else {
                unit.range_at(&self.text, index)
            };
            self.reversed = false;
            self.anchor = None;
        }
        self.drag_origin = Some((unit, self.selection.clone()));
    }
    pub fn pointer_drag(&mut self, index: usize) {
        let Some((unit, origin)) = self.drag_origin.clone() else {
            return;
        };
        let index = self.text.floor_char_boundary(index.min(self.text.len()));
        if unit == aqua_text::navigation::SelectionUnit::Character {
            self.select_to(index);
        } else {
            let target = aqua_text::navigation::WordBoundaries::new(&self.text)
                .selection_range(unit, &self.text, index, true);
            self.reversed = target.start < origin.start;
            self.selection = if self.reversed {
                target.start..origin.end
            } else {
                origin.start..target.end.max(origin.end)
            };
            self.anchor = None;
        }
    }
    pub fn pointer_up(&mut self) {
        self.drag_origin = None;
    }
    pub fn move_line_edge(&mut self, end: bool, document: bool, extend: bool) {
        if self.disabled {
            return;
        }
        let cursor = if self.reversed {
            self.selection.start
        } else {
            self.selection.end
        };
        let target = match (document, end) {
            (true, false) => 0,
            (true, true) => self.text.len(),
            (false, false) => self.text[..cursor].rfind('\n').map_or(0, |index| index + 1),
            (false, true) => self.text[cursor..]
                .find('\n')
                .map_or(self.text.len(), |index| cursor + index),
        };
        if extend {
            self.select_to(target);
        } else {
            self.selection = target..target;
            self.reversed = false;
            self.anchor = None;
        }
        self.marked = None;
        self.drag_origin = None;
        self.typing_group_end = None;
        self.vertical_goal_column = None;
    }
    pub fn move_vertical(&mut self, up: bool, extend: bool, visual_target: Option<usize>) {
        if self.disabled {
            return;
        }
        let cursor = if self.reversed {
            self.selection.start
        } else {
            self.selection.end
        };
        let start = self.text[..cursor].rfind('\n').map_or(0, |index| index + 1);
        let column = *self
            .vertical_goal_column
            .get_or_insert_with(|| self.text[start..cursor].encode_utf16().count());
        let mut target = visual_target
            .map(|index| self.text.floor_char_boundary(index.min(self.text.len())))
            .unwrap_or_else(|| logical_vertical_target(&self.text, cursor, up, column));
        if target == cursor {
            target = if up { 0 } else { self.text.len() };
        }
        if extend {
            self.select_to(target);
        } else {
            self.selection = target..target;
            self.reversed = false;
            self.anchor = None;
        }
        self.vertical_goal_column = Some(column);
        self.typing_group_end = None;
        self.marked = None;
        self.drag_origin = None;
    }
    pub fn delete_toward(&mut self, forward: bool, word: bool) -> EditOutcome {
        if self.disabled {
            return EditOutcome::Unchanged;
        }
        let cursor = if self.reversed {
            self.selection.start
        } else {
            self.selection.end
        };
        let range = self
            .marked
            .clone()
            .filter(|range| !range.is_empty())
            .or_else(|| (!self.selection.is_empty()).then(|| self.selection.clone()))
            .or_else(|| self.token_deletion(cursor, forward))
            .or_else(|| {
                let target = if word {
                    let boundaries = aqua_text::navigation::WordBoundaries::new(&self.text);
                    if forward {
                        boundaries.next(
                            &self.text,
                            cursor,
                            aqua_text::navigation::WordBehavior::platform(),
                        )
                    } else {
                        boundaries.previous(&self.text, cursor)
                    }
                } else {
                    let model = aqua_text::TextModel::new(&self.text);
                    if forward {
                        model.next_grapheme(cursor)
                    } else {
                        model.prev_grapheme(cursor)
                    }
                };
                let range = cursor.min(target)..cursor.max(target);
                (!range.is_empty()).then_some(range)
            });
        match range {
            Some(range) => self.replace_range(range, "", None),
            None => EditOutcome::Unchanged,
        }
    }
    pub fn undo(&mut self) -> bool {
        if self.disabled || self.marked.is_some() {
            return false;
        }
        let Some(snapshot) = self.undo.pop() else {
            return false;
        };
        self.redo.push(self.snapshot());
        self.bound_history();
        self.restore(snapshot);
        true
    }
    pub fn redo(&mut self) -> bool {
        if self.disabled || self.marked.is_some() {
            return false;
        }
        let Some(snapshot) = self.redo.pop() else {
            return false;
        };
        self.undo.push(self.snapshot());
        self.bound_history();
        self.restore(snapshot);
        true
    }
    fn snapshot(&self) -> Snapshot {
        Snapshot {
            text: self.text.clone(),
            selection: self.selection.clone(),
        }
    }
    fn restore(&mut self, snapshot: Snapshot) {
        self.text = snapshot.text;
        self.paint_revision = self.paint_revision.wrapping_add(1);
        *self.token_cache.get_mut() = None;
        self.selection = snapshot.selection;
        self.reversed = false;
        self.anchor = None;
        self.marked = None;
        self.drag_origin = None;
        self.typing_group_end = None;
        self.vertical_goal_column = None;
    }
    fn bound_history(&mut self) {
        let mut bytes: usize = self
            .undo
            .iter()
            .chain(&self.redo)
            .map(|snapshot| snapshot.text.len())
            .sum();
        while self.undo.len() + self.redo.len() > UNDO_LIMIT || bytes > HISTORY_BYTES {
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
}

fn truncate(text: &str, max: usize) -> &str {
    &text[..text.floor_char_boundary(max.min(text.len()))]
}

pub fn utf16_offset_to_utf8(text: &str, target: usize) -> Option<usize> {
    let mut offset = 0usize;
    for (byte, character) in text.char_indices() {
        if offset == target {
            return Some(byte);
        }
        offset = offset.checked_add(character.len_utf16())?;
        if offset > target {
            return None;
        }
    }
    (offset == target).then_some(text.len())
}
pub fn utf16_range_to_utf8(text: &str, range: Range<usize>) -> Option<Range<usize>> {
    if range.start > range.end {
        return None;
    }
    Some(utf16_offset_to_utf8(text, range.start)?..utf16_offset_to_utf8(text, range.end)?)
}
pub fn utf8_range_to_utf16(text: &str, range: Range<usize>) -> Option<Range<usize>> {
    if range.start > range.end {
        return None;
    }
    Some(
        text.get(..range.start)?.encode_utf16().count()
            ..text.get(..range.end)?.encode_utf16().count(),
    )
}

impl aqua_text::input_service::TextInputService for Editor {
    fn text(&self) -> &str {
        self.text()
    }

    fn selection(&self) -> aqua_text::Selection {
        if self.reversed {
            aqua_text::Selection::new(self.selection.end, self.selection.start)
        } else {
            aqua_text::Selection::new(self.selection.start, self.selection.end)
        }
    }

    fn marked_range(&self) -> Option<Range<usize>> {
        self.marked_range()
    }

    fn text_for_utf16_range(&self, range: Range<usize>) -> Option<&str> {
        Editor::text_for_utf16_range(self, range)
    }

    fn selected_utf16_range(&self, ignore_disabled: bool) -> Option<(Range<usize>, bool)> {
        if self.disabled && !ignore_disabled {
            return None;
        }
        Some((self.selected_utf16()?, self.reversed))
    }

    fn marked_utf16_range(&self) -> Option<Range<usize>> {
        self.marked_utf16()
    }

    fn replace_utf16_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        selected_utf16: Option<Range<usize>>,
    ) -> aqua_text::input_service::InputChange {
        match self.replace_input(range, text, selected_utf16) {
            EditOutcome::Unchanged => aqua_text::input_service::InputChange::Unchanged,
            EditOutcome::Changed => aqua_text::input_service::InputChange::Changed,
            EditOutcome::LimitReached => aqua_text::input_service::InputChange::Rejected,
        }
    }

    fn unmark_text(&mut self) {
        Editor::unmark_text(self);
    }
}

fn logical_vertical_target(text: &str, cursor: usize, up: bool, goal_column: usize) -> usize {
    let cursor = text.floor_char_boundary(cursor.min(text.len()));
    let line_start = text[..cursor].rfind('\n').map_or(0, |index| index + 1);
    let line_end = text[cursor..]
        .find('\n')
        .map_or(text.len(), |index| cursor + index);
    let (target_start, target_end) = if up {
        if line_start == 0 {
            return cursor;
        }
        let previous_end = line_start - 1;
        (
            text[..previous_end]
                .rfind('\n')
                .map_or(0, |index| index + 1),
            previous_end,
        )
    } else {
        if line_end >= text.len() {
            return cursor;
        }
        let next_start = line_end + 1;
        (
            next_start,
            text[next_start..]
                .find('\n')
                .map_or(text.len(), |index| next_start + index),
        )
    };
    let mut column = 0usize;
    let mut target_offset = 0usize;
    for (byte_offset, character) in text[target_start..target_end].char_indices() {
        let next = column + character.len_utf16();
        if next > goal_column {
            break;
        }
        column = next;
        target_offset = byte_offset + character.len_utf8();
    }
    target_start + target_offset
}
