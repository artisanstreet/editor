//! Markdown syntax through maintained pulldown-cmark and syntect parsers.
//!
//! Owned blocks retain authored source order, ranges, task markers, and tables.
//! Raw HTML remains inert; images retain alt text. The renderer owns theme and
//! interaction, while adapters own provider normalization.

#![expect(
    clippy::module_name_repetitions,
    reason = "public names stay self-describing at use sites"
)]

/// Shared source budget for parsed/rendered Markdown previews.
pub(crate) const MAX_MARKDOWN_SOURCE_BYTES: usize = 256 * 1024;

use std::ops::Range;
use std::str::FromStr;
use std::sync::Arc;

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Parser, Tag, TagEnd};
use syntect::highlighting::ScopeSelectors;
use syntect::parsing::{ParseState, ScopeStack, SyntaxSet};
use thiserror::Error;

mod source_index;
mod table;
pub use source_index::{
    BlockShape, IndexedLeaf, IndexedLeafContent, LeafContext, LeafSpacing, ListLevel,
    MarkdownIndex, MarkdownIndexError, normalize_record,
};

use table::TableBuilder;
pub use table::{Table, TableAlignment, TableCell};

/// Failure crossing the Markdown engine boundary.
///
/// Malformed syntax degrades to plain text. Excessive preview work is rejected
/// before constructing deeply nested trees or highlighting oversized input.
#[derive(Debug, Error)]
pub enum MarkdownError {
    /// The preview exceeds the parser's source, work or nesting ceiling.
    #[error("Markdown preview exceeds {0}; open the full source")]
    PreviewLimit(&'static str),
    /// `syntect` failed to parse a highlighted line against its grammar set.
    #[error("syntax highlighting failed while parsing a line")]
    LineParse(#[from] syntect::parsing::ParsingError),
    /// A scope-stack operation produced by `syntect` could not be applied.
    #[error("syntax highlighting failed while applying a scope operation")]
    ScopeApply(#[from] syntect::parsing::ScopeError),
    /// A built-in scope selector used for token classification was rejected.
    ///
    /// This indicates a defect in the fixed rule table, not in user input.
    #[error("built-in scope selector `{selector}` failed to parse")]
    Selector {
        /// The selector literal that `syntect` refused.
        selector: &'static str,
        /// Why `syntect` refused it.
        #[source]
        cause: syntect::parsing::ParseScopeError,
    },
}

/// Semantic class assigned to a highlighted source range.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum CodeTokenKind {
    /// Comments.
    Comment,
    /// Quoted string literals.
    Str,
    /// Numeric literals.
    Number,
    /// Keywords and control-flow words.
    Keyword,
    /// Type-like identifiers (`storage.type`, declared type names).
    Type,
    /// Function and method names at definition or call sites.
    Function,
    /// Anything the classifier leaves unclassified.
    Plain,
}

/// One classified byte range inside a [`CodeFence::source`].
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct CodeToken {
    /// Semantic class resolved through the `syntect` scope stack.
    pub kind: CodeTokenKind,
    /// Half-open byte range into the owning fence source.
    pub range: Range<usize>,
}

/// Worker-prepared SVG bytes and dimensions, independent of GPUI state.
/// `source` is painted in the dark theme's tokens, `light_source` in the
/// light theme's; both share one layout.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct DiagramSvg {
    pub source: String,
    pub light_source: String,
    pub width_bits: u32,
    pub height_bits: u32,
}

/// An owned fenced or indented code block.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct CodeFence {
    /// Normalized language token from the info string, lowercased and taken
    /// from the first whitespace-delimited word; `None` for indented blocks
    /// and bare fences.
    pub language: Option<String>,
    /// Whether the closing fence line was present in the parsed input. An
    /// open fence always reports [`None`] tokens so streaming consumers can
    /// fall back to plain presentation until the fence settles.
    pub closed: bool,
    /// Verbatim fence body, newlines included.
    pub source: String,
    /// Classified byte ranges over `source`, ordered and non-overlapping;
    /// `None` while the fence is open or the language is unrecognized.
    pub tokens: Option<Arc<[CodeToken]>>,
    /// Light and dark Mermaid renditions prepared off the UI thread.
    pub diagram: Option<Arc<DiagramSvg>>,
}

/// One flattened inline run.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum Span {
    /// Ordinary text, including flattened image alt runs.
    Text(String),
    /// Inline code payload taken verbatim between backticks.
    Code(String),
    /// Raw inline HTML, carried verbatim as inert data.
    Html(String),
    /// The special `/command` a user's message starts with, painted as the
    /// composer paints it. Never parsed: [`MarkdownDocument::mark_command`]
    /// sets it.
    Command(String),
    /// Emphasized runs (`*x*`, `_x_`); structure is preserved, rendering
    /// stays plain until the native text primitive exposes italics.
    Emphasis(Vec<Self>),
    /// Strong runs (`**x**`); rendered bold through the native highlight.
    Strong(Vec<Self>),
    /// Link label plus the verbatim authored destination. The engine keeps
    /// every destination; the renderer decides safety and never executes one.
    Link {
        /// Visible label runs.
        label: Vec<Self>,
        /// Verbatim destination URL as authored.
        destination: String,
    },
}

