//! Synchronous native rendering for an accepted Markdown message body.
//!
//! [`MarkdownRenderer`] is intentionally a small presentation seam over the
//! owned vocabulary in [`crate::markdown`]. It parses during the render pass
//! and immediately turns the resulting blocks into ordinary GPUI elements;
//! it does not retain message or document state.
//!
//! Every text leaf — paragraph and heading runs, code blocks, HTML carried
//! as inert text, and the plain-source fallback — renders through retained
//! [`SelectableText`](crate::selectable_text::SelectableText), so transcript
//! bodies share one drag-select, copy, and link behavior with no
//! caller-side per-block state. Selection, drag latch, and focus live in
//! framework element state under stable selector-derived ids; the renderer
//! itself stays synchronous and stateless.
//!
//! Lists render as native stacked rows with muted markers (`•` or `1.`)
//! instead of HTML list elements; emphasis and strong survive as inline
//! structure at reference weights (strong 600, links 500), and absolute
//! `http(s)`/`mailto:` link labels render underlined in the accent token
//! and open through the platform browser on click.
//! Relative and other-scheme destinations keep their plain label: with no
//! project base the renderer must not open arbitrary local paths, and raw
//! HTML stays inert text.
//!
//! Spacing and type follow [`ProseTypography`](crate::theme::ProseTypography):
//! 16 px / 28 px body at weight 410, per-heading sizes with collapsing
//! block margins, and fence chrome from the reference code snippet. Inline
//! code reads 400 muted with no wash; mono face and normal tracking ride
//! the frozen text-run contract through `InlinePresentation.code_ranges`
//! (see `code_style`). Size has no override in that API, so inline code
//! keeps the inherited 16 px instead of the reference 0.875 em.

#![allow(clippy::module_name_repetitions)]

use std::ops::Range;

use gpui::{
    AnyElement, Div, FontStyle, FontWeight, HighlightStyle, IntoElement, ParentElement,
    SharedString, Styled, div, prelude::InteractiveElement as _, px,
};

use crate::markdown::{Block, CodeFence, CodeToken, CodeTokenKind, ListItem, MarkdownEngine, Span};
use crate::selectable_text::{SelectableText, TextRunOverride};
use crate::theme::{
    ArtisanTheme, Oklch, ProseTypography, RadiusStep, RadiusTokens, SurfaceStep, ThemeMode,
};

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
        self.render_source_with_tone(source, theme, selector, MarkdownBodyTone::Muted)
    }

    /// Parses and renders one source body with an explicit body text tone.
    ///
    /// Assistant replies render [`MarkdownBodyTone::Foreground`]; every other
    /// message keeps the reference muted body.
    #[must_use]
    pub fn render_source_with_tone(
        &self,
        source: &str,
        theme: ArtisanTheme,
        selector: impl Into<SharedString>,
        tone: MarkdownBodyTone,
    ) -> AnyElement {
        let selector = selector.into();
        let markdown_selector = format!("{}-markdown", selector.as_ref());
        let Ok(document) = self.engine.parse_document(source) else {
            return plain_source(source, theme, markdown_selector, tone);
        };

        if (!source.is_empty() && document.blocks().is_empty())
            || document.blocks().iter().any(block_needs_plain_fallback)
        {
            return plain_source(source, theme, markdown_selector, tone);
        }

        let mut root = markdown_root(markdown_selector.clone(), theme, tone);
        let blocks = document.blocks();
        let gaps = block_gaps(blocks, BlockScope::Root);
        for (index, block) in blocks.iter().enumerate() {
            root = root.child(with_block_margins(
                render_block(index, block, theme, &markdown_selector, tone),
                gaps[index],
            ));
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

fn markdown_root(selector: String, theme: ArtisanTheme, tone: MarkdownBodyTone) -> Div {
    // No container gap: inter-block spacing lives in per-block margins so
    // collapsing behavior matches the reference.
    let mut root = body_container(theme, tone).flex().flex_col();
    root = root.debug_selector(move || selector);
    root
}

fn plain_source(
    source: &str,
    theme: ArtisanTheme,
    selector: String,
    tone: MarkdownBodyTone,
) -> AnyElement {
    let id = SharedString::from(format!("{selector}-plain"));
    let mut root = body_container(theme, tone);
    root = root.debug_selector(move || selector);
    root.child(SelectableText::retained(
        id,
        source.to_owned(),
        theme,
        Vec::new(),
    ))
    .into_any_element()
}

/// Which body text tone a rendered message carries.
///
/// The reference prose body reads muted; the assistant reply is explicitly
/// promoted to the foreground token per product direction, so the tone
/// rides the render entry instead of living in any shared default.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum MarkdownBodyTone {
    /// Reference muted prose body.
    Muted,
    /// Assistant reply body in the foreground token.
    Foreground,
}

/// Returns the theme body color for one markdown body tone.
///
/// Span-level recipes (strong, code, links, headings) carry their own exact
/// tokens and never consult this; only untagged body text inherits it.
#[must_use]
pub fn markdown_body_text_color(tone: MarkdownBodyTone, theme: ArtisanTheme) -> Oklch {
    match tone {
        MarkdownBodyTone::Muted => theme.colors.muted_foreground,
        MarkdownBodyTone::Foreground => theme.colors.foreground,
    }
}

fn body_container(theme: ArtisanTheme, tone: MarkdownBodyTone) -> Div {
    div()
        .w_full()
        .min_w_0()
        .text_size(px(ProseTypography::BODY_SIZE_PX))
        .line_height(px(ProseTypography::BODY_LINE_PX))
        .font_weight(ProseTypography::BODY_WEIGHT)
        .letter_spacing(px(ProseTypography::BODY_TRACKING_PX))
        .text_color(markdown_body_text_color(tone, theme).to_paint())
        .whitespace_normal()
}

/// Which margin recipe a block uses: root blocks take plugin margins,
/// while paragraphs and nested lists inside list items take the
/// item-scope recipe (`> ul > li p`, `ul ul`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockScope {
    /// Top-level transcript blocks.
    Root,
    /// Blocks inside a list item.
    Item,
}

