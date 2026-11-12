#![expect(
    clippy::module_name_repetitions,
    reason = "public names stay self-describing at use sites"
)]

pub(crate) const MAX_MARKDOWN_SOURCE_BYTES: usize = 256 * 1024;

use std::ops::Range;
use std::str::FromStr;
use std::sync::Arc;

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Parser, Tag, TagEnd};
use syntect::highlighting::ScopeSelectors;
use syntect::parsing::{ParseState, ScopeStack, SyntaxSet};
use thiserror::Error;

mod diagram;
mod fence;
mod list;
pub mod literal;
mod projected;
pub mod reveal;
mod source_index;
pub mod stored;
mod table;
mod table_view;
pub use projected::{PROSE_MEASURE_PX, leaf_signatures, prose_measure};
pub use source_index::{
    BlockShape, IndexedLeaf, IndexedLeafContent, LeafContext, LeafSpacing, ListLevel,
    MarkdownIndex, MarkdownIndexError, normalize_record,
};

use table::TableBuilder;

pub const LIST_INDENT: f32 = crate::ui::theme::ProseTypography::LIST_INDENT_PX;

pub type GlyphPainter = std::rc::Rc<
    dyn Fn(editor_assets::AssetId, f32, aqua_core::GpuiHsla) -> aqua_ui::element::Element,
>;

#[must_use]
pub fn glyph_painter() -> GlyphPainter {
    std::rc::Rc::new(crate::ui::asset::source_asset_glyph)
}
pub use table::{Table, TableAlignment, TableCell};