impl Span {
    /// Flattens every visible label to plain text for no-drop assertions.
    /// Destinations stay out: they are metadata, not visible copy.
    #[must_use]
    pub fn text_content(&self) -> String {
        match self {
            Self::Text(text) | Self::Code(text) | Self::Html(text) | Self::Command(text) => {
                text.clone()
            }
            Self::Emphasis(inner) | Self::Strong(inner) => {
                inner.iter().map(Self::text_content).collect()
            }
            Self::Link { label, .. } => label.iter().map(Self::text_content).collect(),
        }
    }
}

/// Flattens a span slice to its visible text.
#[must_use]
pub fn spans_text(spans: &[Span]) -> String {
    spans.iter().map(Span::text_content).collect()
}

/// One recognized top-level block.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum Block {
    /// An ATX heading.
    Heading {
        /// Depth from one to six.
        level: u8,
        /// Flattened inline runs.
        spans: Vec<Span>,
        /// Half-open byte range covering the heading in the parsed input.
        range: Range<usize>,
    },
    /// A paragraph.
    Paragraph {
        /// Flattened inline runs.
        spans: Vec<Span>,
        /// Half-open byte range covering the paragraph in the parsed input.
        range: Range<usize>,
    },
    /// A fenced or indented code block.
    Code(CodeFence),
    /// A raw HTML block, carried verbatim as inert data.
    Html {
        /// Verbatim HTML source text.
        source: String,
    },
    /// An ordered or unordered list with owned items. Nested lists live as
    /// blocks inside their parent item, so tight, loose, and nested shapes
    /// share one renderer path.
    List {
        /// True for numbered lists, false for bulleted lists.
        ordered: bool,
        /// Authored start number for ordered lists, if any.
        start: Option<u64>,
        /// Items in source order.
        items: Vec<ListItem>,
        /// Half-open byte range covering the list in the parsed input.
        range: Range<usize>,
    },
    /// A GFM table of inline-only cells.
    Table(Table),
}

impl Block {
    /// Flattens every visible label under this block for no-drop assertions.
    #[must_use]
    pub fn text_content(&self) -> String {
        match self {
            Self::Heading { spans, .. } | Self::Paragraph { spans, .. } => spans_text(spans),
            Self::Code(fence) => fence.source.clone(),
            Self::Html { source } => source.clone(),
            Self::List { items, .. } => items
                .iter()
                .flat_map(|item| {
                    item.blocks
                        .iter()
                        .map(Self::text_content)
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>()
                .join("\n"),
            Self::Table(table) => table.text_content(),
        }
    }
}

/// One list item: its own blocks plus an optional task marker.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ListItem {
    /// Item content in source order: tight text arrives as one paragraph,
    /// loose text as paragraph blocks, nested lists and fences as siblings.
    pub blocks: Vec<Block>,
    /// Task-list state from `pulldown-cmark`'s `TaskListMarker`, if any.
    pub task: Option<bool>,
}

impl ListItem {
    /// Flattens the item's visible text.
    #[must_use]
    pub fn text_content(&self) -> String {
        self.blocks
            .iter()
            .map(Block::text_content)
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Owned parse result: an ordered list of blocks.
#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct MarkdownDocument {
    pub(crate) blocks: Vec<Block>,
}

impl MarkdownDocument {
    /// Marks `command` where the document opens with it as plain text: the
    /// first paragraph's leading run.
    pub(crate) fn mark_command(&mut self, command: &str) {
        let Some(Block::Paragraph { spans, .. }) = self.blocks.first_mut() else {
            return;
        };
        let Some(Span::Text(text)) = spans.first_mut() else {
            return;
        };
        let Some(rest) = text.strip_prefix(command) else {
            return;
        };
        let rest = rest.to_owned();
        spans[0] = Span::Command(command.to_owned());
        if !rest.is_empty() {
            spans.insert(1, Span::Text(rest));
        }
    }

