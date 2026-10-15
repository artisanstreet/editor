//! One maintained-parser scan into compact native semantic leaf descriptors.
use super::PARSE_OPTIONS;
use super::{
    Arc, BlockShape, Event, INDEX_BYTES, IndexedEvent, IndexedLeaf, Kind, LeafContext, LeafSpacing,
    ListLevel, MarkdownIndex, MarkdownIndexError, SOURCE_BYTES, TableAlignment, Tag, TagEnd, Text,
    Token, event_heap_bytes,
};
use pulldown_cmark::Parser;
use std::collections::{HashMap, HashSet};

#[derive(Clone)]
struct OpenList {
    start: Option<u64>,
    source_start: usize,
    ordinal: u64,
}
struct OpenItem {
    task: Option<bool>,
    first: bool,
    source_start: usize,
    previous: Option<BlockShape>,
}
struct ActiveLeaf {
    leaf: IndexedLeaf,
    implicit: bool,
}

#[expect(
    clippy::too_many_lines,
    reason = "one pass over the parser's events, keeping list, table and leaf state together"
)]
pub(super) fn build(source: &str) -> Result<MarkdownIndex, MarkdownIndexError> {
    if source.len() > SOURCE_BYTES {
        return Err(MarkdownIndexError::SourceBudget);
    }
    let mut result = MarkdownIndex {
        leaves: Vec::new(),
        contexts: Vec::new(),
        decoded: Vec::new(),
        source_bytes: source.len(),
    };
    let mut contexts = HashMap::<LeafContext, usize>::new();
    let mut decoded = HashSet::<Arc<str>>::new();
    let mut lists = Vec::<OpenList>::new();
    let mut items = Vec::<OpenItem>::new();
    let mut alignments = Vec::<TableAlignment>::new();
    let mut quote_depth = 0;
    let mut root_previous = None;
    let mut spacing = Vec::new();
    let mut table_source = None;
    let mut depth = 0usize;
    let mut active: Option<ActiveLeaf> = None;
    // Includes the transient interning maps and pending event vector. The
    // completed index reports its own actual retained capacities below.
    let mut admission_bytes = 0usize;
    for (event, range) in Parser::new_ext(source, PARSE_OPTIONS).into_offset_iter() {
        if admission_bytes > INDEX_BYTES {
            return Err(MarkdownIndexError::IndexBudget);
        }
        if let Event::Start(_) = &event {
            depth += 1;
            if depth > 96 {
                return Err(MarkdownIndexError::NestingBudget);
            }
        } else if let Event::End(_) = &event {
            depth = depth.saturating_sub(1);
        }

        let boundary = matches!(
            &event,
            Event::Start(
                Tag::Paragraph
                    | Tag::Heading { .. }
                    | Tag::CodeBlock(_)
                    | Tag::HtmlBlock
                    | Tag::List(_)
            ) | Event::End(TagEnd::Item)
        );
        if boundary && active.as_ref().is_some_and(|a| a.implicit) {
            let mut leaf = active.take().unwrap().leaf;
            leaf.events.push(IndexedEvent {
                token: Token::Structural(Event::End(TagEnd::Paragraph)),
                range: leaf.range.clone(),
            });
            result.leaves.push(leaf);
            for item in &mut items {
                item.first = false;
            }
        }

        if active.is_none()
            && matches!(&event, Event::End(TagEnd::Item))
            && items.last().is_some_and(|item| item.previous.is_none())
        {
            let source_start = items.last().unwrap().source_start;
            let context = LeafContext {
                lists: lists
                    .iter()
                    .zip(items.iter())
                    .map(|(list, item)| ListLevel {
                        source_start: list.source_start,
                        number: list.start.map(|start| start.saturating_add(list.ordinal)),
                        task: item.task,
                        first: item.first,
                    })
                    .collect(),
                quote_depth,
                table_alignments: alignments.clone(),
                table_source,
                spacing: std::mem::take(&mut spacing),
            };
            let context_id = intern_context(
                context,
                &mut result.contexts,
                &mut contexts,
                &mut admission_bytes,
            );
            let leaf_range = source_start..range.end;
            result.leaves.push(IndexedLeaf {
                range: leaf_range.clone(),
                context: context_id,
                kind: Kind::Paragraph,
                events: vec![
                    IndexedEvent {
                        token: Token::Structural(Event::Start(Tag::Paragraph)),
                        range: leaf_range.clone(),
                    },
                    IndexedEvent {
                        token: Token::Structural(Event::End(TagEnd::Paragraph)),
                        range: leaf_range,
                    },
                ],
            });
            admission_bytes +=
                2 * (std::mem::size_of::<IndexedLeaf>() + 2 * std::mem::size_of::<IndexedEvent>());
            for item in &mut items {
                item.first = false;
            }
        }
        if active.is_none() {
            match &event {
                Event::Start(Tag::List(start)) => {
                    record_spacing(
                        &mut root_previous,
                        &mut items,
                        &mut spacing,
                        BlockShape::List,
                    );
                    lists.push(OpenList {
                        start: *start,
                        source_start: range.start,
                        ordinal: 0,
                    });
                    continue;
                }
                Event::Start(Tag::Item) => {
                    if lists.last().is_some_and(|list| list.ordinal > 0) {
                        spacing.push(LeafSpacing::Item);
                    }
                    items.push(OpenItem {
                        task: None,
                        first: true,
                        source_start: range.start,
                        previous: None,
                    });
                    continue;
                }
                Event::End(TagEnd::Item) => {
                    items.pop();
                    if let Some(list) = lists.last_mut() {
                        list.ordinal += 1;
                    }
                    continue;
                }
                Event::End(TagEnd::List(_)) => {
                    lists.pop();
                    continue;
                }
                Event::Start(Tag::BlockQuote(_)) => {
                    quote_depth += 1;
                    continue;
                }
                Event::End(TagEnd::BlockQuote(_)) => {
                    quote_depth = quote_depth.saturating_sub(1);
                    continue;
                }
                Event::Start(Tag::Table(a)) => {
                    record_spacing(
                        &mut root_previous,
                        &mut items,
                        &mut spacing,
                        BlockShape::Table,
                    );
                    table_source = Some(range.start);
                    alignments = a.iter().copied().map(Into::into).collect();
                    continue;
                }
                Event::End(TagEnd::Table) => {
                    table_source = None;
                    alignments.clear();
                    continue;
                }
                Event::TaskListMarker(checked) => {
                    if let Some(item) = items.last_mut() {
                        item.task = Some(*checked);
                    }
                    continue;
                }
                _ => {}
            }
            let kind = match &event {
                Event::Start(Tag::Heading { .. }) => Some(Kind::Heading),
                Event::Start(Tag::CodeBlock(_)) => Some(Kind::Code),
                Event::Start(Tag::HtmlBlock) | Event::Html(_) => Some(Kind::Html),
                Event::Start(Tag::TableHead) => Some(Kind::TableRow { header: true }),
                Event::Start(Tag::TableRow) => Some(Kind::TableRow { header: false }),
                // Inline content outside a paragraph opens one implicitly.
                Event::Text(_)
                | Event::Code(_)
                | Event::InlineHtml(_)
                | Event::Start(
                    Tag::Paragraph
                    | Tag::Emphasis
                    | Tag::Strong
                    | Tag::Link { .. }
                    | Tag::Image { .. },
                ) => Some(Kind::Paragraph),
                _ => None,
            };
            let Some(kind) = kind else {
                continue;
            };
            let shape = match kind {
                Kind::Paragraph => Some(BlockShape::Paragraph),
                Kind::Heading => Some(BlockShape::Heading(match &event {
                    Event::Start(Tag::Heading { level, .. }) => *level as u8,
                    _ => 1,
                })),
                Kind::Code => Some(BlockShape::Code),
                Kind::Html => Some(BlockShape::Html),
                Kind::TableRow { .. } => None,
            };
            if let Some(shape) = shape {
                record_spacing(&mut root_previous, &mut items, &mut spacing, shape);
            }
            let context = LeafContext {
                lists: lists
                    .iter()
                    .zip(items.iter())
                    .map(|(list, item)| ListLevel {
                        source_start: list.source_start,
                        number: list.start.map(|start| start.saturating_add(list.ordinal)),
                        task: item.task,
                        first: item.first,
                    })
                    .collect(),
                quote_depth,
                table_alignments: alignments.clone(),
                table_source,
                spacing: std::mem::take(&mut spacing),
            };
            let context_id = intern_context(
                context,
                &mut result.contexts,
                &mut contexts,
                &mut admission_bytes,
            );
            let implicit =
                kind == Kind::Paragraph && !matches!(&event, Event::Start(Tag::Paragraph));
            let mut leaf = IndexedLeaf {
                range: range.clone(),
                context: context_id,
                kind,
                events: Vec::new(),
            };
            if implicit {
                leaf.events.push(IndexedEvent {
                    token: Token::Structural(Event::Start(Tag::Paragraph)),
                    range: range.start..range.start,
                });
            }
            active = Some(ActiveLeaf { leaf, implicit });
            admission_bytes += 2 * std::mem::size_of::<IndexedLeaf>();
        }
        let Some(current) = active.as_mut() else {
            continue;
        };
        if let Event::TaskListMarker(checked) = &event {
            if let Some(item) = items.last_mut() {
                item.task = Some(*checked);
            }
            let mut context = result.contexts[current.leaf.context].clone();
            if let Some(level) = context.lists.last_mut() {
                level.task = Some(*checked);
            }
            current.leaf.context = intern_context(
                context,
                &mut result.contexts,
                &mut contexts,
                &mut admission_bytes,
            );
        }
        current.leaf.range.start = current.leaf.range.start.min(range.start);
        current.leaf.range.end = current.leaf.range.end.max(range.end);
        let finishes = match (&event, current.leaf.kind) {
            (Event::End(TagEnd::Paragraph), Kind::Paragraph)
            | (Event::End(TagEnd::Heading(_)), Kind::Heading)
            | (Event::End(TagEnd::CodeBlock), Kind::Code)
            | (Event::End(TagEnd::HtmlBlock), Kind::Html)
            | (Event::End(TagEnd::TableHead), Kind::TableRow { header: true })
            | (Event::End(TagEnd::TableRow), Kind::TableRow { header: false }) => true,
            (Event::Html(_), Kind::Html) if current.leaf.events.is_empty() => true,
            _ => false,
        };
        let token = match event {
            Event::Text(text) => Token::Text(intern_text(
                text.as_ref(),
                source,
                &mut decoded,
                &mut admission_bytes,
            )?),
            Event::Code(text) => Token::Code(intern_text(
                text.as_ref(),
                source,
                &mut decoded,
                &mut admission_bytes,
            )?),
            Event::Html(text) => Token::Html(intern_text(
                text.as_ref(),
                source,
                &mut decoded,
                &mut admission_bytes,
            )?),
            Event::InlineHtml(text) => Token::InlineHtml(intern_text(
                text.as_ref(),
                source,
                &mut decoded,
                &mut admission_bytes,
            )?),
            other => {
                admission_bytes += event_heap_bytes(&other);
                if admission_bytes > INDEX_BYTES {
                    return Err(MarkdownIndexError::IndexBudget);
                }
                Token::Structural(other.into_static())
            }
        };
        current.leaf.events.push(IndexedEvent { token, range });
        admission_bytes += 2 * std::mem::size_of::<IndexedEvent>();
        if admission_bytes > INDEX_BYTES {
            return Err(MarkdownIndexError::IndexBudget);
        }
        if finishes {
            result.leaves.push(active.take().unwrap().leaf);
            for item in &mut items {
                item.first = false;
            }
        }
    }
    if let Some(mut current) = active {
        if current.implicit {
            current.leaf.events.push(IndexedEvent {
                token: Token::Structural(Event::End(TagEnd::Paragraph)),
                range: current.leaf.range.clone(),
            });
        }
        result.leaves.push(current.leaf);
    }
    result.decoded = decoded.into_iter().collect();
    if result.resident_bytes() > INDEX_BYTES {
        return Err(MarkdownIndexError::IndexBudget);
    }
    Ok(result)
}