/// Reference top/bottom margins for one block in px.
fn block_margins(block: &Block, scope: BlockScope) -> (f32, f32) {
    match block {
        Block::Heading { level, .. } => {
            let heading = ProseTypography::heading(*level);
            (heading.margin_top_px, heading.margin_bottom_px)
        }
        Block::Paragraph { .. } => match scope {
            BlockScope::Root => (
                ProseTypography::PARAGRAPH_MARGIN_PX,
                ProseTypography::PARAGRAPH_MARGIN_PX,
            ),
            BlockScope::Item => (
                ProseTypography::ITEM_PARAGRAPH_MARGIN_PX,
                ProseTypography::ITEM_PARAGRAPH_MARGIN_PX,
            ),
        },
        Block::Code(_) => (
            ProseTypography::FENCE_MARGIN_PX,
            ProseTypography::FENCE_MARGIN_PX,
        ),
        // Raw HTML arrives as inert text, so it reads as a paragraph.
        Block::Html { .. } => (
            ProseTypography::PARAGRAPH_MARGIN_PX,
            ProseTypography::PARAGRAPH_MARGIN_PX,
        ),
        Block::List { .. } => match scope {
            BlockScope::Root => (
                ProseTypography::LIST_MARGIN_PX,
                ProseTypography::LIST_MARGIN_PX,
            ),
            BlockScope::Item => (
                ProseTypography::NESTED_LIST_MARGIN_PX,
                ProseTypography::NESTED_LIST_MARGIN_PX,
            ),
        },
    }
}

/// Wraps one rendered block with its collapsed top gap; bottom stays zero
/// because every gap renders exactly once (see [`block_gaps`]).
fn with_block_margins(child: AnyElement, top_px: f32) -> AnyElement {
    div().mt(px(top_px)).child(child).into_any_element()
}