    /// Heap allocations retained by the owned syntax tree (shared tokens
    /// are conservatively charged to each document referencing them).
    pub(crate) fn resident_bytes(&self) -> usize {
        fn spans_bytes(spans: &Vec<Span>) -> usize {
            spans.capacity() * std::mem::size_of::<Span>()
                + spans
                    .iter()
                    .map(|span| match span {
                        Span::Text(text)
                        | Span::Code(text)
                        | Span::Html(text)
                        | Span::Command(text) => text.capacity(),
                        Span::Emphasis(inner) | Span::Strong(inner) => spans_bytes(inner),
                        Span::Link { label, destination } => {
                            spans_bytes(label) + destination.capacity()
                        }
                    })
                    .sum::<usize>()
        }
        fn blocks_bytes(blocks: &Vec<Block>) -> usize {
            blocks.capacity() * std::mem::size_of::<Block>()
                + blocks
                    .iter()
                    .map(|block| match block {
                        Block::Heading { spans, .. } | Block::Paragraph { spans, .. } => {
                            spans_bytes(spans)
                        }
                        Block::Code(fence) => {
                            fence.diagram.as_ref().map_or(0, |svg| {
                                svg.source.capacity() + svg.light_source.capacity()
                            }) + fence.source.capacity()
                                + fence.language.as_ref().map_or(0, String::capacity)
                                + fence
                                    .tokens
                                    .as_ref()
                                    .map_or(0, |tokens| std::mem::size_of_val(tokens.as_ref()))
                        }
                        Block::Html { source } => source.capacity(),
                        Block::List { items, .. } => {
                            items.capacity() * std::mem::size_of::<ListItem>()
                                + items
                                    .iter()
                                    .map(|item| blocks_bytes(&item.blocks))
                                    .sum::<usize>()
                        }
                        Block::Table(table) => {
                            table.alignments.capacity() * std::mem::size_of::<TableAlignment>()
                                + table.header.capacity() * std::mem::size_of::<TableCell>()
                                + table.rows.capacity() * std::mem::size_of::<Vec<TableCell>>()
                                + table
                                    .rows
                                    .iter()
                                    .map(|row| row.capacity() * std::mem::size_of::<TableCell>())
                                    .sum::<usize>()
                                + table
                                    .header
                                    .iter()
                                    .chain(table.rows.iter().flatten())
                                    .map(|cell| spans_bytes(&cell.spans))
                                    .sum::<usize>()
                        }
                    })
                    .sum::<usize>()
        }
        std::mem::size_of::<Self>() + blocks_bytes(&self.blocks)
    }

    /// Returns the parsed blocks in source order.
    #[must_use]
    pub fn blocks(&self) -> &[Block] {
        &self.blocks
    }
}

/// Shared Markdown engine holding the loaded `syntect` grammar set and the
/// fixed scope-classifier table. Construct once and reuse.
#[derive(Debug)]
pub struct MarkdownEngine {
    syntax_set: SyntaxSet,
    classifiers: Vec<(ScopeSelectors, CodeTokenKind)>,
}

/// Priority-ordered scope-selector rules; the first matching rule wins.
const CLASSIFIER_RULES: &[(&str, CodeTokenKind)] = &[
    ("comment", CodeTokenKind::Comment),
    ("string", CodeTokenKind::Str),
    ("constant.numeric", CodeTokenKind::Number),
    ("keyword", CodeTokenKind::Keyword),
    ("storage.type", CodeTokenKind::Type),
    (
        "entity.name.function, support.function",
        CodeTokenKind::Function,
    ),
];

impl MarkdownEngine {
    /// Loads the bundled grammars and builds the classifier table.
    ///
    /// # Errors
    ///
    /// Returns [`MarkdownError::Selector`] if `syntect` rejects one of the
    /// built-in selector literals, which would otherwise leave classification
    /// silently incomplete.
    pub fn new() -> Result<Self, MarkdownError> {
        let classifiers = CLASSIFIER_RULES
            .iter()
            .map(|(selector, kind)| {
                ScopeSelectors::from_str(selector)
                    .map(|selectors| (selectors, *kind))
                    .map_err(|cause| MarkdownError::Selector { selector, cause })
            })
            .collect::<Result<Vec<_>, MarkdownError>>()?;

        Ok(Self {
            syntax_set: SyntaxSet::load_defaults_newlines(),
            classifiers,
        })
    }

    /// Parses `source` into an owned [`MarkdownDocument`], highlighting every
    /// closed fenced block whose language resolves to a bundled grammar.
    ///
    /// # Errors
    ///
    /// Returns [`MarkdownError`] when `syntect` fails while highlighting a
    /// recognized closed fence.
    pub fn parse_document(&self, source: &str) -> Result<MarkdownDocument, MarkdownError> {
        self.parse_document_with_fence_memo(source, &mut NoFenceTokenMemo)
    }

    /// Parses like [`Self::parse_document`], consulting `memo` before
    /// classifying each closed fence.
    ///
    /// A streaming reply re-parses its whole body on every delta, but only
    /// the fence being written changes: the memo lets every settled fence
    /// reuse its tokens instead of running `syntect` again.
    pub(crate) fn parse_document_with_fence_memo(
        &self,
        source: &str,
        memo: &mut dyn FenceTokenMemo,
    ) -> Result<MarkdownDocument, MarkdownError> {
        if source.len() > MAX_MARKDOWN_SOURCE_BYTES {
            return Err(MarkdownError::PreviewLimit("256 KiB of source"));
        }
        let mut document = DocumentBuilder::default().build(source)?;
        self.highlight_blocks(&mut document.blocks, memo)?;
        Ok(document)
    }

    /// Highlights every closed fence in `blocks`, including fences nested
    /// inside list items.
    fn highlight_blocks(
        &self,
        blocks: &mut [Block],
        memo: &mut dyn FenceTokenMemo,
    ) -> Result<(), MarkdownError> {
        for block in blocks {
            match block {
                Block::Code(fence) if fence.closed => {
                    if self.highlight_fence(fence, memo).is_err() {
                        fence.tokens = None;
                    }
                }
                Block::List { items, .. } => {
                    for item in items {
                        self.highlight_blocks(&mut item.blocks, memo)?;
                    }
                }
                Block::Heading { .. }
                | Block::Paragraph { .. }
                | Block::Code(_)
                | Block::Html { .. }
                | Block::Table(_) => {}
            }
        }
        Ok(())
    }

