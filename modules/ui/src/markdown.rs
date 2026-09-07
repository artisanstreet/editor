//! Narrow streaming-Markdown engine seam.
//!
//! Third-party crates own grammar correctness: [`pulldown_cmark`] parses
//! `CommonMark` into offset-tagged events, and [`syntect`] classifies fenced
//! source through its bundled syntax grammars. This module converts both into
//! small owned values ([`MarkdownDocument`], [`CodeToken`]) that a later
//! native renderer can lay out without re-parsing or touching either crate
//! directly.
//!
//! Deliberate Phase 1 limits:
//!
//! - Raw HTML is recognized only so it can be carried as inert source text.
//!   It is never interpreted, rewritten, sanitized into markup, or rendered.
//! - Only `CommonMark` core constructs are modeled. GFM extensions such as
//!   tables and strikethrough remain disabled until the renderer phase
//!   selects them deliberately.
//! - Container constructs are represented only when `pulldown-cmark` exposes
//!   inner paragraph or heading events. Tight-list text and other unmodeled
//!   container-only events are deliberately omitted instead of guessed.
//!   Links, images, and emphasis flatten into their inner text.
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
    /// Ordinary text, including flattened link, image, and emphasis runs.
    Text(String),
    /// Inline code payload taken verbatim between backticks.
    Code(String),
    /// Raw inline HTML, carried verbatim as inert data.
    Html(String),
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
#[derive(Default)]
struct DocumentBuilder {
    blocks: Vec<Block>,
    /// Spans of the paragraph or heading being assembled.
    inline: Vec<Span>,
    /// Byte offset where the current paragraph or heading began.
    inline_start: usize,
    /// Depth of the heading being assembled, if any.
    heading_depth: Option<u8>,
    /// Accumulation state for the code block being assembled, if any.
    code: Option<FenceBuilder>,
    /// Raw text of the HTML block being assembled, if any.
    html: Option<String>,
}

#[derive(Default)]
struct FenceBuilder {
    language: Option<String>,
    source: String,
    start: usize,
}

impl DocumentBuilder {
    /// Walks every offset-tagged event of `source` into blocks.
    fn build(mut self, source: &str) -> MarkdownDocument {
        for (event, range) in Parser::new_ext(source, Options::empty()).into_offset_iter() {
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
                            self.blocks.push(Block::Html {
                                source: source[range].to_owned(),
                            });
                        }
                        None => {}
                    }
                }
                Event::SoftBreak | Event::HardBreak => {
                    self.push_inline(Span::Text("\n".to_owned()));
                }
                Event::Rule
                | Event::InlineMath(_)
                | Event::DisplayMath(_)
                | Event::FootnoteReference(_)
                | Event::TaskListMarker(_) => {}
            }
        }
        MarkdownDocument {
            blocks: std::mem::take(&mut self.blocks),
        }
    }

    fn start_tag(&mut self, tag: Tag<'_>, start: usize) {
        match tag {
            Tag::Paragraph => {
                // Stray text from flattened constructs must never leak into
                // the next real block.
                self.inline.clear();
                self.inline_start = start;
                self.heading_depth = None;
            }
            Tag::Heading { level, .. } => {
                self.inline.clear();
                self.inline_start = start;
                self.heading_depth = Some(heading_depth(level));
            }
            Tag::CodeBlock(kind) => {
                self.code = Some(FenceBuilder {
                    language: fence_language(&kind),
                    source: String::new(),
                    start,
                });
            }
            Tag::HtmlBlock => self.html = Some(String::new()),
            Tag::Item
            | Tag::List(_)
            | Tag::BlockQuote(_)
            | Tag::FootnoteDefinition(_)
            | Tag::DefinitionList
            | Tag::DefinitionListTitle
            | Tag::DefinitionListDefinition
            | Tag::Table(_)
            | Tag::TableHead
            | Tag::TableRow
            | Tag::TableCell
            | Tag::MetadataBlock(_)
            | Tag::Emphasis
            | Tag::Strong
            | Tag::Strikethrough
            | Tag::Superscript
            | Tag::Subscript
            | Tag::Link { .. }
            | Tag::Image { .. } => {}
        }
    }

    fn end_tag(&mut self, tag_end: TagEnd, range: Range<usize>, source: &str) {
        match tag_end {
            TagEnd::Paragraph | TagEnd::Heading(_) => {
                let spans = std::mem::take(&mut self.inline);
                if let Some(level) = self.heading_depth.take() {
                    self.blocks.push(Block::Heading {
                        level,
                        spans,
                        range: self.inline_start..range.end,
                    });
                } else {
                    self.blocks.push(Block::Paragraph {
                        spans,
                        range: self.inline_start..range.end,
                    });
                }
            }
            TagEnd::CodeBlock => {
                if let Some(fence) = self.code.take() {
                    let closed = source
                        .get(fence.start..range.end)
                        .is_some_and(fence_is_closed);
                    self.blocks.push(Block::Code(CodeFence {
                        language: fence.language,
                        closed,
                        source: fence.source,
                        tokens: None,
                    }));
                }
            }
            TagEnd::HtmlBlock => {
                if let Some(source_text) = self.html.take() {
                    self.blocks.push(Block::Html {
                        source: source_text,
                    });
                }
            }
            _ => {}
        }
    }

    fn push_inline(&mut self, span: Span) {
        match (&mut self.inline.last_mut(), &span) {
            (Some(Span::Text(existing)), Span::Text(text)) => existing.push_str(text),
            _ => self.inline.push(span),
        }
    }
}
