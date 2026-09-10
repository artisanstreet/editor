//! Synchronous native rendering for an accepted Markdown message body.
//!
//! [`MarkdownRenderer`] is intentionally a small presentation seam over the
//! owned vocabulary in [`crate::markdown`]. It parses during the render pass
//! and immediately turns the resulting blocks into ordinary GPUI elements;
//! it does not retain message or document state.
//!
//! Lists render as native stacked rows with muted markers (`•` or `1.`)
//! instead of HTML list elements; emphasis and strong survive as inline
//! structure, and absolute `http(s)`/`mailto:` link labels render underlined
//! in the accent token and open through the platform browser on click.
//! Relative and other-scheme destinations keep their plain label: with no
//! project base the renderer must not open arbitrary local paths, and raw
//! HTML stays inert text.

#![allow(clippy::module_name_repetitions)]

use std::ops::Range;

use gpui::{
    AnyElement, Div, FontStyle, FontWeight, HighlightStyle, InteractiveText, IntoElement,
    ParentElement, SharedString, Styled, StyledText, UnderlineStyle, div,
    prelude::InteractiveElement as _, px,
};

use crate::markdown::{Block, CodeFence, CodeToken, CodeTokenKind, ListItem, MarkdownEngine, Span};
use crate::theme::ArtisanTheme;

/// Synchronous renderer for accepted Markdown message bodies.
///
/// The engine is the only retained state. Source text, parsed documents, and
/// syntax ranges are all owned only for the duration of one render call.
#[derive(Debug)]
pub struct MarkdownRenderer {
    engine: MarkdownEngine,
}

impl MarkdownRenderer {
    /// Constructs a renderer with the shared bundled Markdown engine.
    ///
    /// The engine's only construction failure is a rejected fixed classifier
    /// selector, which is a first-party programming error rather than input
    /// supplied by a conversation.
    ///
    /// # Panics
    ///
    /// Panics only if the fixed first-party classifier selector table is
    /// invalid.
    #[must_use]
    pub fn new() -> Self {
        Self {
            engine: MarkdownEngine::new()
                .expect("the built-in Markdown classifier selectors must remain valid"),
        }
    }

    /// Parses and renders one source body as inert native GPUI elements.
    ///
    /// The supplied selector is the stable parent identity of the owning
    /// message card. It is never combined with source text or parser output.
    /// Parse failures and representations that cannot preserve the original
    /// source (open or unknown fences) use the plain body fallback.
    #[must_use]
    pub fn render_source(
        &self,
        source: &str,
        theme: ArtisanTheme,
        selector: impl Into<SharedString>,
    ) -> AnyElement {
        let selector = selector.into();
        let markdown_selector = format!("{}-markdown", selector.as_ref());
        let Ok(document) = self.engine.parse_document(source) else {
            return plain_source(source, theme, markdown_selector);
        };

        if (!source.is_empty() && document.blocks().is_empty())
            || document.blocks().iter().any(block_needs_plain_fallback)
        {
            return plain_source(source, theme, markdown_selector);
        }

        let mut root = markdown_root(markdown_selector.clone(), theme);
        for (index, block) in document.blocks().iter().enumerate() {
            root = root.child(render_block(index, block, theme, &markdown_selector));
        }
        root.into_any_element()
    }
}

impl Default for MarkdownRenderer {
    fn default() -> Self {
        Self::new()
    }
}

fn block_needs_plain_fallback(block: &Block) -> bool {
    matches!(block, Block::Code(fence) if !fence.closed || fence.tokens.is_none())
}

fn markdown_root(selector: String, theme: ArtisanTheme) -> Div {
    let mut root = body_container(theme)
        .flex()
        .flex_col()
        .gap(theme.spacing.steps(3.0));
    root = root.debug_selector(move || selector);
    root
}

fn plain_source(source: &str, theme: ArtisanTheme, selector: String) -> AnyElement {
    let mut root = body_container(theme);
    root = root.debug_selector(move || selector);
    root.child(source.to_owned()).into_any_element()
}

