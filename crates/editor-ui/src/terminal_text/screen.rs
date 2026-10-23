//! The line-model terminal: cursor, overwrite and erase semantics over a
//! bounded buffer of lines.

use std::cell::OnceCell;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use unicode_width::UnicodeWidthChar;

use super::parser::{Params, Parser, Perform};
use super::style::{TerminalLine, TerminalRun, TerminalStyle};

/// Lines a [`TerminalScreen::new`] keeps before dropping the oldest.
pub const DEFAULT_MAX_LINES: usize = 5_000;
/// Characters kept per line; output past this column is dropped.
pub const MAX_LINE_CHARS: usize = 4_096;
/// Highest row an absolute cursor move (`CSI H`, `CSI d`) may address, so a
/// TUI's "jump to row 999" cannot grow hundreds of blank lines.
const MAX_RETAINED_CELLS: usize = 128 * 1024;
const MAX_COMBINING_PER_CELL: usize = 8;
const MAX_CURSOR_ROW: usize = 500;
/// Style-table size before unused styles are first collected.
const STYLE_TABLE_FLOOR: usize = 4_096;
/// Longest OSC 8 target kept; longer links are dropped.
const MAX_LINK_BYTES: usize = 2_048;
/// The interned id of [`TerminalStyle::default`]; always slot 0.
const DEFAULT_STYLE: u32 = 0;

/// One character cell: the character and its interned style id.
#[derive(Clone, Debug)]
struct Cell {
    /// The character drawn in this cell.
    ch: char,
    combining: Vec<char>,
    /// Index into the style table.
    style: u32,
}

/// What erasing leaves behind.
const BLANK: Cell = Cell {
    ch: ' ',
    combining: Vec::new(),
    style: DEFAULT_STYLE,
};

/// One line of cells plus its lazily materialized form.
#[derive(Debug, Default)]
struct Line {
    /// Cells from column 0; the cursor may sit past the end.
    cells: Vec<Cell>,
    /// Materialized text and runs, rebuilt after any change.
    rendered: OnceCell<TerminalLine>,
    dirty: bool,
}

impl Line {
    /// Mutable cells; drops the cached materialization.
    fn cells_mut(&mut self) -> &mut Vec<Cell> {
        self.rendered.take();
        self.dirty = true;
        &mut self.cells
    }

    /// The materialized line, built once per change.
    fn rendered(&self, styles: &[TerminalStyle]) -> &TerminalLine {
        self.rendered
            .get_or_init(|| materialize(&self.cells, styles))
    }
}

/// Builds text plus merged non-default runs from cells.
fn materialize(cells: &[Cell], styles: &[TerminalStyle]) -> TerminalLine {
    let mut text = String::with_capacity(cells.len());
    let mut runs = Vec::new();
    let mut open: Option<(usize, u32)> = None;
    for cell in cells {
        if open.is_none_or(|(_, style)| style != cell.style) {
            if let Some((start, style)) = open {
                push_run(&mut runs, start..text.len(), style, styles);
            }
            open = Some((text.len(), cell.style));
        }
        if cell.ch != '\0' {
            text.push(cell.ch);
            text.extend(&cell.combining);
        }
    }
    if let Some((start, style)) = open {
        push_run(&mut runs, start..text.len(), style, styles);
    }
    TerminalLine { text, runs }
}

/// Records one run unless it is default-styled.
fn push_run(
    runs: &mut Vec<TerminalRun>,
    range: std::ops::Range<usize>,
    style: u32,
    styles: &[TerminalStyle],
) {
    if style == DEFAULT_STYLE {
        return;
    }
    if let Some(style) = styles.get(style as usize) {
        runs.push(TerminalRun {
            range,
            style: style.clone(),
        });
    }
}

/// Converts a line count into the public counters' width.
fn count(lines: usize) -> u64 {
    u64::try_from(lines).unwrap_or(u64::MAX)
}

