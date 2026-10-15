//! Source offsets and exact parser events for lazily loaded Markdown leaves.
//! The scan owns no source or block AST. Borrowed event payloads become offsets;
//! decoded escapes and entities are interned. A fetched complete leaf replays
//! these events, so reference links keep their original document resolution.

use std::ops::Range;
use std::sync::Arc;

use pulldown_cmark::{Alignment, CodeBlockKind, TagEnd};
pub use pulldown_cmark::{Event, Tag};

use serde::{Deserialize, Serialize};
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub enum TableAlignment {
    Unset,
    Left,
    Center,
    Right,
}
impl From<Alignment> for TableAlignment {
    fn from(a: Alignment) -> Self {
        match a {
            Alignment::None => Self::Unset,
            Alignment::Left => Self::Left,
            Alignment::Center => Self::Center,
            Alignment::Right => Self::Right,
        }
    }
}
pub const PARSE_OPTIONS: pulldown_cmark::Options =
    pulldown_cmark::Options::ENABLE_TASKLISTS.union(pulldown_cmark::Options::ENABLE_TABLES);

mod build;

const SOURCE_BYTES: usize = 64 * 1024 * 1024;
const INDEX_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum MarkdownIndexError {
    #[error("Markdown source exceeds the 64 MiB indexing admission limit")]
    SourceBudget,
    #[error("Markdown index exceeds its 16 MiB memory limit")]
    IndexBudget,
    #[error("Markdown nesting exceeds 96 tags")]
    NestingBudget,
    #[error("The fetched Markdown leaf does not cover its complete indexed source range")]
    IncompleteRange,
    #[error("The indexed Markdown leaf has an unexpected semantic shape")]
    InvalidShape,
}

/// One authored list marker surrounding a native semantic leaf.
#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct ListLevel {
    pub source_start: usize,
    pub number: Option<u64>,
    pub task: Option<bool>,
    /// Only the first leaf of an item draws the item's marker.
    pub first: bool,
}

/// Interned context independent of any fetched source bytes.
#[derive(Clone, Debug, Default, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct LeafContext {
    pub lists: Vec<ListLevel>,
    pub quote_depth: usize,
    pub table_alignments: Vec<TableAlignment>,
    pub table_source: Option<usize>,
    pub spacing: Vec<LeafSpacing>,
}

