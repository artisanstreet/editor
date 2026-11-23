use super::mentions::BalancedEvents;
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
    #[error("Markdown leaf {leaf:?} closes a tag at {token:?} that it never opened")]
    UnbalancedEvents {
        leaf: std::ops::Range<usize>,
        token: std::ops::Range<usize>,
    },
    #[error("Markdown leaf {leaf:?} leaves {open} tags open")]
    UnclosedEvents {
        leaf: std::ops::Range<usize>,
        open: usize,
    },
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
    pub(crate) fn build(source: &str) -> Result<Self, MarkdownIndexError> {
        Ok(Self(artisan_markdown_index::MarkdownIndex::build(source)?))
    }
    pub(crate) fn append(self, source: &str) -> Result<(Self, usize), MarkdownIndexError> {
        let (index, kept) = self.0.append(source)?;
        Ok((Self(index), kept))
    }
    pub(crate) fn resident_bytes(&self) -> usize {
        self.0.resident_bytes()
    }
    pub(crate) fn leaves(&self) -> &[IndexedLeaf] {
        self.0.leaves()
    }
    pub(crate) fn context(&self, leaf: usize) -> Option<&LeafContext> {
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

    fn record(
        &self,
        leaf: usize,
    ) -> Result<artisan_markdown_index::LeafRecord, MarkdownIndexError> {
        self.0
            .record(leaf)
            .ok_or(MarkdownIndexError::MissingLeaf { leaf })
    }

    pub(crate) fn normalize_message_leaf(
        &self,
        leaf: usize,
        source: &str,
        engine: &MarkdownEngine,
        memo: &mut dyn super::FenceTokenMemo,
        vocabulary: &artisan_mentions::MessageVocabulary,
    ) -> Result<MessageLeaf, MarkdownIndexError> {
        normalize_message_record(&self.record(leaf)?, 0, source, engine, memo, vocabulary)
    }

    pub(crate) fn leaf_mentions(
        &self,
        leaf: usize,
        source: &str,
        vocabulary: &artisan_mentions::MessageVocabulary,
    ) -> Result<Vec<artisan_mentions::MessageMention>, MarkdownIndexError> {
        super::mentions::record_mentions(&self.record(leaf)?, 0, source, vocabulary)
    }
}

pub(crate) struct MessageLeaf {
    pub(crate) content: IndexedLeafContent,
    pub(crate) mentions: Vec<artisan_mentions::MessageMention>,
}

pub(crate) fn normalize_message_record(
    record: &artisan_markdown_index::LeafRecord,
    start: usize,
    source: &str,
    engine: &MarkdownEngine,
    memo: &mut dyn super::FenceTokenMemo,
    vocabulary: &artisan_mentions::MessageVocabulary,
) -> Result<MessageLeaf, MarkdownIndexError> {
    let exact = record.exact_source(start, source)?;
    let events = BalancedEvents::replay(record, start, source)?;
    let mentions = super::mentions::resolve(&events, exact, vocabulary);
    let builder = DocumentBuilder {
        mentions: mentions.clone(),
        ..DocumentBuilder::default()
    };
    let content = shape_record(
        record,
        builder.build_events(exact, events.into_events())?,
        engine,
        memo,
    )?;
    Ok(MessageLeaf { content, mentions })
}

pub(crate) fn normalize_record(
    record: &artisan_markdown_index::LeafRecord,
    start: usize,
    source: &str,
    engine: &MarkdownEngine,
    memo: &mut dyn super::FenceTokenMemo,
) -> Result<IndexedLeafContent, MarkdownIndexError> {
    let exact = record.exact_source(start, source)?;
    let document = DocumentBuilder::default().build_events(
        exact,
        BalancedEvents::replay(record, start, source)?.into_events(),
    )?;
    shape_record(record, document, engine, memo)
}

fn shape_record(
    record: &artisan_markdown_index::LeafRecord,
    mut document: super::MarkdownDocument,
    engine: &MarkdownEngine,
    memo: &mut dyn super::FenceTokenMemo,
) -> Result<IndexedLeafContent, MarkdownIndexError> {
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
