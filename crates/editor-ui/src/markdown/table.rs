//! Owned GFM table vocabulary and its event accumulator.
//!
//! `pulldown-cmark` owns the table grammar (delimiter rows, escaped pipes,
//! short and long rows); this module only moves its balanced
//! `Table`/`TableHead`/`TableRow`/`TableCell` events into owned cells. Cell
//! content is inline-only by grammar, so a cell is one span run and a table
//! never nests inside another table.

use std::ops::Range;

use pulldown_cmark::Alignment;

use super::{Span, spans_text};

/// Column alignment authored in a table's delimiter row.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TableAlignment {
    /// No alignment marker (`---`): the renderer's default, leading edge.
    Unset,
    /// `:---`
    Left,
    /// `:---:`
    Center,
    /// `---:`
    Right,
}

impl From<Alignment> for TableAlignment {
    fn from(alignment: Alignment) -> Self {
        match alignment {
            Alignment::None => Self::Unset,
            Alignment::Left => Self::Left,
            Alignment::Center => Self::Center,
            Alignment::Right => Self::Right,
        }
    }
}

/// One table cell: flattened inline runs, empty for a blank cell.
#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct TableCell {
    /// Flattened inline runs.
    pub spans: Vec<Span>,
}

/// An owned GFM table.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Table {
    /// One alignment per column, in column order; its length is the table's
    /// column count.
    pub alignments: Vec<TableAlignment>,
    /// Header cells in column order.
    pub header: Vec<TableCell>,
    /// Body rows in source order, each in column order.
    pub rows: Vec<Vec<TableCell>>,
    /// Half-open byte range covering the table in the parsed input.
    pub range: Range<usize>,
}

impl Table {
    /// Flattens the visible text: cells joined by tabs, rows by newlines.
    #[must_use]
    pub fn text_content(&self) -> String {
        std::iter::once(&self.header)
            .chain(&self.rows)
            .map(|row| {
                row.iter()
                    .map(|cell| spans_text(&cell.spans))
                    .collect::<Vec<_>>()
                    .join("\t")
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Accumulation state for the table being assembled.
pub(super) struct TableBuilder {
    alignments: Vec<TableAlignment>,
    header: Vec<TableCell>,
    rows: Vec<Vec<TableCell>>,
    /// Cells of the header or body row being assembled.
    row: Vec<TableCell>,
    start: usize,
}

impl TableBuilder {
    pub(super) fn new(alignments: &[Alignment], start: usize) -> Self {
        Self {
            alignments: alignments.iter().copied().map(Into::into).collect(),
            header: Vec::new(),
            rows: Vec::new(),
            row: Vec::new(),
            start,
        }
    }

    pub(super) fn push_cell(&mut self, spans: Vec<Span>) {
        self.row.push(TableCell { spans });
    }

    pub(super) fn finish_header(&mut self) {
        self.header = std::mem::take(&mut self.row);
    }

    pub(super) fn finish_row(&mut self) {
        let row = std::mem::take(&mut self.row);
        self.rows.push(row);
    }

    pub(super) fn finish(self, end: usize) -> Table {
        Table {
            alignments: self.alignments,
            header: self.header,
            rows: self.rows,
            range: self.start..end,
        }
    }
}
