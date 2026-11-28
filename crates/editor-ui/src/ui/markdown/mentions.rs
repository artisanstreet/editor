use super::{Block, IndexedLeafContent, Span};
use crate::ui::text::slice::slice;
use artisan_mentions::{MessageMention, MessageVocabulary};
use pulldown_cmark::{Event, Tag, TagEnd};
use std::ops::Range;
use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct MessageMarks {
    command: Option<String>,
    vocabulary: Arc<MessageVocabulary>,
}

impl PartialEq for MessageMarks {
    fn eq(&self, other: &Self) -> bool {
        self.command == other.command
            && (Arc::ptr_eq(&self.vocabulary, &other.vocabulary)
                || self.vocabulary == other.vocabulary)
    }
}

impl MessageMarks {
    #[must_use]
    pub(crate) fn new(source: &str, vocabulary: &Arc<MessageVocabulary>) -> Self {
        Self {
            command: artisan_mentions::leading_command(source)
                .and_then(|range| source.get(range))
                .map(str::to_owned),
            vocabulary: Arc::clone(vocabulary),
        }
    }

    pub(super) fn command(&self, ordinal: usize) -> Option<&str> {
        self.command.as_deref().filter(|_| ordinal == 0)
    }

    pub(super) fn vocabulary(&self) -> &MessageVocabulary {
        &self.vocabulary
    }

    pub(super) fn adopt(&mut self, vocabulary: &Arc<MessageVocabulary>) {
        self.vocabulary = Arc::clone(vocabulary);
    }

    pub(super) fn mark_command(&self, ordinal: usize, content: &mut IndexedLeafContent) {
        if let (Some(command), IndexedLeafContent::Block(Block::Paragraph { spans, .. })) =
            (self.command(ordinal), content)
        {
            super::mark_command_spans(spans, command);
        }
    }
}

