use super::{Block, DocumentBuilder, MarkdownEngine, TableAlignment, TableCell};
pub use artisan_markdown_index::{BlockShape, IndexedLeaf, LeafContext, LeafSpacing, ListLevel};
#[derive(Debug, thiserror::Error)]
pub enum MarkdownIndexError {
    #[error(transparent)]
    Index(#[from] artisan_markdown_index::BuildIndexError),
    #[error(transparent)]
    Replay(#[from] artisan_markdown_index::ReplayLeafError),
    #[error(transparent)]
    Parse(#[from] super::MarkdownError),
    #[error("The indexed Markdown leaf has an unexpected semantic shape")]
    InvalidShape,
    #[error("The Markdown index has no leaf {leaf}")]
    MissingLeaf { leaf: usize },
}
#[derive(Debug)]
pub enum IndexedLeafContent {
    Block(Block),
    TableRow {
        header: bool,
        alignments: Vec<TableAlignment>,
        cells: Vec<TableCell>,
    },
}
#[derive(Debug)]
pub struct MarkdownIndex(artisan_markdown_index::MarkdownIndex);
impl MarkdownIndex {
    pub fn build(source: &str) -> Result<Self, MarkdownIndexError> {
        Ok(Self(artisan_markdown_index::MarkdownIndex::build(source)?))
    }
    pub fn extend(self, previous: &str, source: &str) -> Result<(Self, usize), MarkdownIndexError> {
        let (index, kept) = self.0.extend(previous, source)?;
        Ok((Self(index), kept))
    }
    pub fn resident_bytes(&self) -> usize {
        self.0.resident_bytes()
    }
    pub fn leaves(&self) -> &[IndexedLeaf] {
        self.0.leaves()
    }
    pub fn context(&self, leaf: usize) -> Option<&LeafContext> {
        self.0.context(leaf)
    }
    pub(crate) fn normalize_leaf(
        &self,
        leaf: usize,
        start: usize,
        source: &str,
        engine: &MarkdownEngine,
        memo: &mut dyn super::FenceTokenMemo,
    ) -> Result<IndexedLeafContent, MarkdownIndexError> {
        let record = self
            .0
            .record(leaf)
            .ok_or(MarkdownIndexError::MissingLeaf { leaf })?;
        normalize_record(&record, start, source, engine, memo)
    }
}
pub(crate) fn normalize_record(
    record: &artisan_markdown_index::LeafRecord,
    start: usize,
    source: &str,
    engine: &MarkdownEngine,
    memo: &mut dyn super::FenceTokenMemo,
) -> Result<IndexedLeafContent, MarkdownIndexError> {
    let exact = record.exact_source(start, source)?;
    let mut document =
        DocumentBuilder::default().build_events(exact, record.replay(start, source)?)?;
    engine.highlight_blocks(&mut document.blocks, memo)?;
    if document.blocks.len() != 1 {
        return Err(MarkdownIndexError::InvalidShape);
    }
    let Some(block) = document.blocks.pop() else {
        return Err(MarkdownIndexError::InvalidShape);
    };
    match (record.is_table_header(), block) {
        (Some(header), Block::Table(mut table)) => {
            let cells = if header {
                table.header
            } else {
                if table.rows.len() != 1 {
                    return Err(MarkdownIndexError::InvalidShape);
                }
                let Some(row) = table.rows.pop() else {
                    return Err(MarkdownIndexError::InvalidShape);
                };
                row
            };
            Ok(IndexedLeafContent::TableRow {
                header,
                alignments: table.alignments,
                cells,
            })
        }
        (None, block) => Ok(IndexedLeafContent::Block(block)),
        _ => Err(MarkdownIndexError::InvalidShape),
    }
}
