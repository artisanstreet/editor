//! Theme-independent inline flattening.
//!
//! One pass turns a span slice into flattened text plus runs that name their
//! style as a chain of [`InlineLayer`]s instead of a resolved
//! [`HighlightStyle`]. The chain records exactly which layers the old
//! resolved flattener composed, in order, so resolving it later through the
//! same `HighlightStyle::highlight` fold reproduces the resolved runs bit
//! for bit. That split is what lets a cached document keep its flattened
//! text across frames and theme changes while render only resolves colors.

use std::ops::Range;

use gpui::HighlightStyle;

use crate::markdown::Span;
use crate::theme::ArtisanTheme;

use super::{
    InlineLink, bare_url_spans, code_style, command_style, emphasis_style,
    is_openable_link_destination, link_style, strong_style,
};

/// One style layer the flattener composes onto its inherited style.
///
/// `in_link` records the link-ancestor context: the reference resolves
/// `a strong` and `a code` to `color: inherit`, so those layers drop their
/// own color inside a link.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) enum InlineLayer {
    /// Inline code.
    Code { in_link: bool },
    /// Emphasis (italic).
    Emphasis,
    /// Strong.
    Strong { in_link: bool },
    /// An openable link label.
    Link,
    /// A message's leading `/command`.
    Command,
}

impl InlineLayer {
    fn style(self, theme: &ArtisanTheme) -> HighlightStyle {
        match self {
            Self::Code { in_link } => code_style(theme, in_link),
            Self::Emphasis => emphasis_style(),
            Self::Strong { in_link } => strong_style(theme, in_link),
            Self::Link => link_style(theme),
            Self::Command => command_style(theme),
        }
    }
}

/// Resolves one chain the way the flattener composes styles: each nested
/// layer highlights over its parent, starting from the default style.
pub(super) fn resolve_chain(chain: &[InlineLayer], theme: &ArtisanTheme) -> HighlightStyle {
    chain
        .iter()
        .fold(HighlightStyle::default(), |inherited, layer| {
            inherited.highlight(layer.style(theme))
        })
}

/// Flattened inline content with theme-independent style runs.
///
/// Every range addresses `source`. `runs` are source-ordered and
/// non-overlapping; each names an index into `chains`, and touching runs
/// that share a chain are already merged.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct FlatInline {
    pub(super) source: String,
    pub(super) runs: Vec<(Range<usize>, usize)>,
    pub(super) chains: Vec<Vec<InlineLayer>>,
    pub(super) links: Vec<InlineLink>,
    pub(super) code_ranges: Vec<Range<usize>>,
    /// Whether color values become swatches (reply bodies) or stay text
    /// (the frozen [`super::InlinePresentation`] contract).
    pub(super) swatches: bool,
    /// Color values and their swatch slots, in source order.
    pub(super) colors: Vec<InlineColor>,
}

/// The em space a swatch paints over, held to its neighbors by word joiners
/// so it never wraps away from them: `(■)` stays together.
pub(super) const SWATCH_SLOT: &str = "\u{2060}\u{2003}\u{2060}";

/// One color value a reply wrote, as the flattened text shows it.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct InlineColor {
    /// What answers a right-click: the slot, plus the value when shown.
    pub(super) range: Range<usize>,
    /// The swatch slot within `range`.
    pub(super) slot: Range<usize>,
    /// The value as written.
    pub(super) value: String,
    /// Whether the swatch stands in for the value (bare in prose, or a code
    /// span holding only the value) instead of preceding it.
    pub(super) alone: bool,
}

impl FlatInline {
    /// Resolves the chain runs into highlight runs for `theme`.
    ///
    /// Touching runs whose chains differ can still resolve to the same
    /// style (strong inside strong reads as strong); they merge here exactly
    /// as the resolved flattener merged them, so the result matches it.
    pub(super) fn resolve_runs(&self, theme: &ArtisanTheme) -> Vec<(Range<usize>, HighlightStyle)> {
        let styles = self
            .chains
            .iter()
            .map(|chain| resolve_chain(chain, theme))
            .collect::<Vec<_>>();
        resolve_runs(&self.runs, &styles)
    }
}

/// Resolves chain runs against already resolved chain styles.
pub(super) fn resolve_runs(
    runs: &[(Range<usize>, usize)],
    styles: &[HighlightStyle],
) -> Vec<(Range<usize>, HighlightStyle)> {
    let mut resolved: Vec<(Range<usize>, HighlightStyle)> = Vec::with_capacity(runs.len());
    for (range, chain) in runs {
        let Some(style) = styles.get(*chain).copied() else {
            continue;
        };
        if range.start >= range.end || style == HighlightStyle::default() {
            continue;
        }
        if let Some((last_range, last_style)) = resolved.last_mut()
            && *last_style == style
            && last_range.end == range.start
        {
            last_range.end = range.end;
            continue;
        }
        resolved.push((range.clone(), style));
    }
    resolved
}

/// Flattens authored spans into text and style runs.
pub(super) fn flatten_inline(spans: &[Span]) -> FlatInline {
    let mut flat = FlatInline::default();
    let mut chain = Vec::new();
    flatten(spans, &mut chain, &mut flat, false);
    flat
}

/// Flattens like [`flatten_inline`], turning color values into swatches: a
/// bare value in prose, or a code span holding only a value, becomes a
/// square alone; a value inside longer code gets a square before it, the
/// way an editor decorates CSS colors.
pub(super) fn flatten_inline_with_swatches(spans: &[Span]) -> FlatInline {
    let mut flat = FlatInline {
        swatches: true,
        ..FlatInline::default()
    };
    let mut chain = Vec::new();
    flatten(spans, &mut chain, &mut flat, false);
    flat
}