/// Collapsed top gaps for one block sequence in source order.
///
/// CSS margins collapse: the space between two blocks is the larger of the
/// two facing margins, never their sum — and a flex column does not
/// collapse at all, so emitting both a bottom margin and a collapsed top
/// would double every gap (two paragraphs would read 20 + 20 = 40 instead
/// of 20). Each gap here is therefore rendered exactly once, as top margin
/// with zero bottom: the first block takes 0, and every other gap is the
/// max of the facing margins. Followers of h2–h4 keep the heading's own
/// bottom (`h2/h3/h4 + *` zeroes only the follower's top margin, so the
/// collapse still reads the full 24 px after an h2).
#[must_use]
pub fn block_gaps(blocks: &[Block], scope: BlockScope) -> Vec<f32> {
    let mut gaps = Vec::with_capacity(blocks.len());
    let mut previous_bottom: f32 = 0.0;
    let mut previous_zeroes_follower = false;
    for (index, block) in blocks.iter().enumerate() {
        let (margin_top, margin_bottom) = block_margins(block, scope);
        let gap = if index == 0 {
            0.0
        } else if previous_zeroes_follower {
            previous_bottom
        } else {
            previous_bottom.max(margin_top)
        };
        gaps.push(gap);
        previous_bottom = margin_bottom;
        previous_zeroes_follower = matches!(block, Block::Heading { level: 2 | 3 | 4, .. });
    }
    gaps
}

fn render_block(
    index: usize,
    block: &Block,
    theme: ArtisanTheme,
    parent_selector: &str,
    tone: MarkdownBodyTone,
) -> AnyElement {
    render_block_at_depth(index, block, theme, parent_selector, 0, tone)
}

fn render_block_at_depth(
    index: usize,
    block: &Block,
    theme: ArtisanTheme,
    parent_selector: &str,
    depth: u32,
    tone: MarkdownBodyTone,
) -> AnyElement {
    let selector = format!("{parent_selector}-block-{index}");
    let mut element = body_container(theme, tone).flex().flex_col();

    match block {
        Block::Heading { level, spans, .. } => {
            let heading = ProseTypography::heading(*level);
            element = element
                .font_family(theme.typography.heading.family)
                .font_weight(ProseTypography::HEADING_WEIGHT)
                .text_size(px(heading.size_px))
                .line_height(px(heading.line_px))
                .letter_spacing(px(heading.tracking_px))
                .text_color(theme.colors.foreground.to_paint())
                .child(render_inline(&selector, spans, theme));
        }
        Block::Paragraph { spans, .. } => {
            element = element.child(render_inline(&selector, spans, theme));
        }
        Block::Code(fence) => {
            element = element.child(render_code(&selector, fence, theme));
        }
        Block::Html { source } => {
            // Carried verbatim as inert data, exactly as before, but
            // selectable like every other transcript leaf.
            let id = SharedString::from(format!("{selector}-html"));
            element = element.child(SelectableText::retained(
                id,
                source.clone(),
                theme,
                Vec::new(),
            ));
        }
        Block::List {
            ordered,
            start,
            items,
            ..
        } => {
            element = element.child(render_list(
                &selector, *ordered, *start, items, theme, depth, tone,
            ));
        }
    }
    element = element.debug_selector(move || selector);
    element.into_any_element()
}