    /// Fills `fence.tokens` when the language resolves to a bundled grammar;
    /// leaves `None` for unknown languages and bare or indented fences.
    fn highlight_fence(
        &self,
        fence: &mut CodeFence,
        memo: &mut dyn FenceTokenMemo,
    ) -> Result<(), MarkdownError> {
        let Some(language) = fence.language.as_deref() else {
            return Ok(());
        };
        let Some(syntax) = self.syntax_set.find_syntax_by_token(language) else {
            return Ok(());
        };
        if let Some(tokens) = memo.cached(language, &fence.source) {
            fence.tokens = Some(tokens);
            return Ok(());
        }

        let mut parse_state = ParseState::new(syntax);
        let mut scope_stack = ScopeStack::new();
        let mut pending: Option<(Range<usize>, CodeTokenKind)> = None;
        let mut merged = Vec::new();

        for (offset, line) in LineOffsets::new(&fence.source) {
            let produced = self.highlight_line(
                &mut parse_state,
                &mut scope_stack,
                line,
                offset,
                &mut pending,
            )?;
            for token in produced {
                merge_token(&mut merged, token);
            }
        }
        if let Some((range, kind)) = pending.take() {
            merged.push(CodeToken { kind, range });
        }

        let tokens: Arc<[CodeToken]> = merged.into();
        memo.remember(language, &fence.source, Arc::clone(&tokens));
        fence.tokens = Some(tokens);
        Ok(())
    }

    /// Highlights one physical line, mirroring how `syntect`'s own highlight
    /// iterator turns `(position, operation)` pairs into styled regions: each
    /// segment between consecutive operations inherits the scope stack
    /// accumulated so far.
    fn highlight_line(
        &self,
        parse_state: &mut ParseState,
        scope_stack: &mut ScopeStack,
        line: &str,
        base: usize,
        pending: &mut Option<(Range<usize>, CodeTokenKind)>,
    ) -> Result<Vec<CodeToken>, MarkdownError> {
        let operations = parse_state.parse_line(line, &self.syntax_set)?;
        let mut produced = Vec::new();
        let mut cursor = base;

        for (position, operation) in operations {
            let absolute = base.saturating_add(position);
            self.emit_segment(
                scope_stack,
                line,
                base,
                cursor,
                absolute,
                pending,
                &mut produced,
            );
            cursor = cursor.max(absolute);
            scope_stack.apply(&operation)?;
        }
        let end = base.saturating_add(line.len());
        self.emit_segment(scope_stack, line, base, cursor, end, pending, &mut produced);

        Ok(produced)
    }

    /// Classifies one segment when it is contiguous, in bounds, and contains
    /// visible content; whitespace between tokens stays unclassified.
    #[expect(
        clippy::too_many_arguments,
        reason = "the classifier threads its scratch state through one call"
    )]
    fn emit_segment(
        &self,
        scope_stack: &ScopeStack,
        line: &str,
        base: usize,
        start: usize,
        end: usize,
        pending: &mut Option<(Range<usize>, CodeTokenKind)>,
        produced: &mut Vec<CodeToken>,
    ) {
        if end <= start {
            return;
        }
        let Some(text) = line.get(start - base..end - base) else {
            return;
        };
        // Trailing line noise (the physical newline, padding spaces) stays
        // out of the classified range; leading indentation is kept verbatim.
        let visible = text.trim_end().len();
        if visible == 0 {
            return;
        }
        if let Some(kind) = self.classify(scope_stack) {
            extend(pending, start..start + visible, kind, produced);
        }
    }

    fn classify(&self, scope_stack: &ScopeStack) -> Option<CodeTokenKind> {
        let scopes = scope_stack.as_slice();
        self.classifiers
            .iter()
            .find(|(selectors, _)| selectors.does_match(scopes).is_some())
            .map(|(_, kind)| *kind)
    }
}

/// Memo of classified fence tokens consulted while parsing.
///
/// Tokens are a pure function of the fence language and source bytes, so a
/// memo may serve any earlier classification of the same pair. Only
/// successful classifications are remembered; a `syntect` failure leaves
/// that fence uncolored and is not cached.
pub(crate) trait FenceTokenMemo {
    /// Returns previously classified tokens for this exact language and
    /// source, if the memo holds them.
    fn cached(&mut self, language: &str, source: &str) -> Option<Arc<[CodeToken]>>;

    /// Records the tokens just classified for this language and source.
    fn remember(&mut self, language: &str, source: &str, tokens: Arc<[CodeToken]>);
}

/// The memo behind the uncached [`MarkdownEngine::parse_document`]: every
/// fence classifies from scratch.
struct NoFenceTokenMemo;

impl FenceTokenMemo for NoFenceTokenMemo {
    fn cached(&mut self, _language: &str, _source: &str) -> Option<Arc<[CodeToken]>> {
        None
    }

    fn remember(&mut self, _language: &str, _source: &str, _tokens: Arc<[CodeToken]>) {}
}

/// Yields each physical line together with its starting byte offset.
struct LineOffsets<'a> {
    source: &'a str,
    cursor: usize,
}