/// Appends a swatch slot for `value`; `alone` when it stands in for the
/// value, which is then not written.
fn push_swatch(flat: &mut FlatInline, value: &str, alone: bool) -> Range<usize> {
    let start = flat.source.len();
    flat.source.push_str(SWATCH_SLOT);
    let slot = start..flat.source.len();
    if alone {
        flat.colors.push(InlineColor {
            range: slot.clone(),
            slot: slot.clone(),
            value: value.to_owned(),
            alone,
        });
    }
    slot
}

/// Appends prose text, its bare color values as lone swatches.
fn push_prose(flat: &mut FlatInline, text: &str, chain: &[InlineLayer]) {
    let colors = if flat.swatches {
        super::rich::color_ranges(text, &[])
    } else {
        Vec::new()
    };
    let mut cursor = 0;
    for color in colors {
        let start = flat.source.len();
        flat.source.push_str(&text[cursor..color.start]);
        let end = flat.source.len();
        emit(flat, start, end, chain);
        push_swatch(flat, &text[color.clone()], true);
        cursor = color.end;
    }
    let start = flat.source.len();
    flat.source.push_str(&text[cursor..]);
    let end = flat.source.len();
    emit(flat, start, end, chain);
}

/// Appends one code span: a lone swatch when it holds only a color value,
/// else the code with a swatch before each value in it.
fn push_code(flat: &mut FlatInline, code: &str, chain: &mut Vec<InlineLayer>, in_link: bool) {
    let colors = if flat.swatches && !in_link {
        let whole = 0..code.len();
        super::rich::color_ranges(code, std::slice::from_ref(&whole))
    } else {
        Vec::new()
    };
    if let [only] = colors.as_slice()
        && code[only.clone()] == *code.trim()
    {
        push_swatch(flat, code.trim(), true);
        return;
    }
    let start = flat.source.len();
    let mut cursor = 0;
    for color in colors {
        flat.source.push_str(&code[cursor..color.start]);
        let slot = push_swatch(flat, &code[color.clone()], false);
        flat.source.push_str(&code[color.clone()]);
        flat.colors.push(InlineColor {
            range: slot.start..flat.source.len(),
            slot,
            value: code[color.clone()].to_owned(),
            alone: false,
        });
        cursor = color.end;
    }
    flat.source.push_str(&code[cursor..]);
    let end = flat.source.len();
    if start < end {
        flat.code_ranges.push(start..end);
    }
    chain.push(InlineLayer::Code { in_link });
    emit(flat, start, end, chain);
    chain.pop();
}

/// Emits one leaf run, merging into the previous run when it continues the
/// same chain. An empty chain is the default style and emits nothing.
fn emit(flat: &mut FlatInline, start: usize, end: usize, chain: &[InlineLayer]) {
    if start >= end || chain.is_empty() {
        return;
    }
    let id = flat
        .chains
        .iter()
        .position(|known| known == chain)
        .unwrap_or_else(|| {
            flat.chains.push(chain.to_vec());
            flat.chains.len() - 1
        });
    if let Some((last_range, last_id)) = flat.runs.last_mut()
        && *last_id == id
        && last_range.end == start
    {
        last_range.end = end;
        return;
    }
    flat.runs.push((start..end, id));
}

/// Flattens with link-ancestor context. Emission is strictly source-ordered,
/// so runs arrive ordered and non-overlapping in one linear pass.
fn flatten(spans: &[Span], chain: &mut Vec<InlineLayer>, flat: &mut FlatInline, in_link: bool) {
    for span in spans {
        match span {
            Span::Text(inline) | Span::Html(inline) => {
                if matches!(span, Span::Text(_))
                    && !in_link
                    && let Some(linked) = bare_url_spans(inline)
                {
                    flatten(&linked, chain, flat, true);
                    continue;
                }
                if in_link {
                    let start = flat.source.len();
                    flat.source.push_str(inline);
                    let end = flat.source.len();
                    emit(flat, start, end, chain);
                } else {
                    push_prose(flat, inline, chain);
                }
            }
            Span::Code(code) => push_code(flat, code, chain, in_link),
            Span::Command(command) => {
                chain.push(InlineLayer::Command);
                let start = flat.source.len();
                flat.source.push_str(command);
                let end = flat.source.len();
                emit(flat, start, end, chain);
                chain.pop();
            }
            Span::Emphasis(inner) => {
                chain.push(InlineLayer::Emphasis);
                flatten(inner, chain, flat, in_link);
                chain.pop();
            }
            Span::Strong(inner) => {
                chain.push(InlineLayer::Strong { in_link });
                flatten(inner, chain, flat, in_link);
                chain.pop();
            }
            Span::Link { label, destination } => {
                if is_openable_link_destination(destination) {
                    flatten_link(label, destination, chain, flat);
                } else {
                    flatten(label, chain, flat, in_link);
                }
            }
        }
    }
}

/// Flattens an authored link label and records its destination.
fn flatten_link(
    label: &[Span],
    destination: &str,
    chain: &mut Vec<InlineLayer>,
    flat: &mut FlatInline,
) {
    let start = flat.source.len();
    chain.push(InlineLayer::Link);
    flatten(label, chain, flat, true);
    chain.pop();
    let end = flat.source.len();
    if start < end {
        flat.links.push(InlineLink {
            range: start..end,
            destination: destination.to_owned(),
        });
    }
}
