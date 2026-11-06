use std::cell::OnceCell;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use unicode_width::UnicodeWidthChar;

use super::parser::{Params, Parser, Perform};
use super::style::{TerminalLine, TerminalRun, TerminalStyle};

pub const DEFAULT_MAX_LINES: usize = 5_000;
pub const MAX_LINE_CHARS: usize = 4_096;
const MAX_RETAINED_CELLS: usize = 128 * 1024;
const MAX_COMBINING_PER_CELL: usize = 8;
const MAX_CURSOR_ROW: usize = 500;
const STYLE_TABLE_FLOOR: usize = 4_096;
const MAX_LINK_BYTES: usize = 2_048;
const DEFAULT_STYLE: u32 = 0;

#[derive(Clone, Debug)]
struct Cell {
    ch: char,
    combining: Vec<char>,
    style: u32,
}

const BLANK: Cell = Cell {
    ch: ' ',
    combining: Vec::new(),
    style: DEFAULT_STYLE,
};

#[derive(Debug, Default)]
struct Line {
    cells: Vec<Cell>,
    rendered: OnceCell<TerminalLine>,
    dirty: bool,
}

impl Line {
    fn cells_mut(&mut self) -> &mut Vec<Cell> {
        self.rendered.take();
        self.dirty = true;
        &mut self.cells
    }

    fn rendered(&self, styles: &[TerminalStyle]) -> &TerminalLine {
        self.rendered
            .get_or_init(|| materialize(&self.cells, styles))
    }
}

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

fn count(lines: usize) -> u64 {
    u64::try_from(lines).unwrap_or(u64::MAX)
}

#[derive(Debug)]
struct Grid {
    lines: VecDeque<Line>,
    row: usize,
    col: usize,
    saved: Option<(usize, usize)>,
    styles: Vec<TerminalStyle>,
    style_ids: HashMap<TerminalStyle, u32>,
    compact_at: usize,
    pen: TerminalStyle,
    pen_id: u32,
    max_lines: usize,
    retained_cells: usize,
    truncated_lines: u64,
    cleared_lines: u64,
}

impl Grid {
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

    fn content_len(&self) -> usize {
        let len = self.lines.len();
        if self.lines.back().is_some_and(|line| line.cells.is_empty()) {
            len - 1
        } else {
            len
        }
    }

    fn current_line(&mut self) -> &mut Line {
        &mut self.lines[self.row]
    }

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

    fn move_to_row(&mut self, row: usize) {
        let target = row.min(MAX_CURSOR_ROW - 1);
        while self.lines.len() <= target && self.lines.len() < self.max_lines {
            self.lines.push_back(Line::default());
        }
        self.row = target.min(self.lines.len() - 1);
    }

    fn move_to_col(&mut self, col: usize) {
        self.col = col.min(MAX_LINE_CHARS);
    }

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

    fn clear_screen(&mut self) {
        self.cleared_lines += count(self.content_len());
        self.lines.clear();
        self.retained_cells = 0;
        self.lines.push_back(Line::default());
        self.row = 0;
        self.saved = None;
    }

    const fn save_cursor(&mut self) {
        self.saved = Some((self.row, self.col));
    }

    fn restore_cursor(&mut self) {
        if let Some((row, col)) = self.saved {
            self.row = row.min(self.lines.len() - 1);
            self.col = col;
        }
    }

    fn set_pen(&mut self, style: TerminalStyle) {
        if style == self.pen {
            return;
        }
        self.pen_id = self.intern(&style);
        self.pen = style;
    }

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

#[derive(Debug)]
pub struct TerminalScreen {
    parser: Parser,
    grid: Grid,
}

impl Default for TerminalScreen {
    fn default() -> Self {
        Self::new()
    }
}

impl TerminalScreen {
    #[must_use]
    pub fn new() -> Self {
        Self::with_max_lines(DEFAULT_MAX_LINES)
    }

    #[must_use]
    pub fn with_max_lines(max_lines: usize) -> Self {
        Self {
            parser: Parser::default(),
            grid: Grid::new(max_lines),
        }
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.parser.advance(byte, &mut self.grid);
        }
        self.grid.enforce_cell_budget();
    }

    #[must_use]
    pub fn line_count(&self) -> usize {
        self.grid.content_len()
    }

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

    #[must_use]
    pub fn lines(&self) -> impl DoubleEndedIterator<Item = &TerminalLine> + ExactSizeIterator {
        let styles = &self.grid.styles;
        self.grid
            .lines
            .iter()
            .take(self.grid.content_len())
            .map(move |line| line.rendered(styles))
    }

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

    #[must_use]
    pub fn last_nonempty_line(&self) -> Option<&str> {
        self.lines()
            .rev()
            .map(|line| line.text.trim_end())
            .find(|text| !text.is_empty())
    }

    #[must_use]
    pub const fn cleared_earlier(&self) -> bool {
        self.grid.cleared_lines > 0
    }

    #[must_use]
    pub const fn cleared_line_count(&self) -> u64 {
        self.grid.cleared_lines
    }

    #[must_use]
    pub const fn truncated_line_count(&self) -> u64 {
        self.grid.truncated_lines
    }
}