/// Source sibling kinds; the renderer's shared margin policy supplies pixels.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub enum BlockShape {
    Paragraph,
    Heading(u8),
    Code,
    Html,
    List,
    Table,
}
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub enum LeafSpacing {
    Sibling {
        previous: Option<BlockShape>,
        current: BlockShape,
        in_item: bool,
    },
    Item,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
enum Kind {
    Paragraph,
    Heading,
    Code,
    Html,
    TableRow { header: bool },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
enum Text {
    Range(Range<usize>),
    Decoded(Arc<str>),
}
#[derive(Clone, Debug, Serialize, Deserialize)]
enum Token {
    Structural(#[serde(with = "event_serde")] Event<'static>),
    Text(Text),
    Code(Text),
    Html(Text),
    InlineHtml(Text),
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct IndexedEvent {
    token: Token,
    range: Range<usize>,
}

/// One native paragraph, heading, code unit, HTML unit, or table row.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IndexedLeaf {
    range: Range<usize>,
    context: usize,
    kind: Kind,
    events: Vec<IndexedEvent>,
}
impl IndexedLeaf {
    pub fn source_range(&self) -> Range<usize> {
        self.range.clone()
    }
}

/// Compact source topology. Drop the input String immediately after building.
#[derive(Debug)]
pub struct MarkdownIndex {
    leaves: Vec<IndexedLeaf>,
    contexts: Vec<LeafContext>,
    decoded: Vec<Arc<str>>,
    source_bytes: usize,
}

impl MarkdownIndex {
    pub fn build(source: &str) -> Result<Self, MarkdownIndexError> {
        build::build(source)
    }

    pub fn record(&self, ordinal: usize) -> Option<LeafRecord> {
        let leaf = self.leaves.get(ordinal)?.clone();
        let context = self.contexts.get(leaf.context)?.clone();
        let table_continues = context.table_source.is_some()
            && self
                .context(ordinal + 1)
                .and_then(|context| context.table_source)
                == context.table_source;
        Some(LeafRecord {
            leaf,
            context,
            table_continues,
        })
    }
    pub fn leaves(&self) -> &[IndexedLeaf] {
        &self.leaves
    }
    pub fn context(&self, leaf: usize) -> Option<&LeafContext> {
        self.leaves
            .get(leaf)
            .map(|leaf| &self.contexts[leaf.context])
    }
    pub fn source_bytes(&self) -> usize {
        self.source_bytes
    }
    pub fn resident_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.leaves.capacity() * std::mem::size_of::<IndexedLeaf>()
            + self.contexts.capacity() * std::mem::size_of::<LeafContext>()
            + self.decoded.capacity() * std::mem::size_of::<Arc<str>>()
            + self
                .decoded
                .iter()
                .map(|text| text.len() + 2 * std::mem::size_of::<usize>())
                .sum::<usize>()
            + self
                .contexts
                .iter()
                .map(|c| {
                    c.lists.capacity() * std::mem::size_of::<ListLevel>()
                        + c.table_alignments.capacity() * std::mem::size_of::<TableAlignment>()
                        + c.spacing.capacity() * std::mem::size_of::<LeafSpacing>()
                })
                .sum::<usize>()
            + self
                .leaves
                .iter()
                .map(|leaf| {
                    leaf.events.capacity() * std::mem::size_of::<IndexedEvent>()
                        + leaf
                            .events
                            .iter()
                            .map(|e| match &e.token {
                                Token::Structural(event) => event_heap_bytes(event),
                                _ => 0,
                            })
                            .sum::<usize>()
                })
                .sum::<usize>()
    }
}

fn resolve_text<'a>(
    text: &'a Text,
    source: &'a str,
    origin: usize,
) -> Result<pulldown_cmark::CowStr<'a>, MarkdownIndexError> {
    let value = match text {
        Text::Range(range) => source
            .get(
                range
                    .start
                    .checked_sub(origin)
                    .ok_or(MarkdownIndexError::IncompleteRange)?
                    ..range
                        .end
                        .checked_sub(origin)
                        .ok_or(MarkdownIndexError::IncompleteRange)?,
            )
            .ok_or(MarkdownIndexError::IncompleteRange)?,
        Text::Decoded(value) => value.as_ref(),
    };
    Ok(value.into())
}

fn event_heap_bytes(event: &Event<'_>) -> usize {
    match event {
        Event::Start(
            Tag::Link {
                dest_url,
                title,
                id,
                ..
            }
            | Tag::Image {
                dest_url,
                title,
                id,
                ..
            },
        ) => dest_url.len() + title.len() + id.len(),
        Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(info))) => info.len(),
        Event::Start(Tag::Heading {
            id, classes, attrs, ..
        }) => {
            id.as_ref().map_or(0, |v| v.len())
                + classes.capacity() * std::mem::size_of::<pulldown_cmark::CowStr<'_>>()
                + classes.iter().map(|v| v.len()).sum::<usize>()
                + attrs.capacity()
                    * std::mem::size_of::<(
                        pulldown_cmark::CowStr<'_>,
                        Option<pulldown_cmark::CowStr<'_>>,
                    )>()
                + attrs
                    .iter()
                    .map(|(key, value)| key.len() + value.as_ref().map_or(0, |v| v.len()))
                    .sum::<usize>()
        }
        Event::Start(Tag::Table(alignments)) => {
            alignments.capacity() * std::mem::size_of::<Alignment>()
        }
        _ => 0,
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LeafRecord {
    pub leaf: IndexedLeaf,
    pub context: LeafContext,
    /// Another semantic row follows in the same authored table.
    pub table_continues: bool,
}
impl LeafRecord {
    /// A physical literal line includes its LF delimiter. The final empty line
    /// has an empty range, so line ordinals remain faithful to authored text.
    pub fn literal(range: Range<usize>) -> Self {
        Self {
            leaf: IndexedLeaf {
                range: range.clone(),
                context: 0,
                kind: Kind::Paragraph,
                events: vec![IndexedEvent {
                    token: Token::Text(Text::Range(range.clone())),
                    range,
                }],
            },
            context: LeafContext::default(),
            table_continues: false,
        }
    }