impl<'a> LineOffsets<'a> {
    const fn new(source: &'a str) -> Self {
        Self { source, cursor: 0 }
    }
}

impl<'a> Iterator for LineOffsets<'a> {
    type Item = (usize, &'a str);

    fn next(&mut self) -> Option<Self::Item> {
        let remainder = self.source.get(self.cursor..)?;
        match remainder.find('\n') {
            Some(newline) => {
                let item = (self.cursor, &remainder[..=newline]);
                self.cursor += newline + 1;
                Some(item)
            }
            None if remainder.is_empty() => None,
            None => {
                let item = (self.cursor, remainder);
                self.cursor = self.source.len();
                Some(item)
            }
        }
    }
}

fn merge_token(tokens: &mut Vec<CodeToken>, token: CodeToken) {
    if let Some(last) = tokens.last_mut()
        && last.kind == token.kind
        && last.range.end == token.range.start
    {
        last.range.end = token.range.end;
        return;
    }
    tokens.push(token);
}

/// Closes out the segment under construction, merging it forward when it
/// continues the same semantic run, otherwise emitting the finished one.
#[expect(
    clippy::suspicious_operation_groupings,
    reason = "a run continues when the kind matches and the ranges touch"
)]
fn extend(
    pending: &mut Option<(Range<usize>, CodeTokenKind)>,
    range: Range<usize>,
    kind: CodeTokenKind,
    produced: &mut Vec<CodeToken>,
) {
    let continues = matches!(
        pending,
        Some((current, current_kind)) if *current_kind == kind && current.end == range.start
    );
    if continues {
        if let Some((current, _)) = pending {
            current.end = range.end;
        }
        return;
    }
    if let Some((finished_range, finished_kind)) = pending.take() {
        produced.push(CodeToken {
            kind: finished_kind,
            range: finished_range,
        });
    }
    *pending = Some((range, kind));
}