fn body_container(theme: ArtisanTheme) -> Div {
    div()
        .w_full()
        .min_w_0()
        .text_size(theme.typography.editor_text_desktop)
        .line_height(theme.spacing.steps(6.0))
        .whitespace_normal()
}

fn render_block(
    index: usize,
    block: &Block,
    theme: ArtisanTheme,
    parent_selector: &str,
) -> AnyElement {
    render_block_at_depth(index, block, theme, parent_selector, 0)
}

fn render_block_at_depth(
    index: usize,
    block: &Block,
    theme: ArtisanTheme,
    parent_selector: &str,
    depth: u32,
) -> AnyElement {
    let selector = format!("{parent_selector}-block-{index}");
    let mut element = body_container(theme).flex().flex_col();

    match block {
        Block::Heading { level, spans, .. } => {
            element = element
                .font_family(theme.typography.heading.family)
                .font_weight(FontWeight::BOLD)
                .text_size(heading_size(*level, theme))
                .child(render_inline(&selector, spans, theme));
        }
        Block::Paragraph { spans, .. } => {
            element = element.child(render_inline(&selector, spans, theme));
        }
        Block::Code(fence) => {
            element = element.child(render_code(&selector, fence, theme));
        }
        Block::Html { source } => {
            element = element.child(source.clone());
        }
        Block::List {
            ordered,
            start,
            items,
            ..
        } => {
            element = element.child(render_list(
                &selector,
                *ordered,
                *start,
                items,
                theme,
                depth,
            ));
        }
    }
    element = element.debug_selector(move || selector);
    element.into_any_element()
}

/// Renders an ordered or unordered list as native rows: one marker plus the
/// item's own blocks. Nested lists recurse with one indent step per depth.
fn render_list(
    parent_selector: &str,
    ordered: bool,
    start: Option<u64>,
    items: &[ListItem],
    theme: ArtisanTheme,
    depth: u32,
) -> AnyElement {
    let mut list = div()
        .w_full()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(theme.spacing.steps(2.0));
    if depth > 0 {
        list = list.pl(theme.spacing.steps(4.0));
    }
    let base = start.unwrap_or(1);
    for (position, item) in items.iter().enumerate() {
        let marker = item_marker(ordered, base, position, item.task);
        let item_selector = format!("{parent_selector}-item-{position}");
        let mut content = div().flex().flex_1().min_w_0().flex_col();
        if item.blocks.is_empty() {
            let empty: &[Span] = &[];
            content = content.child(render_inline(&item_selector, empty, theme));
        }
        for (sub_index, block) in item.blocks.iter().enumerate() {
            content = content.child(render_block_at_depth(
                sub_index,
                block,
                theme,
                &item_selector,
                depth.saturating_add(1),
            ));
        }
        let mut row = div()
            .w_full()
            .min_w_0()
            .flex()
            .flex_row()
            .gap(theme.spacing.steps(2.0));
        row = row.child(
            div()
                .flex_shrink_0()
                .text_color(theme.colors.muted_foreground.to_paint())
                .child(marker),
        );
        row = row.child(content);
        let selector = item_selector.clone();
        row = row.debug_selector(move || selector);
        list = list.child(row);
    }
    let selector = format!("{parent_selector}-list");
    list.debug_selector(move || selector).into_any_element()
}

fn item_marker(ordered: bool, base: u64, position: usize, task: Option<bool>) -> String {
    if let Some(checked) = task {
        if checked {
            return String::from("☑");
        }
        return String::from("☐");
    }
    if ordered {
        format!("{}.", base.saturating_add(position as u64))
    } else {
        String::from("•")
    }
}

fn heading_size(level: u8, theme: ArtisanTheme) -> gpui::Pixels {
    let scale = match level {
        1 => 1.5,
        2 => 1.3,
        3 => 1.15,
        _ => 1.0,
    };
    theme.typography.editor_text_desktop * scale
}