fn record_spacing(
    root: &mut Option<BlockShape>,
    items: &mut [OpenItem],
    spacing: &mut Vec<LeafSpacing>,
    shape: BlockShape,
) {
    let in_item = !items.is_empty();
    let previous = if let Some(item) = items.last_mut() {
        item.previous.replace(shape)
    } else {
        root.replace(shape)
    };
    spacing.push(LeafSpacing::Sibling {
        previous,
        current: shape,
        in_item,
    });
}
fn intern_context(
    context: LeafContext,
    owned: &mut Vec<LeafContext>,
    pool: &mut HashMap<LeafContext, usize>,
    bytes: &mut usize,
) -> usize {
    if let Some(id) = pool.get(&context) {
        return *id;
    }
    *bytes += 2
        * (std::mem::size_of::<LeafContext>()
            + context.lists.capacity() * std::mem::size_of::<ListLevel>()
            + context.table_alignments.capacity() * std::mem::size_of::<TableAlignment>()
            + context.spacing.capacity() * std::mem::size_of::<LeafSpacing>()
            + 64);
    let id = owned.len();
    owned.push(context.clone());
    pool.insert(context, id);
    id
}

fn intern_text(
    text: &str,
    source: &str,
    decoded: &mut HashSet<Arc<str>>,
    bytes: &mut usize,
) -> Result<Text, MarkdownIndexError> {
    let start = (text.as_ptr() as usize).checked_sub(source.as_ptr() as usize);
    if let Some(start) = start.filter(|start| *start <= source.len())
        && source.get(start..start.saturating_add(text.len())) == Some(text)
    {
        return Ok(Text::Range(start..start + text.len()));
    }
    if let Some(known) = decoded.get(text) {
        return Ok(Text::Decoded(known.clone()));
    }
    if bytes.saturating_add(text.len() + 2 * std::mem::size_of::<usize>() + 64) > INDEX_BYTES {
        return Err(MarkdownIndexError::IndexBudget);
    }
    let text: Arc<str> = Arc::from(text);
    *bytes = bytes.saturating_add(text.len() + 2 * std::mem::size_of::<usize>() + 64);
    decoded.insert(text.clone());
    Ok(Text::Decoded(text))
}
