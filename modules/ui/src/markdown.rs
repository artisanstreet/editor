//! Narrow streaming-Markdown engine seam.
//!
//! Third-party crates own grammar correctness: [`pulldown_cmark`] parses
//! `CommonMark` into offset-tagged events, and [`syntect`] classifies fenced
//! source through its bundled syntax grammars. This module converts both into
//! small owned values ([`MarkdownDocument`], [`CodeToken`]) that a later
//! native renderer can lay out without re-parsing or touching either crate
//! directly.
//!
//! Deliberate Phase 1 limits, as promoted for list parity:
//!
//! - Raw HTML is recognized only so it can be carried as inert source text.
//!   It is never interpreted, rewritten, sanitized into markup, or rendered.
//! - Only `CommonMark` core constructs are modeled, plus task-list markers:
//!   GFM extensions such as tables and strikethrough remain disabled until
//!   the renderer phase selects them deliberately. Task markers need
//!   [`Options::ENABLE_TASKLISTS`], which only affects list-item marker
//!   scanning, because `pulldown-cmark` never emits `TaskListMarker`
//!   otherwise.
//! - Ordered, unordered, nested, tight, loose, and task lists are preserved
//!   through the shared `pulldown-cmark` event stream; no parallel parser is
//!   introduced. Tight item text becomes paragraph blocks so loose and tight
//!   lists share one renderer path with no dropped or duplicated text.
//! - Inline emphasis, strong, and link labels plus verbatim destinations are
//!   preserved. Images continue to flatten into their alt text; the renderer
//!   decides link safety (mirroring the Svelte anchor guard) and never
//!   executes a destination.
//! - Highlight output is scope-classified [`CodeToken`] data over byte
//!   ranges, never HTML and never theme-resolved colors. Mapping kinds onto
//!   Artisan theme tokens remains first-party renderer work.

// The `Markdown*` prefix is deliberate: these types are the crate's public
// Markdown vocabulary and read better fully qualified at call sites.
#![allow(clippy::module_name_repetitions)]

use std::ops::Range;
use std::str::FromStr;

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use syntect::highlighting::ScopeSelectors;
use syntect::parsing::{ParseState, ScopeStack, SyntaxSet};
use thiserror::Error;

/// Failure crossing the Markdown engine boundary.
///
/// Parsing itself is total: malformed input degrades to plain text instead of
/// failing. These variants only cover syntax-definition machinery failing
/// below the seam.
#[derive(Debug, Error)]
pub enum MarkdownError {
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
    pub tokens: Option<Vec<CodeToken>>,
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
    /// Emphasized runs (`*x*`, `_x_`); structure is preserved, rendering
    /// stays plain until the native text primitive exposes italics.
    Emphasis(Vec<Span>),
    /// Strong runs (`**x**`); rendered bold through the native highlight.
    Strong(Vec<Span>),
    /// Link label plus the verbatim authored destination. The engine keeps
    /// every destination; the renderer decides safety and never executes one.
    Link {
        /// Visible label runs.
        label: Vec<Span>,
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
            Self::Text(text) | Self::Code(text) | Self::Html(text) => text.clone(),
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
                        .map(Block::text_content)
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>()
                .join("\n"),
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
    blocks: Vec<Block>,
}

impl MarkdownDocument {
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
        let mut document = DocumentBuilder::default().build(source);

        for block in &mut document.blocks {
            if let Block::Code(fence) = block
                && fence.closed
            {
                self.highlight_fence(fence)?;
            }
        }

        Ok(document)
    }

