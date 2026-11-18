use pulldown_cmark::Alignment;
use std::ops::Range;

use super::Span;

pub use artisan_markdown_index::TableAlignment;

#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct TableCell {
    pub(crate) spans: Vec<Span>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Table {
    pub(crate) alignments: Vec<TableAlignment>,
    pub(crate) header: Vec<TableCell>,
    pub(crate) rows: Vec<Vec<TableCell>>,
    pub(crate) range: Range<usize>,
}

impl Table {}

pub(super) struct TableBuilder {
    alignments: Vec<TableAlignment>,
    header: Vec<TableCell>,
    rows: Vec<Vec<TableCell>>,
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