    pub fn is_table_header(&self) -> Option<bool> {
        match self.leaf.kind {
            Kind::TableRow { header } => Some(header),
            _ => None,
        }
    }
    pub fn replay(
        &self,
        start: usize,
        source: &str,
    ) -> Result<Vec<(Event<'static>, Range<usize>)>, MarkdownIndexError> {
        let leaf = &self.leaf;
        let local_start = leaf
            .range
            .start
            .checked_sub(start)
            .ok_or(MarkdownIndexError::IncompleteRange)?;
        let local_end = leaf
            .range
            .end
            .checked_sub(start)
            .ok_or(MarkdownIndexError::IncompleteRange)?;
        let exact = source
            .get(local_start..local_end)
            .ok_or(MarkdownIndexError::IncompleteRange)?;
        let origin = leaf.range.start;
        let mut events = Vec::with_capacity(leaf.events.len() + 2);
        if let Kind::TableRow { .. } = leaf.kind {
            let alignments = self
                .context
                .table_alignments
                .iter()
                .map(|a| match a {
                    TableAlignment::Unset => Alignment::None,
                    TableAlignment::Left => Alignment::Left,
                    TableAlignment::Center => Alignment::Center,
                    TableAlignment::Right => Alignment::Right,
                })
                .collect();
            events.push((Event::Start(Tag::Table(alignments)), 0..0));
        }
        for event in &leaf.events {
            let value = match &event.token {
                Token::Structural(event) => event.clone(),
                Token::Text(value) => Event::Text(resolve_text(value, exact, origin)?),
                Token::Code(value) => Event::Code(resolve_text(value, exact, origin)?),
                Token::Html(value) => Event::Html(resolve_text(value, exact, origin)?),
                Token::InlineHtml(value) => Event::InlineHtml(resolve_text(value, exact, origin)?),
            };
            let range = event
                .range
                .start
                .checked_sub(origin)
                .ok_or(MarkdownIndexError::IncompleteRange)?
                ..event
                    .range
                    .end
                    .checked_sub(origin)
                    .ok_or(MarkdownIndexError::IncompleteRange)?;
            events.push((value.into_static(), range));
        }
        if matches!(leaf.kind, Kind::TableRow { .. }) {
            events.push((Event::End(TagEnd::Table), 0..exact.len()));
        }
        Ok(events)
    }
}
mod event_serde {
    use pulldown_cmark::Event;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    pub fn serialize<S: Serializer>(
        event: &Event<'static>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        event.serialize(serializer)
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Event<'static>, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        let encoded = serde_json::to_string(&value).map_err(serde::de::Error::custom)?;
        let event: Event<'_> = serde_json::from_str(&encoded).map_err(serde::de::Error::custom)?;
        Ok(event.into_static())
    }
}

#[cfg(test)]
mod sparse_table_context_tests {
    use super::*;
    #[test]
    fn exact_table_membership_survives_sparse_record_replay() {
        let index = MarkdownIndex::build(
            "| A | B |\n| --- | --- |\n| one | two |\n| three | four |\n\nafter\n",
        )
        .unwrap();
        assert!(index.record(0).unwrap().table_continues);
        assert!(index.record(1).unwrap().table_continues);
        assert!(!index.record(2).unwrap().table_continues);
        assert!(!index.record(3).unwrap().table_continues);
        let last = index.record(2).unwrap();
        let wire = serde_json::to_string(&last).unwrap();
        let decoded: LeafRecord = serde_json::from_str(&wire).unwrap();
        assert_eq!(decoded.context, last.context);
        assert!(!decoded.table_continues);
    }
}