    /// Fills `fence.tokens` when the language resolves to a bundled grammar;
    /// leaves `None` for unknown languages and bare or indented fences.
    fn highlight_fence(&self, fence: &mut CodeFence) -> Result<(), MarkdownError> {
        let Some(language) = fence.language.as_deref() else {
            return Ok(());
        };
        let Some(syntax) = self.syntax_set.find_syntax_by_token(language) else {
            return Ok(());
        };

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

        fence.tokens = Some(merged);
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
    #[allow(clippy::too_many_arguments)]
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

/// Yields each physical line together with its starting byte offset.
struct LineOffsets<'a> {
    source: &'a str,
    cursor: usize,
}

impl<'a> LineOffsets<'a> {
    fn new(source: &'a str) -> Self {
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
fn heading_depth(level: HeadingLevel) -> u8 {
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
///
/// `pulldown-cmark` always emits balanced `Start`/`End` pairs, including at
/// end of truncated input, so no end-of-input recovery layer exists: every
/// stack is empty when the event loop ends (asserted in debug builds).
#[derive(Default)]
struct DocumentBuilder {
    blocks: Vec<Block>,
    /// Spans of the paragraph, heading, or tight item text being assembled
    /// at document level (used only when no list item is open).
    inline: Vec<Span>,
    /// Byte offset where the current document-level inline run began.
    inline_start: usize,
    /// Depth of the heading being assembled, if any.
    heading_depth: Option<u8>,
    /// True while a paragraph is open at document level or inside an item.
    paragraph_open: bool,
    /// Accumulation state for the code block being assembled, if any.
    code: Option<FenceBuilder>,
    /// Raw text of the HTML block being assembled, if any.
    html: Option<String>,
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

struct ActiveItem {
    blocks: Vec<Block>,
    inline: Vec<Span>,
    inline_start: usize,
    heading_depth: Option<u8>,
    paragraph_open: bool,
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
    fn spans_mut(&mut self) -> &mut Vec<Span> {
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
/// [`Options::ENABLE_TASKLISTS`] is set. No other GFM extension is enabled.
const PARSE_OPTIONS: Options = Options::ENABLE_TASKLISTS;

impl DocumentBuilder {
    /// Walks every offset-tagged event of `source` into blocks.
    fn build(mut self, source: &str) -> MarkdownDocument {
        for (event, range) in Parser::new_ext(source, PARSE_OPTIONS).into_offset_iter() {
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
        MarkdownDocument {
            blocks: std::mem::take(&mut self.blocks),
        }
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
                    inline: Vec::new(),
                    inline_start: start,
                    heading_depth: None,
                    paragraph_open: false,
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
            Tag::BlockQuote(_)
            | Tag::FootnoteDefinition(_)
            | Tag::DefinitionList
            | Tag::DefinitionListTitle
            | Tag::DefinitionListDefinition
            | Tag::Table(_)
            | Tag::TableHead
            | Tag::TableRow
            | Tag::TableCell
            | Tag::MetadataBlock(_)
            | Tag::Strikethrough
            | Tag::Superscript
            | Tag::Subscript => {}
        }
    }

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
            .is_some_and(|item| !item.paragraph_open && !item.inline.is_empty());
        if !needs_flush {
            return;
        }
        if let Some(item) = self.items.last_mut() {
            let spans = std::mem::take(&mut item.inline);
            let start = item.inline_start;
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
        if !item.inline.is_empty() || item.paragraph_open || item.heading_depth.is_some() {
            let spans = std::mem::take(&mut item.inline);
            if let Some(level) = item.heading_depth.take() {
                item.blocks.push(Block::Heading {
                    level,
                    spans,
                    range: item.inline_start..end,
                });
            } else {
                item.blocks.push(Block::Paragraph {
                    spans,
                    range: item.inline_start..end,
                });
            }
            item.paragraph_open = false;
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
        if let Some(item) = self.items.last_mut() {
            // A heading inside an item assembles in the item's buffer.
            push_span_merge(&mut item.inline, span);
        } else {
            push_span_merge(&mut self.inline, span);
        }
    }

    fn clear_inline(&mut self, start: usize) {
        if let Some(item) = self.items.last_mut() {
            item.inline.clear();
            item.inline_start = start;
        } else {
            self.inline.clear();
            self.inline_start = start;
        }
    }

    fn take_inline(&mut self) -> Vec<Span> {
        if let Some(item) = self.items.last_mut() {
            std::mem::take(&mut item.inline)
        } else {
            std::mem::take(&mut self.inline)
        }
    }

    fn take_inline_start(&mut self, fallback: usize) -> usize {
        if let Some(item) = self.items.last_mut() {
            std::mem::replace(&mut item.inline_start, fallback)
        } else {
            std::mem::replace(&mut self.inline_start, fallback)
        }
    }

    fn set_paragraph_open(&mut self, open: bool) {
        if let Some(item) = self.items.last_mut() {
            item.paragraph_open = open;
        } else {
            self.paragraph_open = open;
        }
    }

    fn set_heading_depth(&mut self, depth: Option<u8>) {
        if let Some(item) = self.items.last_mut() {
            item.heading_depth = depth;
        } else {
            self.heading_depth = depth;
        }
    }

    fn take_heading_depth(&mut self) -> Option<u8> {
        if let Some(item) = self.items.last_mut() {
            item.heading_depth.take()
        } else {
            self.heading_depth.take()
        }
    }
}

fn push_span_merge(target: &mut Vec<Span>, span: Span) {
    match (target.last_mut(), &span) {
        (Some(Span::Text(existing)), Span::Text(text)) => existing.push_str(text),
        _ => target.push(span),
    }
}