/// Renders an ordered or unordered list as native rows: one marker plus the
/// item's own blocks. Rows keep the `li` 8 px pitch; item content blocks
/// collapse with the item-scope recipe and zero outer margins, so a tight
/// single-paragraph item reads exactly its row pitch. Nested lists recurse
/// with the reference 26 px list indent.
fn render_list(
    parent_selector: &str,
    ordered: bool,
    start: Option<u64>,
    items: &[ListItem],
    theme: ArtisanTheme,
    depth: u32,
    tone: MarkdownBodyTone,
) -> AnyElement {
    let mut list = div()
        .w_full()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(px(ProseTypography::ITEM_GAP_PX))
        .pl(px(ProseTypography::LIST_INDENT_PX));
    if depth > 0 {
        list = list.pl(px(ProseTypography::LIST_INDENT_PX));
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
        let gaps = block_gaps(&item.blocks, BlockScope::Item);
        for (sub_index, block) in item.blocks.iter().enumerate() {
            content = content.child(with_block_margins(
                render_block_at_depth(
                    sub_index,
                    block,
                    theme,
                    &item_selector,
                    depth.saturating_add(1),
                    tone,
                ),
                gaps[sub_index],
            ));
        }
        let mut row = div()
            .w_full()
            .min_w_0()
            .flex()
            .flex_row()
            .gap(theme.spacing.steps(2.0));
        // Ordered markers read at 400 (`ol > li::marker`); unordered
        // markers inherit body weight like the reference.
        let mut marker_element = div()
            .flex_shrink_0()
            .text_color(theme.colors.muted_foreground.to_paint());
        if ordered {
            marker_element = marker_element.font_weight(FontWeight::NORMAL);
        }
        row = row.child(marker_element.child(marker));
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

fn render_inline(selector: &str, spans: &[Span], theme: ArtisanTheme) -> AnyElement {
    let presentation = present_inline(spans, theme);
    let id = SharedString::from(selector.to_owned());
    let text = SharedString::from(presentation.source);
    // Inline code rides the frozen text-run contract: mono family plus
    // zero tracking per code range. Weight (400) and muted color already
    // ride the highlight runs.
    let overrides = presentation
        .code_ranges
        .iter()
        .map(|range| TextRunOverride {
            range: range.clone(),
            font_family: Some(SharedString::from(theme.typography.mono.family)),
            letter_spacing: Some(px(0.0)),
        })
        .collect::<Vec<_>>();
    let element = SelectableText::retained(id, text, theme, presentation.highlights)
        .with_text_run_overrides(overrides);
    if presentation.links.is_empty() {
        return element.into_any_element();
    }
    // The selection element owns link clicks and suppresses drag
    // activation, so no separate click handler lives beside it: one
    // element, one behavior.
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
    element
        .links(ranges, move |index, _window, cx| {
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
    let selector = format!("{parent_selector}-code");
    let id = SharedString::from(format!("{selector}-text"));
    // Fence chrome follows `docs-code-snippet-body`: 14 px mono at 24 px
    // leading with 16 px padding and the shared 22 px 3xl radius, reading
    // in the foreground pre-code token at 400 with normal tracking
    // (plugin `pre`, `code` letter-spacing reset), not the muted body
    // 410/−0.64. The face is the reference vertical gradient
    // (`bg-linear-to-t from-surface-50 to-surface-125`, dark 950→900)
    // through the existing gradient helper; copy and filename chrome have
    // no renderer action counterpart. The `card-lg` shadow stack paints
    // through the shared recipe (no new machinery).
    let (gradient_top, gradient_bottom) = match theme.mode {
        ThemeMode::Light => (SurfaceStep::S125.oklch(), SurfaceStep::S50.oklch()),
        ThemeMode::Dark => (SurfaceStep::S900.oklch(), SurfaceStep::S950.oklch()),
    };
    let card_lg = theme
        .elevation
        .card_lg_shadow
        .into_iter()
        .map(|layer| layer.to_box_shadow())
        .collect::<Vec<_>>();
    let mut code = body_container(theme, MarkdownBodyTone::Muted)
        .font_family(theme.typography.mono.family)
        .text_size(px(ProseTypography::CODE_SIZE_PX))
        .line_height(px(ProseTypography::CODE_LINE_PX))
        .font_weight(FontWeight::NORMAL)
        .letter_spacing(px(0.0))
        .text_color(theme.colors.foreground.to_paint())
        .bg(crate::gradient::vertical_gradient(
            gradient_top,
            gradient_bottom,
        ))
        .rounded(RadiusTokens::value(RadiusStep::X3l))
        .shadow(card_lg)
        .p(px(ProseTypography::CODE_PAD_PX))
        .child(SelectableText::retained(id, source, theme, highlights));
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
/// ordered non-overlapping highlight list, openable-link metadata, and
/// inline-code ranges.
///
/// FROZEN range contract for the selection extension: `source` owns the
/// text every range addresses; `highlights` are sorted, non-overlapping,
/// and on character boundaries (exactly what one `with_highlights` call
/// consumes); `links` are openable ranges in source order;
/// `code_ranges` are the inline-code sub-ranges in source order,
/// non-overlapping and on character boundaries (exactly what
/// `with_font_family_overrides` consumes for the mono face, with run-level
/// tracking reset beside it). Additive changes only — never reshape.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct InlinePresentation {
    /// Flattened visible text.
    pub source: String,
    /// Highlight runs in source order, non-overlapping, on character
    /// boundaries: exactly what one `with_highlights` call consumes.
    pub highlights: Vec<(Range<usize>, HighlightStyle)>,
    /// Openable links in source order.
    pub links: Vec<InlineLink>,
    /// Inline-code ranges in source order for family/tracking treatment.
    pub code_ranges: Vec<Range<usize>>,
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
        code_ranges: accumulator.code_ranges,
    }
}

#[derive(Default)]
struct InlineAccumulator {
    source: String,
    runs: Vec<(Range<usize>, HighlightStyle)>,
    links: Vec<InlineLink>,
    code_ranges: Vec<Range<usize>>,
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
    flatten_spans_in_link(spans, inherited, accumulator, theme, false);
}

/// Flattens with link-ancestor context: the reference resolves `a strong`
/// and `a code` to `color: inherit`, so strong and code inside a link keep
/// the link color instead of imposing foreground/muted.
fn flatten_spans_in_link(
    spans: &[Span],
    inherited: HighlightStyle,
    accumulator: &mut InlineAccumulator,
    theme: ArtisanTheme,
    in_link: bool,
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
                let end = accumulator.source.len();
                if start < end {
                    accumulator.code_ranges.push(start..end);
                }
                emit_run(
                    accumulator,
                    start,
                    end,
                    inherited.highlight(code_style(theme, in_link)),
                );
            }
            Span::Emphasis(inner) => {
                flatten_spans_in_link(
                    inner,
                    inherited.highlight(emphasis_style()),
                    accumulator,
                    theme,
                    in_link,
                );
            }
            Span::Strong(inner) => {
                flatten_spans_in_link(
                    inner,
                    inherited.highlight(strong_style(theme, in_link)),
                    accumulator,
                    theme,
                    in_link,
                );
            }
            Span::Link { label, destination } => {
                if is_openable_link_destination(destination) {
                    let start = accumulator.source.len();
                    flatten_spans_in_link(
                        label,
                        inherited.highlight(link_style(theme)),
                        accumulator,
                        theme,
                        true,
                    );
                    let end = accumulator.source.len();
                    if start < end {
                        accumulator.links.push(InlineLink {
                            range: start..end,
                            destination: destination.clone(),
                        });
                    }
                } else {
                    flatten_spans_in_link(label, inherited, accumulator, theme, in_link);
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

fn code_style(theme: ArtisanTheme, in_link: bool) -> HighlightStyle {
    // Reference inline code reads 400 muted with no wash (`prose.css`
    // inline-code over plugin `code`), except inside a link where `a code`
    // inherits the link color. The 400 weight rides here; mono face and
    // normal tracking ride the frozen text-run contract
    // (`InlinePresentation.code_ranges` → `TextRunOverride` with the mono
    // family and zero tracking). Size deliberately has no override in that
    // API, so inline code keeps the inherited 16 px instead of the
    // reference 0.875 em (14 px) — a stated limit, not parity.
    HighlightStyle {
        color: if in_link {
            None
        } else {
            Some(theme.colors.muted_foreground.to_paint())
        },
        font_weight: Some(FontWeight::NORMAL),
        ..Default::default()
    }
}

fn strong_style(theme: ArtisanTheme, in_link: bool) -> HighlightStyle {
    // Plugin `strong` 600 in the headings/bold foreground token
    // (`prose.css` bold rule); body inheritance would dim it to muted.
    // Inside a link, `a strong` inherits the link color instead.
    HighlightStyle {
        color: if in_link {
            None
        } else {
            Some(theme.colors.foreground.to_paint())
        },
        font_weight: Some(ProseTypography::STRONG_WEIGHT),
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
    // Conversation links always render through the anchor component
    // (`ProseA`), i.e. the `conversation-link` class: blue with no
    // underline (`prose.css` links/conversation-link rules), never the
    // plain-`a` foreground+underline. `banner_info` is exactly that blue
    // per mode (Tailwind blue-500 light / blue-400 dark).
    HighlightStyle {
        color: Some(theme.colors.banner_info.to_paint()),
        font_weight: Some(ProseTypography::LINK_WEIGHT),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::ThemeMode;

    #[test]
    fn reply_body_tone_is_foreground_detail_body_stays_muted() {
        let theme = ArtisanTheme::for_mode(ThemeMode::Dark);
        assert_eq!(
            markdown_body_text_color(MarkdownBodyTone::Foreground, theme),
            theme.colors.foreground
        );
        assert_eq!(
            markdown_body_text_color(MarkdownBodyTone::Muted, theme),
            theme.colors.muted_foreground
        );
    }
}