/// The cursor-and-lines state the parser drives.
#[derive(Debug)]
struct Grid {
    /// Retained lines, oldest first; never empty.
    lines: VecDeque<Line>,
    /// Cursor line, always a valid index into `lines`.
    row: usize,
    /// Cursor column, at most [`MAX_LINE_CHARS`].
    col: usize,
    /// `ESC 7` / `CSI s` cursor.
    saved: Option<(usize, usize)>,
    /// Interned styles; slot 0 is the default.
    styles: Vec<TerminalStyle>,
    /// Reverse lookup for `styles`.
    style_ids: HashMap<TerminalStyle, u32>,
    /// Table size that triggers the next unused-style collection.
    compact_at: usize,
    /// The current drawing style.
    pen: TerminalStyle,
    /// Interned id of `pen`.
    pen_id: u32,
    /// Retention bound for `lines`, at least 1.
    max_lines: usize,
    retained_cells: usize,
    /// Lines dropped from the front by the retention bound.
    truncated_lines: u64,
    /// Content lines dropped by clear-screen sequences.
    cleared_lines: u64,
}

impl Grid {
    /// An empty grid retaining at most `max_lines` lines.
    fn new(max_lines: usize) -> Self {
        let mut lines = VecDeque::new();
        lines.push_back(Line::default());
        let mut style_ids = HashMap::new();
        style_ids.insert(TerminalStyle::default(), DEFAULT_STYLE);
        Self {
            lines,
            row: 0,
            col: 0,
            saved: None,
            styles: vec![TerminalStyle::default()],
            style_ids,
            compact_at: STYLE_TABLE_FLOOR,
            pen: TerminalStyle::default(),
            pen_id: DEFAULT_STYLE,
            max_lines: max_lines.max(1),
            retained_cells: 0,
            truncated_lines: 0,
            cleared_lines: 0,
        }
    }

    /// Lines reported as content: all of them except a final empty line,
    /// which is just where the cursor waits after a trailing newline.
    fn content_len(&self) -> usize {
        let len = self.lines.len();
        if self.lines.back().is_some_and(|line| line.cells.is_empty()) {
            len - 1
        } else {
            len
        }
    }

    /// The cursor line.
    fn current_line(&mut self) -> &mut Line {
        &mut self.lines[self.row]
    }

    /// Moves the cursor down one line, appending (and enforcing the
    /// retention bound) at the bottom. The column is untouched.
    fn index(&mut self) {
        if self.row + 1 < self.lines.len() {
            self.row += 1;
            return;
        }
        self.enforce_cell_budget();
        self.lines.push_back(Line::default());
        self.row += 1;
        if self.lines.len() > self.max_lines {
            if let Some(line) = self.lines.pop_front() {
                self.retained_cells -= line.cells.len();
            }
            self.row -= 1;
            self.truncated_lines += 1;
            if let Some((row, _)) = &mut self.saved {
                *row = row.saturating_sub(1);
            }
        }
    }

    /// Moves the cursor to absolute `row` (0-based from the top of the
    /// retained lines), growing blank lines up to the bounds.
    fn move_to_row(&mut self, row: usize) {
        let target = row.min(MAX_CURSOR_ROW - 1);
        while self.lines.len() <= target && self.lines.len() < self.max_lines {
            self.lines.push_back(Line::default());
        }
        self.row = target.min(self.lines.len() - 1);
    }

    /// Sets the cursor column, clamped to the line cap.
    fn move_to_col(&mut self, col: usize) {
        self.col = col.min(MAX_LINE_CHARS);
    }

    /// `CSI K`: 0 erases to the end, 1 to the start (inclusive), 2 the line.
    fn erase_in_line(&mut self, mode: u32) {
        let col = self.col;
        let before = self.current_line().cells.len();
        let cells = self.current_line().cells_mut();
        match mode {
            0 => cells.truncate(col),
            1 => {
                let end = (col + 1).min(cells.len());
                cells[..end].fill(BLANK);
            }
            2 => cells.clear(),
            _ => {}
        }
        let after = cells.len();
        self.retained_cells -= before - after;
    }