fn render_inline(selector: &str, spans: &[Span], theme: ArtisanTheme) -> AnyElement {
    let presentation = present_inline(spans, theme);
    let text = StyledText::new(presentation.source).with_highlights(presentation.highlights);
    if presentation.links.is_empty() {
        return text.into_any_element();
    }
    let ranges = presentation
        .links
        .iter()
        .map(|link| link.range.clone())
        .collect::<Vec<_>>();
    let destinations = presentation
        .links
        .into_iter()
        .map(|link| link.destination)
        .collect::<Vec<_>>();
    InteractiveText::new(SharedString::from(selector.to_owned()), text)
        .on_click(ranges, move |index, _window, cx| {
            if let Some(destination) = destinations.get(index) {
                cx.open_url(destination);
            }
        })
        .into_any_element()
}

fn render_code(parent_selector: &str, fence: &CodeFence, theme: ArtisanTheme) -> AnyElement {
    let source = SharedString::from(fence.source.clone());
    let highlights = fence
        .tokens
        .as_deref()
        .unwrap_or_default()
        .iter()
        .filter_map(|token| valid_code_range(token, source.as_ref()))
        .map(|(range, kind)| (range, code_token_style(theme, kind)))
        .collect::<Vec<_>>();
    let text = StyledText::new(source).with_highlights(highlights);
    let selector = format!("{parent_selector}-code");
    let mut code = body_container(theme)
        .font_family(theme.typography.mono.family)
        .bg(theme.colors.muted.to_paint())
        .child(text);
    code = code.debug_selector(move || selector);
    code.into_any_element()
}

fn valid_code_range(token: &CodeToken, source: &str) -> Option<(Range<usize>, CodeTokenKind)> {
    let range = token.range.clone();
    (range.start <= range.end
        && range.end <= source.len()
        && source.is_char_boundary(range.start)
        && source.is_char_boundary(range.end))
    .then_some((range, token.kind))
}

/// One openable link run: its byte range in the flattened source plus the
/// verbatim absolute destination it opens. Carried separately from the
/// highlight runs so click handling and the future selection consumer share
/// one metadata source.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct InlineLink {
    /// Byte range of the visible label in the flattened source.
    pub range: Range<usize>,
    /// Verbatim absolute destination (`http(s)`/`mailto:` only).
    pub destination: String,
}

/// Owned inline presentation for one span slice: flattened text, one
/// ordered non-overlapping highlight list, and openable-link metadata.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct InlinePresentation {
    /// Flattened visible text.
    pub source: String,
    /// Highlight runs in source order, non-overlapping, on character
    /// boundaries: exactly what one `with_highlights` call consumes.
    pub highlights: Vec<(Range<usize>, HighlightStyle)>,
    /// Openable links in source order.
    pub links: Vec<InlineLink>,
}

/// Flattens spans into presentation data with a single merged highlight
/// list.
///
/// `StyledText::with_highlights` replaces its stored highlights on every
/// call, so code, strong, emphasis, and link ranges must merge into one
/// iterator here. Styles propagate recursively: each nested run inherits
/// its parent style combined through the existing
/// [`HighlightStyle::highlight`] helper, and leaf text emits one run only
/// when the inherited style is non-default. Emission is strictly
/// source-ordered, so runs arrive ordered and non-overlapping in a single
/// linear pass with no boundary sort and no per-segment tag scan — the old
/// sweep shape (sorted boundaries crossed with every tag range) was
/// quadratic in formatted-run count and stalled large replies that the
/// renderer re-parses on every render.
///
/// # Must use
///
/// The return value owns the flattened text the ranges address; dropping it
/// silently loses the presentation.
#[must_use]
pub fn present_inline(spans: &[Span], theme: ArtisanTheme) -> InlinePresentation {
    let mut accumulator = InlineAccumulator::default();
    flatten_spans(spans, HighlightStyle::default(), &mut accumulator, theme);
    InlinePresentation {
        source: accumulator.source,
        highlights: accumulator.runs,
        links: accumulator.links,
    }
}

#[derive(Default)]
struct InlineAccumulator {
    source: String,
    runs: Vec<(Range<usize>, HighlightStyle)>,
    links: Vec<InlineLink>,
}