#[derive(Debug, Error)]
pub enum MarkdownError {
    #[error("Markdown preview exceeds {0}; open the full source")]
    PreviewLimit(&'static str),
    #[error("syntax highlighting failed while parsing a line")]
    LineParse(#[from] syntect::parsing::ParsingError),
    #[error("syntax highlighting failed while applying a scope operation")]
    ScopeApply(#[from] syntect::parsing::ScopeError),
    #[error("built-in scope selector `{selector}` failed to parse")]
    Selector {
        selector: &'static str,
        #[source]
        cause: syntect::parsing::ParseScopeError,
    },
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum CodeTokenKind {
    Comment,
    Str,
    Number,
    Keyword,
    Type,
    Function,
    Plain,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct CodeToken {
    pub kind: CodeTokenKind,
    pub range: Range<usize>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct DiagramSvg {
    pub source: Arc<str>,
    pub light_source: Arc<str>,
    pub width_bits: u32,
    pub height_bits: u32,
}

impl DiagramSvg {
    pub fn width(&self) -> f32 {
        f32::from_bits(self.width_bits)
    }

    pub fn height(&self) -> f32 {
        f32::from_bits(self.height_bits)
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct CodeFence {
    pub language: Option<String>,
    pub closed: bool,
    pub source: String,
    pub tokens: Option<Arc<[CodeToken]>>,
    pub diagram: Option<Arc<DiagramSvg>>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum Span {
    Text(String),
    Code(String),
    Html(String),
    Command(String),
    Emphasis(Vec<Self>),
    Strong(Vec<Self>),
    Link {
        label: Vec<Self>,
        destination: String,
    },
}

impl Span {
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

#[must_use]
pub fn spans_text(spans: &[Span]) -> String {
    spans.iter().map(Span::text_content).collect()
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum Block {
    Heading {
        level: u8,
        spans: Vec<Span>,
        range: Range<usize>,
    },
    Paragraph {
        spans: Vec<Span>,
        range: Range<usize>,
    },
    Code(CodeFence),
    Html {
        source: String,
    },
    List {
        ordered: bool,
        start: Option<u64>,
        items: Vec<ListItem>,
        range: Range<usize>,
    },
    Table(Table),
}

impl Block {
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

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ListItem {
    pub blocks: Vec<Block>,
    pub task: Option<bool>,
}

impl ListItem {
    #[must_use]
    pub fn text_content(&self) -> String {
        self.blocks
            .iter()
            .map(Block::text_content)
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct MarkdownDocument {
    pub(crate) blocks: Vec<Block>,
}

pub(crate) fn mark_command_spans(spans: &mut Vec<Span>, command: &str) {
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

impl MarkdownDocument {
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
                            fence
                                .diagram
                                .as_ref()
                                .map_or(0, |svg| svg.source.len() + svg.light_source.len())
                                + fence.source.capacity()
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

    #[must_use]
    pub fn blocks(&self) -> &[Block] {
        &self.blocks
    }
}

static SYNTAX_SET: std::sync::LazyLock<SyntaxSet> =
    std::sync::LazyLock::new(SyntaxSet::load_defaults_newlines);

#[derive(Debug)]
pub struct MarkdownEngine {
    syntax_set: &'static SyntaxSet,
    classifiers: Vec<(ScopeSelectors, CodeTokenKind)>,
    diagrams: std::sync::Mutex<diagram::DiagramCache>,
}

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
            syntax_set: &SYNTAX_SET,
            classifiers,
            diagrams: std::sync::Mutex::default(),
        })
    }

    pub fn parse_document(&self, source: &str) -> Result<MarkdownDocument, MarkdownError> {
        self.parse_document_with_fence_memo(source, &mut NoFenceTokenMemo)
    }

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

    fn highlight_blocks(
        &self,
        blocks: &mut [Block],
        memo: &mut dyn FenceTokenMemo,
    ) -> Result<(), MarkdownError> {
        for block in blocks {
            match block {
                Block::Code(fence) if fence.closed => {
                    if fence.language.as_deref() == Some("mermaid") {
                        fence.diagram = self
                            .diagrams
                            .lock()
                            .map_or_else(|poisoned| poisoned.into_inner(), |cache| cache)
                            .get(&fence.source);
                    }
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

    fn highlight_line(
        &self,
        parse_state: &mut ParseState,
        scope_stack: &mut ScopeStack,
        line: &str,
        base: usize,
        pending: &mut Option<(Range<usize>, CodeTokenKind)>,
    ) -> Result<Vec<CodeToken>, MarkdownError> {
        let operations = parse_state.parse_line(line, self.syntax_set)?;
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

pub(crate) trait FenceTokenMemo {
    fn cached(&mut self, language: &str, source: &str) -> Option<Arc<[CodeToken]>>;

    fn remember(&mut self, language: &str, source: &str, tokens: Arc<[CodeToken]>);
}

struct NoFenceTokenMemo;

impl FenceTokenMemo for NoFenceTokenMemo {
    fn cached(&mut self, _language: &str, _source: &str) -> Option<Arc<[CodeToken]>> {
        None
    }

    fn remember(&mut self, _language: &str, _source: &str, _tokens: Arc<[CodeToken]>) {}
}

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

fn fence_language(kind: &CodeBlockKind<'_>) -> Option<String> {
    match kind {
        CodeBlockKind::Indented => None,
        CodeBlockKind::Fenced(info) => info
            .split(|character: char| character.is_whitespace() || character == ',')
            .find(|token| !token.is_empty())
            .map(str::to_ascii_lowercase),
    }
}

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

#[derive(Default)]
struct DocumentBuilder {
    blocks: Vec<Block>,
    inline: InlineScope,
    code: Option<FenceBuilder>,
    html: Option<String>,
    table: Option<TableBuilder>,
    lists: Vec<ActiveList>,
    items: Vec<ActiveItem>,
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

pub(crate) use artisan_markdown_index::PARSE_OPTIONS;

impl DocumentBuilder {
    fn build(self, source: &str) -> Result<MarkdownDocument, MarkdownError> {
        self.build_events(
            source,
            Parser::new_ext(source, PARSE_OPTIONS).into_offset_iter(),
        )
    }

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
                Event::Html(html) => match self.html.as_mut() {
                    Some(open) => open.push_str(html.as_ref()),
                    None if self.code.is_none() => {
                        self.push_block(Block::Html {
                            source: source[range].to_owned(),
                        });
                    }
                    None => {}
                },
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
            item.blocks.push(Block::Paragraph {
                spans,
                range: start..start,
            });
        }
    }

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

pub mod terminal;

pub(crate) mod inline;
pub mod presenter;
pub mod visual;