    /// `CSI J`: 0 erases below, 1 above, 2 and 3 clear the screen.
    fn erase_in_display(&mut self, mode: u32) {
        match mode {
            0 => {
                self.erase_in_line(0);
                self.retained_cells -= self
                    .lines
                    .iter()
                    .skip(self.row + 1)
                    .map(|line| line.cells.len())
                    .sum::<usize>();
                self.lines.truncate(self.row + 1);
            }
            1 => {
                for line in self.lines.iter_mut().take(self.row) {
                    self.retained_cells -= line.cells.len();
                    line.cells_mut().clear();
                }
                self.erase_in_line(1);
            }
            2 | 3 => self.clear_screen(),
            _ => {}
        }
    }

    /// Drops every line and homes the cursor row. The screen top is then the
    /// first retained line, which is what absolute rows address.
    fn clear_screen(&mut self) {
        self.cleared_lines += count(self.content_len());
        self.lines.clear();
        self.retained_cells = 0;
        self.lines.push_back(Line::default());
        self.row = 0;
        self.saved = None;
    }

    /// `ESC 7` / `CSI s`.
    const fn save_cursor(&mut self) {
        self.saved = Some((self.row, self.col));
    }

    /// `ESC 8` / `CSI u`; the row clamps to the retained lines.
    fn restore_cursor(&mut self) {
        if let Some((row, col)) = self.saved {
            self.row = row.min(self.lines.len() - 1);
            self.col = col;
        }
    }

    /// Makes `style` the pen, interning it when new.
    fn set_pen(&mut self, style: TerminalStyle) {
        if style == self.pen {
            return;
        }
        self.pen_id = self.intern(&style);
        self.pen = style;
    }

    /// The id for `style`, adding it (after collecting unused styles when the
    /// table has grown) if unseen.
    fn intern(&mut self, style: &TerminalStyle) -> u32 {
        if let Some(&id) = self.style_ids.get(style) {
            return id;
        }
        if self.styles.len() >= self.compact_at {
            self.compact_styles();
        }
        let id = u32::try_from(self.styles.len()).unwrap_or(u32::MAX);
        self.styles.push(style.clone());
        self.style_ids.insert(style.clone(), id);
        id
    }

    /// Drops styles no cell, pen or default uses and renumbers the rest, so
    /// a long truecolor stream cannot grow the table without bound. Cached
    /// materializations hold cloned styles and stay valid.
    fn compact_styles(&mut self) {
        let mut used = vec![false; self.styles.len()];
        used[DEFAULT_STYLE as usize] = true;
        used[self.pen_id as usize] = true;
        for line in &self.lines {
            for cell in &line.cells {
                used[cell.style as usize] = true;
            }
        }
        let mut remap = vec![DEFAULT_STYLE; self.styles.len()];
        let mut kept = Vec::new();
        for (id, style) in self.styles.drain(..).enumerate() {
            if used[id] {
                remap[id] = u32::try_from(kept.len()).unwrap_or(u32::MAX);
                kept.push(style);
            }
        }
        for line in &mut self.lines {
            for cell in &mut line.cells {
                cell.style = remap[cell.style as usize];
            }
        }
        self.pen_id = remap[self.pen_id as usize];
        self.style_ids = kept
            .iter()
            .enumerate()
            .map(|(id, style)| (style.clone(), u32::try_from(id).unwrap_or(u32::MAX)))
            .collect();
        self.compact_at = (kept.len() * 2).max(STYLE_TABLE_FLOOR);
        self.styles = kept;
    }
    fn enforce_cell_budget(&mut self) {
        while self.retained_cells > MAX_RETAINED_CELLS && self.lines.len() > 1 {
            if let Some(line) = self.lines.pop_front() {
                self.retained_cells -= line.cells.len();
                self.row = self.row.saturating_sub(1);
                self.truncated_lines += 1;
                if let Some((row, _)) = &mut self.saved {
                    *row = row.saturating_sub(1);
                }
            }
        }
    }
}

