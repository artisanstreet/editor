//! Synchronous native rendering for an accepted Markdown message body.
//!
//! [`MarkdownRenderer`] is intentionally a small presentation seam over the
//! owned vocabulary in [`crate::markdown`]. It parses during the render pass
//! and immediately turns the resulting blocks into ordinary GPUI elements;
//! it does not retain message or document state.
//!
//! Lists render as native stacked rows with muted markers (`•` or `1.`)
//! instead of HTML list elements; emphasis and strong survive as inline
//! structure, and link labels render underlined in the accent token only
//! when the destination matches the Svelte anchor guard (absolute
//! `http(s)`/`mailto` or scheme-relative/relative paths). Anything else
//! falls back to its plain label, and raw HTML stays inert text.

#![allow(clippy::module_name_repetitions)]

use std::ops::Range;

use gpui::{
    AnyElement, Div, FontWeight, HighlightStyle, IntoElement, ParentElement, SharedString, Styled,
    StyledText, UnderlineStyle, div, prelude::InteractiveElement as _, px,
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
                .child(render_inline(spans, theme));
        }
        Block::Paragraph { spans, .. } => {
            element = element.child(render_inline(spans, theme));
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
            content = content.child(render_inline(empty, theme));
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

fn render_inline(spans: &[Span], theme: ArtisanTheme) -> StyledText {
    let InlineText {
        source,
        code_ranges,
        strong_ranges,
        link_ranges,
    } = inline_text(spans);
    let code_style = inline_code_style(theme);
    let strong_style = strong_style();
    let link_style = link_style(theme);
    StyledText::new(source)
        .with_highlights(code_ranges.into_iter().map(|range| (range, code_style)))
        .with_highlights(strong_ranges.into_iter().map(|range| (range, strong_style)))
        .with_highlights(link_ranges.into_iter().map(|range| (range, link_style)))
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

struct InlineText {
    source: String,
    code_ranges: Vec<Range<usize>>,
    strong_ranges: Vec<Range<usize>>,
    link_ranges: Vec<Range<usize>>,
}

fn inline_text(spans: &[Span]) -> InlineText {
    let mut text = InlineText {
        source: String::new(),
        code_ranges: Vec::new(),
        strong_ranges: Vec::new(),
        link_ranges: Vec::new(),
    };
    flatten_spans(spans, &mut text);
    text
}

fn flatten_spans(spans: &[Span], text: &mut InlineText) {
    for span in spans {
        match span {
            Span::Text(inline) | Span::Html(inline) => text.source.push_str(inline),
            Span::Code(code) => {
                let start = text.source.len();
                text.source.push_str(code);
                if start < text.source.len() {
                    text.code_ranges.push(start..text.source.len());
                }
            }
            Span::Emphasis(inner) => {
                // No native italic highlight in the pinned GPUI build; the
                // structure survives in the model and the label paints plain.
                flatten_spans(inner, text);
            }
            Span::Strong(inner) => {
                let start = text.source.len();
                flatten_spans(inner, text);
                if start < text.source.len() {
                    text.strong_ranges.push(start..text.source.len());
                }
            }
            Span::Link { label, destination } => {
                let start = text.source.len();
                flatten_spans(label, text);
                if start < text.source.len() && is_safe_link_destination(destination) {
                    text.link_ranges.push(start..text.source.len());
                }
            }
        }
    }
}

/// Mirrors the Svelte anchor guard (`anchor.svelte`): absolute `http(s)`
/// and `mailto` destinations are live; scheme-relative and relative paths
/// stay live; any other scheme (including `javascript:` and `data:`) falls
/// back to its plain label. The engine keeps every destination verbatim.
fn is_safe_link_destination(destination: &str) -> bool {
    let trimmed = destination.trim();
    if trimmed.is_empty() {
        return false;
    }
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("https://") || lower.starts_with("http://") || lower.starts_with("mailto:")
    {
        return true;
    }
    // Relative destinations carry no scheme before the first path, query,
    // or fragment delimiter; anything with a scheme colon is unsafe here.
    let scheme_end = trimmed
        .find(|character| matches!(character, '/' | '?' | '#'))
        .unwrap_or(trimmed.len());
    !trimmed[..scheme_end].contains(':')
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

fn link_style(theme: ArtisanTheme) -> HighlightStyle {
    HighlightStyle {
        color: Some(theme.colors.accent_foreground.to_paint()),
        underline: Some(UnderlineStyle {
            thickness: px(1.0),
            color: Some(theme.colors.accent_foreground),
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
