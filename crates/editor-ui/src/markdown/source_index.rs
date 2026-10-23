//! Client normalization of portable parser leaves. Forge and Client share the pure scan.
use super::{Block, DocumentBuilder, MarkdownEngine, TableAlignment, TableCell};
pub use artisan_markdown_index::{BlockShape, IndexedLeaf, LeafContext, LeafSpacing, ListLevel};
#[derive(Debug, thiserror::Error)]
pub enum MarkdownIndexError {
    #[error(transparent)]
    Index(#[from] artisan_markdown_index::MarkdownIndexError),
    #[error(transparent)]
    Parse(#[from] super::MarkdownError),
    #[error("The indexed Markdown leaf has an unexpected semantic shape")]
    InvalidShape,
    #[error("The fetched Markdown leaf does not cover its complete indexed source range")]
    IncompleteRange,
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
    pub fn leaves(&self) -> &[IndexedLeaf] {
        self.0.leaves()
    }
    pub fn context(&self, leaf: usize) -> Option<&LeafContext> {
        self.0.context(leaf)
    }
    pub const fn source_bytes(&self) -> usize {
        self.0.source_bytes()
    }
    pub fn resident_bytes(&self) -> usize {
        self.0.resident_bytes()
    }
    pub fn normalize_leaf(
        &self,
        leaf: usize,
        start: usize,
        source: &str,
        engine: &MarkdownEngine,
    ) -> Result<IndexedLeafContent, MarkdownIndexError> {
        let record = self
            .0
            .record(leaf)
            .ok_or(MarkdownIndexError::IncompleteRange)?;
        normalize_record(&record, start, source, engine)
    }
}
pub fn normalize_record(
    record: &artisan_markdown_index::LeafRecord,
    start: usize,
    source: &str,
    engine: &MarkdownEngine,
) -> Result<IndexedLeafContent, MarkdownIndexError> {
    let range = record.leaf.source_range();
    let exact = source
        .get(
            range
                .start
                .checked_sub(start)
                .ok_or(MarkdownIndexError::IncompleteRange)?
                ..range
                    .end
                    .checked_sub(start)
                    .ok_or(MarkdownIndexError::IncompleteRange)?,
        )
        .ok_or(MarkdownIndexError::IncompleteRange)?;
    let mut document =
        DocumentBuilder::default().build_events(exact, record.replay(start, source)?)?;
    engine.highlight_blocks(&mut document.blocks, &mut super::NoFenceTokenMemo)?;
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
#[cfg(test)]
mod tests;