pub(crate) struct BalancedEvents(Vec<(Event<'static>, Range<usize>)>);

impl BalancedEvents {
    pub(crate) fn replay(
        record: &artisan_markdown_index::LeafRecord,
        start: usize,
        source: &str,
    ) -> Result<Self, super::MarkdownIndexError> {
        let events = record.replay(start, source)?;
        let leaf = record.leaf.source_range();
        let mut open: Vec<TagEnd> = Vec::new();
        for (event, range) in &events {
            match event {
                Event::Start(tag) => open.push(tag.to_end()),
                Event::End(end) if open.pop() == Some(*end) => {}
                Event::End(_) => {
                    return Err(super::MarkdownIndexError::UnbalancedEvents {
                        leaf,
                        token: range.clone(),
                    });
                }
                Event::Text(_)
                | Event::Code(_)
                | Event::InlineMath(_)
                | Event::DisplayMath(_)
                | Event::Html(_)
                | Event::InlineHtml(_)
                | Event::FootnoteReference(_)
                | Event::SoftBreak
                | Event::HardBreak
                | Event::Rule
                | Event::TaskListMarker(_) => {}
            }
        }
        if open.is_empty() {
            Ok(Self(events))
        } else {
            Err(super::MarkdownIndexError::UnclosedEvents {
                leaf,
                open: open.len(),
            })
        }
    }

    pub(crate) fn into_events(self) -> Vec<(Event<'static>, Range<usize>)> {
        self.0
    }
}

fn may_mention(leaf: &str) -> bool {
    leaf.contains(['$', '@'])
}

pub(crate) fn record_mentions(
    record: &artisan_markdown_index::LeafRecord,
    start: usize,
    source: &str,
    vocabulary: &MessageVocabulary,
) -> Result<Vec<MessageMention>, super::MarkdownIndexError> {
    let exact = record.exact_source(start, source)?;
    if !may_mention(exact) {
        return Ok(Vec::new());
    }
    Ok(resolve(
        &BalancedEvents::replay(record, start, source)?,
        exact,
        vocabulary,
    ))
}

pub(super) fn resolve(
    events: &BalancedEvents,
    leaf: &str,
    vocabulary: &MessageVocabulary,
) -> Vec<MessageMention> {
    if !may_mention(leaf) {
        return Vec::new();
    }
    let mut found = Vec::new();
    for segment in segments(&events.0, leaf) {
        let text = slice(leaf, segment.clone());
        let links = super::inline::bare_url_ranges(text);
        found.extend(
            vocabulary
                .candidates(text)
                .into_iter()
                .filter(|mention| !any_intersecting(&links, &mention.range))
                .map(|mention| MessageMention {
                    kind: mention.kind,
                    range: mention.range.start + segment.start..mention.range.end + segment.start,
                }),
        );
    }
    if found.is_empty() {
        return found;
    }
    let eligibility = Eligibility::new(&events.0, leaf);
    found.retain(|mention| eligibility.allows(&mention.range));
    artisan_mentions::exclusive(found)
}

fn segments(events: &[(Event<'_>, Range<usize>)], leaf: &str) -> Vec<Range<usize>> {
    let cells: Vec<Range<usize>> = events
        .iter()
        .filter(|(event, _)| matches!(event, Event::Start(Tag::TableCell)))
        .map(|(_, range)| range.clone())
        .collect();
    if cells.is_empty() {
        std::iter::once(0..leaf.len()).collect()
    } else {
        cells
    }
}

fn first_ending_after(sorted: &[Range<usize>], at: usize) -> Option<&Range<usize>> {
    sorted.get(sorted.partition_point(|range| range.end <= at))
}

fn any_intersecting(sorted: &[Range<usize>], range: &Range<usize>) -> bool {
    first_ending_after(sorted, range.start).is_some_and(|other| intersects(range, other))
}

fn intersects(range: &Range<usize>, other: &Range<usize>) -> bool {
    range.start < other.end && other.start < range.end
}

struct Eligibility {
    splits: Vec<usize>,
    blocked: Vec<Range<usize>>,
    prose: Vec<Range<usize>>,
}

impl Eligibility {
    fn new(events: &[(Event<'_>, Range<usize>)], leaf: &str) -> Self {
        let effects = html_effects(events);
        let mut splits = Vec::new();
        let mut blocked = Vec::new();
        let mut prose = Vec::new();
        let mut hidden = 0usize;
        let mut html = 0usize;
        for ((event, range), effect) in events.iter().zip(effects) {
            let readable = match event {
                Event::Start(tag) => {
                    if hides_text(tag) {
                        hidden += 1;
                    }
                    if !inline_format(tag) {
                        splits.push(range.start);
                    }
                    continue;
                }
                Event::End(tag) => {
                    if ends_hidden_text(*tag) {
                        hidden = hidden.checked_sub(1).expect(
                            "invariant: balanced leaf events close only the links they opened",
                        );
                    }
                    if !ends_inline_format(*tag) {
                        splits.push(range.end);
                    }
                    continue;
                }
                Event::InlineHtml(_) => {
                    html = match effect {
                        HtmlEffect::Opens => html + 1,
                        HtmlEffect::Closes => html
                            .checked_sub(1)
                            .expect("invariant: an inline HTML close follows the open it matched"),
                        HtmlEffect::Inert => html,
                    };
                    false
                }
                Event::Text(text) => {
                    hidden == 0 && html == 0 && leaf.get(range.clone()) == Some(text.as_ref())
                }
                Event::Code(_)
                | Event::InlineMath(_)
                | Event::DisplayMath(_)
                | Event::Html(_)
                | Event::FootnoteReference(_)
                | Event::SoftBreak
                | Event::HardBreak
                | Event::Rule
                | Event::TaskListMarker(_) => false,
            };
            if readable {
                prose.push(range.clone());
            } else {
                blocked.push(range.clone());
            }
        }
        splits.sort_unstable();
        blocked.sort_unstable_by_key(|range| (range.start, range.end));
        prose.sort_unstable_by_key(|range| (range.start, range.end));
        Self {
            splits,
            blocked,
            prose,
        }
    }

    fn allows(&self, mention: &Range<usize>) -> bool {
        let split = self
            .splits
            .get(self.splits.partition_point(|at| *at <= mention.start))
            .is_some_and(|at| *at < mention.end);
        let led = first_ending_after(&self.prose, mention.start)
            .is_some_and(|range| range.start <= mention.start);
        !split && led && !any_intersecting(&self.blocked, mention)
    }
}

const fn hides_text(tag: &Tag<'_>) -> bool {
    matches!(
        tag,
        Tag::Link { .. } | Tag::Image { .. } | Tag::CodeBlock(_) | Tag::HtmlBlock
    )
}

const fn ends_hidden_text(tag: TagEnd) -> bool {
    matches!(
        tag,
        TagEnd::Link | TagEnd::Image | TagEnd::CodeBlock | TagEnd::HtmlBlock
    )
}

const fn inline_format(tag: &Tag<'_>) -> bool {
    matches!(
        tag,
        Tag::Emphasis | Tag::Strong | Tag::Strikethrough | Tag::Superscript | Tag::Subscript
    )
}

const fn ends_inline_format(tag: TagEnd) -> bool {
    matches!(
        tag,
        TagEnd::Emphasis
            | TagEnd::Strong
            | TagEnd::Strikethrough
            | TagEnd::Superscript
            | TagEnd::Subscript
    )
}

const VOID_ELEMENTS: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "source", "track",
    "wbr",
];

#[derive(Clone, Debug, PartialEq, Eq)]
enum HtmlTag {
    Open(String),
    Close(String),
    Standalone,
}

fn html_tag(html: &str) -> HtmlTag {
    let inner = html.trim();
    let (closing, rest) = match inner.strip_prefix("</") {
        Some(rest) => (true, rest),
        None => (false, inner.strip_prefix('<').unwrap_or(inner)),
    };
    let name = rest
        .chars()
        .take_while(|character| character.is_ascii_alphanumeric() || *character == '-')
        .collect::<String>()
        .to_ascii_lowercase();
    if !name.starts_with(|first: char| first.is_ascii_alphabetic()) {
        return HtmlTag::Standalone;
    }
    if closing {
        return HtmlTag::Close(name);
    }
    if inner.ends_with("/>") || VOID_ELEMENTS.contains(&name.as_str()) {
        HtmlTag::Standalone
    } else {
        HtmlTag::Open(name)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HtmlEffect {
    Opens,
    Closes,
    Inert,
}

fn html_effects(events: &[(Event<'_>, Range<usize>)]) -> Vec<HtmlEffect> {
    let mut effects = vec![HtmlEffect::Inert; events.len()];
    let mut open: std::collections::HashMap<String, Vec<usize>> = std::collections::HashMap::new();
    for (index, (event, _)) in events.iter().enumerate() {
        let html = match event {
            Event::Start(Tag::TableCell) => {
                open.clear();
                continue;
            }
            Event::InlineHtml(html) => html,
            Event::Start(_)
            | Event::End(_)
            | Event::Text(_)
            | Event::Code(_)
            | Event::InlineMath(_)
            | Event::DisplayMath(_)
            | Event::Html(_)
            | Event::FootnoteReference(_)
            | Event::SoftBreak
            | Event::HardBreak
            | Event::Rule
            | Event::TaskListMarker(_) => continue,
        };
        match html_tag(html) {
            HtmlTag::Open(name) => open.entry(name).or_default().push(index),
            HtmlTag::Close(name) => {
                if let Some(opened) = open.get_mut(&name).and_then(Vec::pop) {
                    effects[opened] = HtmlEffect::Opens;
                    effects[index] = HtmlEffect::Closes;
                }
            }
            HtmlTag::Standalone => {}
        }
    }
    effects
}

pub(super) fn split_text(
    text: &str,
    range: &Range<usize>,
    mentions: &[MessageMention],
) -> Vec<Span> {
    let mut spans = Vec::new();
    let mut cursor = 0;
    let first = mentions.partition_point(|mention| mention.range.end <= range.start);
    for mention in mentions
        .iter()
        .skip(first)
        .take_while(|mention| mention.range.start < range.end)
    {
        let start = mention.range.start.max(range.start) - range.start;
        let end = mention.range.end.min(range.end) - range.start;
        if cursor < start {
            spans.push(Span::Text(slice(text, cursor..start).to_owned()));
        }
        spans.push(if range.contains(&mention.range.start) {
            Span::MentionLead {
                kind: mention.kind,
                name: slice(text, start + mention.kind.sigil().len_utf8()..end).to_owned(),
            }
        } else {
            Span::MentionTail {
                kind: mention.kind,
                text: slice(text, start..end).to_owned(),
            }
        });
        cursor = end;
    }
    if cursor < text.len() {
        spans.push(Span::Text(slice(text, cursor..).to_owned()));
    }
    spans
}

#[cfg(test)]
#[path = "../../../tests/markdown/mentions.rs"]
mod tests;