/// Emits one leaf run, coalescing into the previous run when the style
/// matches and the text is contiguous.
fn emit_run(accumulator: &mut InlineAccumulator, start: usize, end: usize, style: HighlightStyle) {
    if start >= end || style == HighlightStyle::default() {
        return;
    }
    if let Some((last_range, last_style)) = accumulator.runs.last_mut() {
        if *last_style == style && last_range.end == start {
            last_range.end = end;
            return;
        }
    }
    accumulator.runs.push((start..end, style));
}

fn flatten_spans(
    spans: &[Span],
    inherited: HighlightStyle,
    accumulator: &mut InlineAccumulator,
    theme: ArtisanTheme,
) {
    for span in spans {
        match span {
            Span::Text(inline) | Span::Html(inline) => {
                let start = accumulator.source.len();
                accumulator.source.push_str(inline);
                emit_run(accumulator, start, accumulator.source.len(), inherited);
            }
            Span::Code(code) => {
                let start = accumulator.source.len();
                accumulator.source.push_str(code);
                emit_run(
                    accumulator,
                    start,
                    accumulator.source.len(),
                    inherited.highlight(inline_code_style(theme)),
                );
            }
            Span::Emphasis(inner) => {
                flatten_spans(inner, inherited.highlight(emphasis_style()), accumulator, theme);
            }
            Span::Strong(inner) => {
                flatten_spans(inner, inherited.highlight(strong_style()), accumulator, theme);
            }
            Span::Link { label, destination } => {
                if is_openable_link_destination(destination) {
                    let start = accumulator.source.len();
                    flatten_spans(
                        label,
                        inherited.highlight(link_style(theme)),
                        accumulator,
                        theme,
                    );
                    let end = accumulator.source.len();
                    if start < end {
                        accumulator.links.push(InlineLink {
                            range: start..end,
                            destination: destination.clone(),
                        });
                    }
                } else {
                    flatten_spans(label, inherited, accumulator, theme);
                }
            }
        }
    }
}

/// Only absolute `http(s)` and `mailto` destinations open through the
/// platform browser. Relative paths need a project base the renderer does
/// not own, so they keep their plain label instead of opening an arbitrary
/// local path; any other scheme stays inert. The engine still preserves
/// every destination verbatim in the model.
fn is_openable_link_destination(destination: &str) -> bool {
    let trimmed = destination.trim();
    if trimmed.is_empty() {
        return false;
    }
    let lower = trimmed.to_ascii_lowercase();
    lower.starts_with("https://") || lower.starts_with("http://") || lower.starts_with("mailto:")
}

fn inline_code_style(theme: ArtisanTheme) -> HighlightStyle {
    HighlightStyle {
        color: Some(theme.colors.accent_foreground.to_paint()),
        background_color: Some(theme.colors.muted.to_paint()),
        ..Default::default()
    }
}

fn strong_style() -> HighlightStyle {
    HighlightStyle {
        font_weight: Some(FontWeight::BOLD),
        ..Default::default()
    }
}

fn emphasis_style() -> HighlightStyle {
    HighlightStyle {
        font_style: Some(FontStyle::Italic),
        ..Default::default()
    }
}

fn link_style(theme: ArtisanTheme) -> HighlightStyle {
    HighlightStyle {
        color: Some(theme.colors.accent_foreground.to_paint()),
        underline: Some(UnderlineStyle {
            thickness: px(1.0),
            color: Some(theme.colors.accent_foreground.to_paint()),
            wavy: false,
        }),
        ..Default::default()
    }
}

fn code_token_style(theme: ArtisanTheme, kind: CodeTokenKind) -> HighlightStyle {
    let color = match kind {
        CodeTokenKind::Comment => theme.colors.muted_foreground,
        CodeTokenKind::Str => theme.colors.banner_success,
        CodeTokenKind::Number => theme.colors.banner_warning,
        CodeTokenKind::Keyword => theme.colors.accent_foreground,
        CodeTokenKind::Type => theme.colors.primary,
        CodeTokenKind::Function => theme.colors.question_to,
        CodeTokenKind::Plain => theme.colors.foreground,
    };
    HighlightStyle {
        color: Some(color.to_paint()),
        ..Default::default()
    }
}