impl Perform for Grid {
    fn print(&mut self, ch: char) {
        let width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if width == 0 {
            let col = self.col;
            let cells = self.current_line().cells_mut();
            let end = col.min(cells.len());
            if let Some(cell) = cells[..end].iter_mut().rev().find(|cell| cell.ch != '\0')
                && cell.combining.len() < MAX_COMBINING_PER_CELL
            {
                cell.combining.push(ch);
            }
            return;
        }
        if self.col.saturating_add(width) > MAX_LINE_CHARS {
            return;
        }
        let col = self.col;
        let cell = Cell {
            ch,
            combining: Vec::new(),
            style: self.pen_id,
        };
        let cells = self.current_line().cells_mut();
        if cells.get(col).is_some_and(|cell| cell.ch == '\0') && col > 0 {
            cells[col - 1] = BLANK;
        }
        if cells.get(col + 1).is_some_and(|cell| cell.ch == '\0') {
            cells[col + 1] = BLANK;
        }
        let added = (col + width).saturating_sub(cells.len());
        if added > 0 {
            cells.resize(col + width, BLANK);
        }
        cells[col] = cell;
        if width == 2 {
            cells[col + 1] = Cell {
                ch: '\0',
                combining: Vec::new(),
                style: DEFAULT_STYLE,
            };
        }
        self.retained_cells += added;
        self.col += width;
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            // LF, VT and FF all act as CRLF, as cooked-mode output does.
            b'\n' | 0x0b | 0x0c => {
                self.index();
                self.col = 0;
            }
            b'\r' => self.col = 0,
            b'\t' => self.move_to_col((self.col / 8 + 1) * 8),
            0x08 => self.col = self.col.saturating_sub(1),
            _ => {}
        }
    }

    fn csi(&mut self, params: &Params, final_byte: u8) {
        let mode = params.first(0);
        // Movement counts and 1-based positions: 0 and absent read as 1.
        let amount = |index: usize| (params.first(index) as usize).max(1);
        match final_byte {
            b'm' => {
                let mut style = self.pen.clone();
                style.apply_sgr(params);
                self.set_pen(style);
            }
            b'K' => self.erase_in_line(mode),
            b'J' => self.erase_in_display(mode),
            b'A' => self.row = self.row.saturating_sub(amount(0)),
            b'F' => {
                self.row = self.row.saturating_sub(amount(0));
                self.col = 0;
            }
            b'B' => self.row = (self.row + amount(0)).min(self.lines.len() - 1),
            b'E' => {
                self.row = (self.row + amount(0)).min(self.lines.len() - 1);
                self.col = 0;
            }
            b'C' => self.move_to_col(self.col + amount(0)),
            b'D' => self.col = self.col.saturating_sub(amount(0)),
            b'G' | b'`' => self.move_to_col(amount(0) - 1),
            b'H' | b'f' => {
                self.move_to_row(amount(0) - 1);
                self.move_to_col(amount(1) - 1);
            }
            b'd' => self.move_to_row(amount(0) - 1),
            b's' => self.save_cursor(),
            b'u' => self.restore_cursor(),
            _ => {}
        }
    }

    fn esc(&mut self, final_byte: u8) {
        match final_byte {
            b'7' => self.save_cursor(),
            b'8' => self.restore_cursor(),
            b'c' => {
                self.clear_screen();
                self.set_pen(TerminalStyle::default());
                self.col = 0;
            }
            b'D' => self.index(),
            b'E' => {
                self.index();
                self.col = 0;
            }
            b'M' => self.row = self.row.saturating_sub(1),
            _ => {}
        }
    }

    fn osc(&mut self, payload: &[u8]) {
        let Some(rest) = payload.strip_prefix(b"8;") else {
            return;
        };
        let target = rest
            .iter()
            .position(|&byte| byte == b';')
            .map_or(&[][..], |split| &rest[split + 1..]);
        let link = (!target.is_empty() && target.len() <= MAX_LINK_BYTES)
            .then(|| Arc::<str>::from(String::from_utf8_lossy(target).as_ref()));
        let style = TerminalStyle {
            link,
            ..self.pen.clone()
        };
        self.set_pen(style);
    }
}

