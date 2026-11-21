use super::PARSE_OPTIONS;
use super::{
    Arc, BlockShape, BuildIndexError, Event, INDEX_BYTES, IndexedEvent, IndexedLeaf, ItemStart,
    Kind, LeafContext, LeafSpacing, ListLevel, ListMark, ListShape, MarkdownIndex, Marker, Resume,
    SOURCE_BYTES, TableAlignment, Tag, TagEnd, Text, Token, event_heap_bytes,
};
use pulldown_cmark::Parser;
use std::collections::{HashMap, HashSet};

#[derive(Clone)]
struct OpenList {
    start: Option<u64>,
    source_start: usize,
    ordinal: u64,
    top: bool,
    loose: Option<bool>,
}

impl OpenList {
    const fn mark(&self) -> ListMark {
        ListMark {
            start: self.start,
            source_start: self.source_start,
            ordinal: self.ordinal,
        }
    }
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

pub(crate) fn build(source: &str) -> Result<MarkdownIndex, BuildIndexError> {
    Ok(build_from(source, 0, Resume::Start)?
        .expect("invariant: a build from the start resumes no list"))
}

#[expect(
    clippy::too_many_lines,
    reason = "one pass over the parser's events, keeping list, table and leaf state together"
)]
#[expect(
    clippy::wildcard_enum_match_arm,
    reason = "pulldown-cmark's events and tags: only the ones named here shape the index"
)]
pub(crate) fn build_from(
    source: &str,
    start: usize,
    resume: Resume,
) -> Result<Option<MarkdownIndex>, BuildIndexError> {
    if source.len() > SOURCE_BYTES {
        return Err(BuildIndexError::SourceTooLarge {
            bytes: source.len(),
        });
    }
    let mut result = MarkdownIndex {
        leaves: Vec::new(),
        contexts: Vec::new(),
        decoded: Vec::new(),
        source_bytes: source.len(),
        definitions: false,
        items: Vec::new(),
        lists: Vec::new(),
    };
    let mut contexts = HashMap::<LeafContext, usize>::new();
    let mut decoded = HashSet::<Arc<str>>::new();
    let mut lists = Vec::<OpenList>::new();
    let mut items = Vec::<OpenItem>::new();
    let mut alignments = Vec::<TableAlignment>::new();
    let mut quote_depth = 0;
    let (mut root_previous, mut continued) = match resume {
        Resume::Start => (None, None),
        Resume::After(shape) => (Some(shape), None),
        Resume::Item { list, marker } => (Some(BlockShape::List), Some((list, marker))),
    };
    let mut spacing = Vec::new();
    let mut table_source = None;
    let mut depth = 0usize;
    let mut active: Option<ActiveLeaf> = None;
    let mut admission_bytes = 0usize;
    let suffix = source
        .get(start..)
        .expect("invariant: an index is rebuilt from a line boundary inside its source");
    let parser = Parser::new_ext(suffix, PARSE_OPTIONS);
    result.definitions = parser.reference_definitions().iter().next().is_some();
    for (event, range) in parser
        .into_offset_iter()
        .map(|(event, range)| (event, range.start + start..range.end + start))
    {
        if let Some((list, marker)) = continued
            && !continues_at(source, start, &event, &range, list, marker)
        {
            return Ok(None);
        }
        if admission_bytes > INDEX_BYTES {
            return Err(BuildIndexError::IndexTooLarge);
        }
        if let Event::Start(_) = &event {
            depth += 1;
            if depth > 96 {
                return Err(BuildIndexError::NestingTooDeep);
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
        if boundary && let Some(implicit) = active.take_if(|a| a.implicit) {
            let mut leaf = implicit.leaf;
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
            && let Some(source_start) = items
                .last()
                .filter(|item| item.previous.is_none())
                .map(|item| item.source_start)
        {
            let context_id = intern_context(
                leaf_context(
                    &lists,
                    &items,
                    quote_depth,
                    &alignments,
                    table_source,
                    std::mem::take(&mut spacing),
                ),
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
            admission_bytes += 2 * (size_of::<IndexedLeaf>() + 2 * size_of::<IndexedEvent>());
            for item in &mut items {
                item.first = false;
            }
        }
        if active.is_none() {
            match &event {
                Event::Start(Tag::List(start)) => {
                    let top = lists.is_empty() && quote_depth == 0;
                    let list = continued.take().map(|(list, _)| list).map_or_else(
                        || {
                            record_spacing(
                                &mut root_previous,
                                &mut items,
                                &mut spacing,
                                BlockShape::List,
                            );
                            OpenList {
                                start: *start,
                                source_start: range.start,
                                ordinal: 0,
                                top,
                                loose: None,
                            }
                        },
                        |mark| OpenList {
                            start: mark.start,
                            source_start: mark.source_start,
                            ordinal: mark.ordinal,
                            top,
                            loose: None,
                        },
                    );
                    lists.push(list);
                    continue;
                }
                Event::Start(Tag::Item) => {
                    if lists.last().is_some_and(|list| list.ordinal > 0) {
                        spacing.push(LeafSpacing::Item);
                    }
                    if let [list] = lists.as_slice()
                        && list.top
                        && let Some(line_start) = marker_line(source, range.start)
                        && let Some(marker) = source.get(line_start..).and_then(marker_of)
                    {
                        result.items.push(ItemStart {
                            line_start,
                            first_leaf: result.leaves.len(),
                            list: list.mark(),
                            marker,
                        });
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
                    if let Some(list) = lists.pop().filter(|list| list.top) {
                        result.lists.push(ListShape {
                            source_start: list.source_start,
                            loose: list.loose,
                        });
                    }
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
            let context_id = intern_context(
                leaf_context(
                    &lists,
                    &items,
                    quote_depth,
                    &alignments,
                    table_source,
                    std::mem::take(&mut spacing),
                ),
                &mut result.contexts,
                &mut contexts,
                &mut admission_bytes,
            );
            let implicit =
                kind == Kind::Paragraph && !matches!(&event, Event::Start(Tag::Paragraph));
            if kind == Kind::Paragraph
                && quote_depth == 0
                && items.len() == 1
                && let [list] = lists.as_mut_slice()
            {
                list.loose.get_or_insert(!implicit);
            }
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
            admission_bytes += 2 * size_of::<IndexedLeaf>();
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
                    return Err(BuildIndexError::IndexTooLarge);
                }
                Token::Structural(other.into_static())
            }
        };
        current.leaf.events.push(IndexedEvent { token, range });
        admission_bytes += 2 * size_of::<IndexedEvent>();
        if admission_bytes > INDEX_BYTES {
            return Err(BuildIndexError::IndexTooLarge);
        }
        if finishes && let Some(done) = active.take() {
            result.leaves.push(done.leaf);
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
    if continued.is_some() {
        return Ok(None);
    }
    result.decoded = decoded.into_iter().collect();
    if result.resident_bytes() > INDEX_BYTES {
        return Err(BuildIndexError::IndexTooLarge);
    }
    Ok(Some(result))
}

fn continues_at(
    source: &str,
    line_start: usize,
    event: &Event<'_>,
    range: &std::ops::Range<usize>,
    list: ListMark,
    marker: Marker,
) -> bool {
    matches!(event, Event::Start(Tag::List(first)) if first.is_some() == list.start.is_some())
        && marker_line(source, range.start) == Some(line_start)
        && source.get(line_start..).and_then(marker_of) == Some(marker)
}

fn marker_of(line: &str) -> Option<Marker> {
    let indent = line.len() - line.trim_start_matches(' ').len();
    let rest = line.get(indent..)?;
    let digits = rest.len() - rest.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    let delimiter = *rest.as_bytes().get(digits)?;
    let valid = if digits == 0 {
        matches!(delimiter, b'-' | b'*' | b'+')
    } else {
        matches!(delimiter, b'.' | b')')
    };
    valid.then_some(Marker { indent, delimiter })
}

fn marker_line(source: &str, marker: usize) -> Option<usize> {
    let before = source.get(..marker)?;
    let line_start = before.rfind('\n').map_or(0, |newline| newline + 1);
    before
        .get(line_start..)
        .is_some_and(|indent| indent.len() <= 3 && indent.bytes().all(|byte| byte == b' '))
        .then_some(line_start)
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
fn leaf_context(
    lists: &[OpenList],
    items: &[OpenItem],
    quote_depth: usize,
    alignments: &[TableAlignment],
    table_source: Option<usize>,
    spacing: Vec<LeafSpacing>,
) -> LeafContext {
    LeafContext {
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
        table_alignments: alignments.to_vec(),
        table_source,
        spacing,
    }
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
        * (size_of::<LeafContext>()
            + context.lists.capacity() * size_of::<ListLevel>()
            + context.table_alignments.capacity() * size_of::<TableAlignment>()
            + context.spacing.capacity() * size_of::<LeafSpacing>()
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
) -> Result<Text, BuildIndexError> {
    let start = (text.as_ptr() as usize).checked_sub(source.as_ptr() as usize);
    if let Some(start) = start.filter(|start| *start <= source.len())
        && source.get(start..start.saturating_add(text.len())) == Some(text)
    {
        return Ok(Text::Range(start..start + text.len()));
    }
    if let Some(known) = decoded.get(text) {
        return Ok(Text::Decoded(Arc::clone(known)));
    }
    if bytes.saturating_add(text.len() + 2 * size_of::<usize>() + 64) > INDEX_BYTES {
        return Err(BuildIndexError::IndexTooLarge);
    }
    let text: Arc<str> = Arc::from(text);
    *bytes = bytes.saturating_add(text.len() + 2 * size_of::<usize>() + 64);
    decoded.insert(Arc::clone(&text));
    Ok(Text::Decoded(text))
}