/// Maps a `pulldown-cmark` heading level onto the owned depth value.
const fn heading_depth(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

/// Extracts the language token from a code-block tag, lowercased and taken
/// from the first whitespace-delimited word of the info string.
fn fence_language(kind: &CodeBlockKind<'_>) -> Option<String> {
    match kind {
        CodeBlockKind::Indented => None,
        CodeBlockKind::Fenced(info) => info
            .split(|character: char| character.is_whitespace() || character == ',')
            .find(|token| !token.is_empty())
            .map(str::to_ascii_lowercase),
    }
}

/// Decides fence closure by looking for a closing line of three or more
/// matching fence characters after the opening line. This inspects only the
/// exact region `pulldown-cmark` already delimited and implements no grammar.
fn fence_is_closed(region: &str) -> bool {
    let mut lines = region.lines();
    let Some(opening) = lines.next() else {
        return false;
    };
    let opening = opening.trim_start_matches(' ');
    let Some(opener) = opening
        .chars()
        .next()
        .filter(|character| matches!(character, '`' | '~'))
    else {
        return false;
    };
    let opener_length = opening
        .chars()
        .take_while(|character| *character == opener)
        .count();
    if opener_length < 3 {
        return false;
    }

    for line in lines {
        let trimmed = line.trim_start_matches(' ');
        if line.len() - trimmed.len() > 3 {
            continue;
        }
        let candidate = trimmed.trim_end();
        let closer_length = candidate
            .chars()
            .take_while(|character| *character == opener)
            .count();
        if closer_length >= opener_length && candidate.chars().all(|character| character == opener)
        {
            return true;
        }
    }
    false
}

/// Accumulates parser events into the owned block model.
///
/// Lists nest through explicit stacks so tight text, loose paragraphs,
/// nested lists, and fences inside items keep source order with no dropped
/// or duplicated text. Inline emphasis, strong, and links nest through a
/// small format stack over the same single `pulldown-cmark` event stream.
/// Table cells assemble in the same inline buffer as paragraph text, one
/// cell at a time.
///
/// `pulldown-cmark` always emits balanced `Start`/`End` pairs, including at
/// end of truncated input, so no end-of-input recovery layer exists: every
/// stack is empty when the event loop ends (asserted in debug builds).
#[derive(Default)]
struct DocumentBuilder {
    blocks: Vec<Block>,
    inline: InlineScope,
    /// Accumulation state for the code block being assembled, if any.
    code: Option<FenceBuilder>,
    /// Raw text of the HTML block being assembled, if any.
    html: Option<String>,
    /// Accumulation state for the table being assembled, if any.
    table: Option<TableBuilder>,
    /// Open lists, outermost first.
    lists: Vec<ActiveList>,
    /// Open items, outermost first; an item always belongs to the list at
    /// the same depth.
    items: Vec<ActiveItem>,
    /// Open inline emphasis, strong, link, and image-alt frames.
    formats: Vec<FormatFrame>,
}

#[derive(Default)]
struct FenceBuilder {
    language: Option<String>,
    source: String,
    start: usize,
}

struct ActiveList {
    ordered: bool,
    start: Option<u64>,
    items: Vec<ListItem>,
    range_start: usize,
}

#[derive(Default)]
struct InlineScope {
    spans: Vec<Span>,
    start: usize,
    heading_depth: Option<u8>,
    paragraph_open: bool,
}

struct ActiveItem {
    blocks: Vec<Block>,
    inline: InlineScope,
    task: Option<bool>,
    /// Value of `lists.len()` when this item opened: the item belongs to
    /// the list at that depth, so a closing list only settles items it
    /// directly owns instead of stealing a parent item.
    list_depth: usize,
}

enum FormatFrame {
    Emphasis(Vec<Span>),
    Strong(Vec<Span>),
    Link {
        label: Vec<Span>,
        destination: String,
    },
    ImageAlt(Vec<Span>),
}

impl FormatFrame {
    const fn spans_mut(&mut self) -> &mut Vec<Span> {
        match self {
            Self::Emphasis(spans) | Self::Strong(spans) | Self::ImageAlt(spans) => spans,
            Self::Link { label, .. } => label,
        }
    }

    fn into_span(self) -> Span {
        match self {
            Self::Emphasis(spans) => Span::Emphasis(spans),
            Self::Strong(spans) => Span::Strong(spans),
            Self::Link { label, destination } => Span::Link { label, destination },
            Self::ImageAlt(spans) => Span::Text(spans_text(&spans)),
        }
    }
}

/// Parser options for the shared engine: `CommonMark` core plus task-list
/// markers (`- [ ]` / `- [x]`), which `pulldown-cmark` only scans when
/// [`pulldown_cmark::Options::ENABLE_TASKLISTS`] is set, and GFM tables. No other GFM
/// extension is enabled.
pub(crate) use artisan_markdown_index::PARSE_OPTIONS;

impl DocumentBuilder {
    /// Walks every offset-tagged event of `source` into blocks.
    fn build(self, source: &str) -> Result<MarkdownDocument, MarkdownError> {
        self.build_events(
            source,
            Parser::new_ext(source, PARSE_OPTIONS).into_offset_iter(),
        )
    }

    /// Indexed leaves replay the exact original parser events against only
    /// their admitted source range. No fragment is parsed as a new document.
    #[expect(
        clippy::wildcard_enum_match_arm,
        reason = "only nesting depth is counted here; other pulldown-cmark events pass"
    )]
    fn build_events<'a>(
        mut self,
        source: &str,
        events: impl IntoIterator<Item = (Event<'a>, Range<usize>)>,
    ) -> Result<MarkdownDocument, MarkdownError> {
        let mut depth = 0usize;
        for (count, (event, range)) in events.into_iter().enumerate() {
            if count >= 32_768 {
                return Err(MarkdownError::PreviewLimit("32768 parser events"));
            }
            match &event {
                Event::Start(_) => {
                    depth += 1;
                    if depth > 96 {
                        return Err(MarkdownError::PreviewLimit("96 nested tags"));
                    }
                }
                Event::End(_) => depth = depth.saturating_sub(1),
                _ => {}
            }
            match event {
                Event::Start(tag) => self.start_tag(tag, range.start),
                Event::End(tag_end) => self.end_tag(tag_end, range, source),
                Event::Text(_) if self.code.is_some() || self.html.is_some() => {
                    let raw = &source[range];
                    if let Some(fence) = self.code.as_mut() {
                        fence.source.push_str(raw);
                    } else if let Some(html) = self.html.as_mut() {
                        html.push_str(raw);
                    }
                }
                Event::Text(text) => self.push_inline(Span::Text(text.as_ref().to_owned())),
                Event::Code(code) => self.push_inline(Span::Code(code.as_ref().to_owned())),
                Event::InlineHtml(html) => {
                    self.push_inline(Span::Html(html.as_ref().to_owned()));
                }
                Event::Html(html) => {
                    // A continuation chunk of an open `HtmlBlock`, or stray
                    // block-level HTML outside any tags: either way the text
                    // is captured verbatim as inert data.
                    match self.html.as_mut() {
                        Some(open) => open.push_str(html.as_ref()),
                        None if self.code.is_none() => {
                            self.push_block(Block::Html {
                                source: source[range].to_owned(),
                            });
                        }
                        None => {}
                    }
                }
                Event::SoftBreak | Event::HardBreak => {
                    self.push_inline(Span::Text("\n".to_owned()));
                }
                Event::TaskListMarker(checked) => {
                    if let Some(item) = self.items.last_mut() {
                        item.task = Some(checked);
                    }
                }
                Event::Rule
                | Event::InlineMath(_)
                | Event::DisplayMath(_)
                | Event::FootnoteReference(_) => {}
            }
        }
        self.finish_invariant();
        Ok(MarkdownDocument {
            blocks: std::mem::take(&mut self.blocks),
        })
    }

    /// Documents the parser's balanced-events contract: every opened
    /// paragraph, heading, fence, list, item, and inline frame is closed by
    /// its own `End` event, including at end of truncated input, so all
    /// builder stacks are empty here.
    fn finish_invariant(&self) {
        debug_assert!(
            self.items.is_empty(),
            "balanced item events leave no open item"
        );
        debug_assert!(
            self.lists.is_empty(),
            "balanced list events leave no open list"
        );
        debug_assert!(
            self.formats.is_empty(),
            "balanced inline events leave no open frame"
        );
        debug_assert!(
            self.code.is_none(),
            "balanced fence events settle every fence"
        );
        debug_assert!(
            self.html.is_none(),
            "balanced HTML events settle every block"
        );
        debug_assert!(
            self.table.is_none(),
            "balanced table events settle every table"
        );
    }

    fn start_tag(&mut self, tag: Tag<'_>, start: usize) {
        match tag {
            Tag::Paragraph => {
                // Stray text from flattened constructs must never leak into
                // the next real block.
                self.clear_inline(start);
                self.set_paragraph_open(true);
                self.set_heading_depth(None);
            }
            Tag::Heading { level, .. } => {
                self.clear_inline(start);
                self.set_paragraph_open(false);
                self.set_heading_depth(Some(heading_depth(level)));
            }
            Tag::CodeBlock(kind) => {
                self.flush_tight_before_block();
                self.code = Some(FenceBuilder {
                    language: fence_language(&kind),
                    source: String::new(),
                    start,
                });
            }
            Tag::HtmlBlock => {
                self.flush_tight_before_block();
                self.html = Some(String::new());
            }
            Tag::List(start_number) => {
                self.flush_tight_before_block();
                self.lists.push(ActiveList {
                    ordered: start_number.is_some(),
                    start: start_number,
                    items: Vec::new(),
                    range_start: start,
                });
            }
            Tag::Item => {
                self.items.push(ActiveItem {
                    blocks: Vec::new(),
                    inline: InlineScope {
                        start,
                        ..Default::default()
                    },
                    task: None,
                    list_depth: self.lists.len(),
                });
            }
            Tag::Emphasis => self.formats.push(FormatFrame::Emphasis(Vec::new())),
            Tag::Strong => self.formats.push(FormatFrame::Strong(Vec::new())),
            Tag::Link { dest_url, .. } => self.formats.push(FormatFrame::Link {
                label: Vec::new(),
                destination: dest_url.to_string(),
            }),
            Tag::Image { .. } => self.formats.push(FormatFrame::ImageAlt(Vec::new())),
            Tag::Table(alignments) => {
                self.flush_tight_before_block();
                self.table = Some(TableBuilder::new(&alignments, start));
            }
            Tag::TableCell => self.clear_inline(start),
            // Table rows carry no state of their own: their `TagEnd`
            // settles the cells collected since the previous row.
            Tag::TableHead
            | Tag::TableRow
            | Tag::BlockQuote(_)
            | Tag::FootnoteDefinition(_)
            | Tag::DefinitionList
            | Tag::DefinitionListTitle
            | Tag::DefinitionListDefinition
            | Tag::MetadataBlock(_)
            | Tag::Strikethrough
            | Tag::Superscript
            | Tag::Subscript => {}
        }
    }

    #[expect(
        clippy::wildcard_enum_match_arm,
        reason = "pulldown-cmark ends that close nothing here are skipped, future ones too"
    )]
    fn end_tag(&mut self, tag_end: TagEnd, range: Range<usize>, source: &str) {
        match tag_end {
            TagEnd::Paragraph => {
                let spans = self.take_inline();
                let start = self.take_inline_start(range.start);
                self.set_paragraph_open(false);
                // A heading level lingering here would belong to a different
                // open construct; paragraphs keep their own shape.
                self.take_heading_depth();
                self.push_block(Block::Paragraph {
                    spans,
                    range: start..range.end,
                });
            }
            TagEnd::Heading(_) => {
                let spans = self.take_inline();
                let start = self.take_inline_start(range.start);
                self.set_paragraph_open(false);
                let level = self.take_heading_depth().unwrap_or(1);
                self.push_block(Block::Heading {
                    level,
                    spans,
                    range: start..range.end,
                });
            }
            TagEnd::CodeBlock => {
                if let Some(fence) = self.code.take() {
                    let closed = source
                        .get(fence.start..range.end)
                        .is_some_and(fence_is_closed);
                    self.push_block(Block::Code(CodeFence {
                        language: fence.language,
                        closed,
                        source: fence.source,
                        tokens: None,
                        diagram: None,
                    }));
                }
            }
            TagEnd::HtmlBlock => {
                if let Some(source_text) = self.html.take() {
                    self.push_block(Block::Html {
                        source: source_text,
                    });
                }
            }
            TagEnd::List(_) => {
                // `TagEnd::Item` already settled every item this list owns,
                // so a closing list must never pop again unconditionally:
                // that stole the still-open parent item into the inner list
                // and stranded the nested list at the wrong container. Only
                // an item opened at this list's own depth may settle here,
                // which balanced input never leaves behind.
                if self
                    .items
                    .last()
                    .is_some_and(|item| item.list_depth == self.lists.len())
                {
                    self.close_open_item(range.end);
                }
                if let Some(list) = self.lists.pop() {
                    self.push_block(Block::List {
                        ordered: list.ordered,
                        start: list.start,
                        items: list.items,
                        range: list.range_start..range.end,
                    });
                }
            }
            TagEnd::Item => {
                self.close_open_item(range.end);
            }
            TagEnd::Emphasis | TagEnd::Strong | TagEnd::Link | TagEnd::Image => {
                if let Some(frame) = self.formats.pop() {
                    let span = frame.into_span();
                    self.push_inline(span);
                }
            }
            TagEnd::TableCell => {
                let spans = self.take_inline();
                if let Some(table) = self.table.as_mut() {
                    table.push_cell(spans);
                }
            }
            TagEnd::TableHead => {
                if let Some(table) = self.table.as_mut() {
                    table.finish_header();
                }
            }
            TagEnd::TableRow => {
                if let Some(table) = self.table.as_mut() {
                    table.finish_row();
                }
            }
            TagEnd::Table => {
                if let Some(table) = self.table.take() {
                    self.push_block(Block::Table(table.finish(range.end)));
                }
            }
            _ => {}
        }
    }

    /// Moves tight item text into a paragraph block before a nested
    /// block-level construct (nested list, fence, HTML) starts, so the
    /// outer text and the nested block become siblings in source order.
    fn flush_tight_before_block(&mut self) {
        if self.items.is_empty() || !self.formats.is_empty() {
            return;
        }
        let needs_flush = self
            .items
            .last()
            .is_some_and(|item| !item.inline.paragraph_open && !item.inline.spans.is_empty());
        if !needs_flush {
            return;
        }
        if let Some(item) = self.items.last_mut() {
            let spans = std::mem::take(&mut item.inline.spans);
            let start = item.inline.start;
            // End is unknown until the item closes; reuse the start so the
            // range stays honest rather than invented.
            item.blocks.push(Block::Paragraph {
                spans,
                range: start..start,
            });
        }
    }

    /// Closes the innermost open item: tight remainder becomes a paragraph,
    /// then the item joins its list. A missing list (malformed nesting)
    /// keeps the item's blocks at the current container instead of dropping.
    fn close_open_item(&mut self, end: usize) {
        let Some(mut item) = self.items.pop() else {
            return;
        };
        if !item.inline.spans.is_empty()
            || item.inline.paragraph_open
            || item.inline.heading_depth.is_some()
        {
            let spans = std::mem::take(&mut item.inline.spans);
            if let Some(level) = item.inline.heading_depth.take() {
                item.blocks.push(Block::Heading {
                    level,
                    spans,
                    range: item.inline.start..end,
                });
            } else {
                item.blocks.push(Block::Paragraph {
                    spans,
                    range: item.inline.start..end,
                });
            }
            item.inline.paragraph_open = false;
        }
        let list_item = ListItem {
            blocks: item.blocks,
            task: item.task,
        };
        if let Some(list) = self.lists.last_mut() {
            list.items.push(list_item);
        } else {
            // No open list owns this item: keep every confirmed block at
            // the current container rather than dropping the tail.
            for block in list_item.blocks {
                self.push_block(block);
            }
        }
    }

    fn push_block(&mut self, block: Block) {
        if let Some(item) = self.items.last_mut() {
            item.blocks.push(block);
        } else {
            self.blocks.push(block);
        }
    }

    fn push_inline(&mut self, span: Span) {
        if let Some(frame) = self.formats.last_mut() {
            push_span_merge(frame.spans_mut(), span);
            return;
        }
        push_span_merge(&mut self.inline_scope().spans, span);
    }

    fn inline_scope(&mut self) -> &mut InlineScope {
        self.items
            .last_mut()
            .map_or(&mut self.inline, |item| &mut item.inline)
    }

    fn clear_inline(&mut self, start: usize) {
        let scope = self.inline_scope();
        scope.spans.clear();
        scope.start = start;
    }

    fn take_inline(&mut self) -> Vec<Span> {
        std::mem::take(&mut self.inline_scope().spans)
    }

    fn take_inline_start(&mut self, fallback: usize) -> usize {
        std::mem::replace(&mut self.inline_scope().start, fallback)
    }

    fn set_paragraph_open(&mut self, open: bool) {
        self.inline_scope().paragraph_open = open;
    }

    fn set_heading_depth(&mut self, depth: Option<u8>) {
        self.inline_scope().heading_depth = depth;
    }

    fn take_heading_depth(&mut self) -> Option<u8> {
        self.inline_scope().heading_depth.take()
    }
}

fn push_span_merge(target: &mut Vec<Span>, span: Span) {
    match (target.last_mut(), &span) {
        (Some(Span::Text(existing)), Span::Text(text)) => existing.push_str(text),
        _ => target.push(span),
    }
}

#[cfg(test)]
mod preview_budget_tests {
    use super::*;
    #[test]
    fn deeply_nested_inline_source_returns_a_preview_error() {
        let source = format!("{}text{}", "*".repeat(200), "*".repeat(200));
        let engine = MarkdownEngine::new().unwrap();
        assert!(matches!(
            engine.parse_document(&source),
            Err(MarkdownError::PreviewLimit(_))
        ));
    }
    #[test]
    fn many_short_paragraphs_are_bounded_before_tree_preparation() {
        let engine = MarkdownEngine::new().unwrap();
        assert!(matches!(
            engine.parse_document(&"a\n\n".repeat(12_000)),
            Err(MarkdownError::PreviewLimit(_))
        ));
    }
}