/// An incremental line-model terminal fed raw output bytes.
///
/// Lines are bounded: at most `max_lines` are kept (oldest dropped and
/// counted in [`Self::truncated_line_count`]), and each holds at most
/// [`MAX_LINE_CHARS`] characters. A clear screen drops everything before it
/// ([`Self::cleared_earlier`]). Absolute cursor rows address the first
/// retained line, which is the screen top after any clear.
///
/// A final empty line, where the cursor waits after a trailing newline, is
/// not content: `"a\n"` reports one line, `"a\n\n"` two.
#[derive(Debug)]
pub struct TerminalScreen {
    /// Partial-sequence state carried between feeds.
    parser: Parser,
    /// Lines and cursor.
    grid: Grid,
}

impl Default for TerminalScreen {
    fn default() -> Self {
        Self::new()
    }
}

impl TerminalScreen {
    /// An empty screen keeping [`DEFAULT_MAX_LINES`] lines.
    #[must_use]
    pub fn new() -> Self {
        Self::with_max_lines(DEFAULT_MAX_LINES)
    }

    /// An empty screen keeping at most `max_lines` lines (at least 1).
    #[must_use]
    pub fn with_max_lines(max_lines: usize) -> Self {
        Self {
            parser: Parser::default(),
            grid: Grid::new(max_lines),
        }
    }

    /// Consumes raw output bytes. Escape sequences and UTF-8 code points may
    /// be split across calls; invalid UTF-8 becomes U+FFFD.
    pub fn feed(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.parser.advance(byte, &mut self.grid);
        }
        self.grid.enforce_cell_budget();
    }

    /// Number of content lines.
    #[must_use]
    pub fn line_count(&self) -> usize {
        self.grid.content_len()
    }

    /// Consume rows changed since the owning display last synchronized.
    /// Materializing text does not consume this change record. Consumers must
    /// also reconcile clear/truncation counters and line count for row shifts.
    pub fn take_dirty_line_indices(&mut self) -> Vec<usize> {
        let count = self.grid.content_len();
        self.grid
            .lines
            .iter_mut()
            .take(count)
            .enumerate()
            .filter_map(|(index, line)| std::mem::take(&mut line.dirty).then_some(index))
            .collect()
    }

    /// The content lines, oldest first. Each line materializes once per
    /// change, so repeated frames over unchanged lines are cheap.
    #[must_use]
    pub fn lines(&self) -> impl DoubleEndedIterator<Item = &TerminalLine> + ExactSizeIterator {
        let styles = &self.grid.styles;
        self.grid
            .lines
            .iter()
            .take(self.grid.content_len())
            .map(move |line| line.rendered(styles))
    }

    /// Content line `index`, if any.
    #[must_use]
    pub fn line(&self, index: usize) -> Option<&TerminalLine> {
        if index >= self.grid.content_len() {
            return None;
        }
        self.grid
            .lines
            .get(index)
            .map(|line| line.rendered(&self.grid.styles))
    }

    /// The content lines' text joined with `'\n'`.
    #[must_use]
    pub fn plain_text(&self) -> String {
        let mut text = String::new();
        for (index, line) in self.lines().enumerate() {
            if index > 0 {
                text.push('\n');
            }
            text.push_str(&line.text);
        }
        text
    }

    /// The last line with visible text, trailing whitespace trimmed, for a
    /// one-line preview.
    #[must_use]
    pub fn last_nonempty_line(&self) -> Option<&str> {
        self.lines()
            .rev()
            .map(|line| line.text.trim_end())
            .find(|text| !text.is_empty())
    }

    /// Whether a clear screen has dropped earlier content.
    #[must_use]
    pub const fn cleared_earlier(&self) -> bool {
        self.grid.cleared_lines > 0
    }

    /// Content lines dropped by clear-screen sequences so far.
    #[must_use]
    pub const fn cleared_line_count(&self) -> u64 {
        self.grid.cleared_lines
    }

    /// Lines dropped from the front by the `max_lines` bound so far.
    #[must_use]
    pub const fn truncated_line_count(&self) -> u64 {
        self.grid.truncated_lines
    }
}
