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
    InlineLink, RichLinkTitleSource, bare_url_spans, code_style, emphasis_style,
    is_openable_link_destination, is_rich_link_destination, is_source_attribution_label,
    link_style, readable_citations, span_label_text, strong_style,
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
}

impl InlineLayer {
    fn style(self, theme: &ArtisanTheme) -> HighlightStyle {
        match self {
            Self::Code { in_link } => code_style(theme, in_link),
            Self::Emphasis => emphasis_style(),
            Self::Strong { in_link } => strong_style(theme, in_link),
            Self::Link => link_style(theme),
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
/// that share a chain are already merged. `probes` lists every openable
/// destination in the order the flattener consulted the title source.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct FlatInline {
    pub(super) source: String,
    pub(super) runs: Vec<(Range<usize>, usize)>,
    pub(super) chains: Vec<Vec<InlineLayer>>,
    pub(super) links: Vec<InlineLink>,
    pub(super) citation_links: Vec<InlineLink>,
    pub(super) code_ranges: Vec<Range<usize>>,
    pub(super) icon_offsets: Vec<(usize, String)>,
    pub(super) probes: Vec<String>,
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

/// Flattens `spans` into text plus chain runs, consulting `titles` for
/// resolved rich-link titles and favicon slots.
pub(super) fn flatten_inline(spans: &[Span], titles: &dyn RichLinkTitleSource) -> FlatInline {
    let mut flat = FlatInline::default();
    let mut chain = Vec::new();
    flatten(spans, &mut chain, &mut flat, titles, false);
    flat
}

/// Flattens like [`flatten_inline`], turning color values into swatches: a
/// bare value in prose, or a code span holding only a value, becomes a
/// square alone; a value inside longer code gets a square before it, the
/// way an editor decorates CSS colors.
pub(super) fn flatten_inline_with_swatches(
    spans: &[Span],
    titles: &dyn RichLinkTitleSource,
) -> FlatInline {
    let mut flat = FlatInline {
        swatches: true,
        ..FlatInline::default()
    };
    let mut chain = Vec::new();
    flatten(spans, &mut chain, &mut flat, titles, false);
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
    let id = match flat.chains.iter().position(|known| known == chain) {
        Some(id) => id,
        None => {
            flat.chains.push(chain.to_vec());
            flat.chains.len() - 1
        }
    };
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
fn flatten(
    spans: &[Span],
    chain: &mut Vec<InlineLayer>,
    flat: &mut FlatInline,
    titles: &dyn RichLinkTitleSource,
    in_link: bool,
) {
    for span in spans {
        match span {
            Span::Text(inline) | Span::Html(inline) => {
                let cleaned;
                let inline = if inline.contains('\u{e200}') {
                    cleaned = readable_citations(inline);
                    &cleaned
                } else {
                    inline
                };
                if matches!(span, Span::Text(_))
                    && !in_link
                    && let Some(linked) = bare_url_spans(inline)
                {
                    flatten(&linked, chain, flat, titles, true);
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
            Span::Emphasis(inner) => {
                chain.push(InlineLayer::Emphasis);
                flatten(inner, chain, flat, titles, in_link);
                chain.pop();
            }
            Span::Strong(inner) => {
                chain.push(InlineLayer::Strong { in_link });
                flatten(inner, chain, flat, titles, in_link);
                chain.pop();
            }
            Span::Link { label, destination } => {
                if is_openable_link_destination(destination) {
                    flatten_link(label, destination, chain, flat, titles);
                } else {
                    flatten(label, chain, flat, titles, in_link);
                }
            }
        }
    }
}

/// Flattens one openable link: resolved title or authored label, favicon
/// slot, and the link and citation records.
fn flatten_link(
    label: &[Span],
    destination: &str,
    chain: &mut Vec<InlineLayer>,
    flat: &mut FlatInline,
    titles: &dyn RichLinkTitleSource,
) {
    let start = flat.source.len();
    flat.probes.push(destination.to_owned());
    let rich = is_rich_link_destination(destination);
    let source_attribution = rich && is_source_attribution_label(&span_label_text(label));
    // Title before favicon: the order the title source has always seen.
    let resolved = rich
        .then(|| titles.resolved_title(destination))
        .flatten()
        .filter(|title| !title.trim().is_empty());
    if titles.favicon(destination).is_some() {
        flat.icon_offsets.push((start, destination.to_owned()));
        flat.source.push_str("\u{2003}\u{2060}\u{00a0}");
    }
    chain.push(InlineLayer::Link);
    let label_text = span_label_text(label);
    if let Some(title) = resolved {
        flat.source.push_str(&capped_link_text(title.as_ref()));
        let end = flat.source.len();
        emit(flat, start, end, chain);
    } else if label_text.trim() == destination {
        // A pure link (bare, autolinked, or labelled with its own address)
        // shows the same capped width as a resolved title.
        flat.source.push_str(&capped_link_text(label_text.trim()));
        let end = flat.source.len();
        emit(flat, start, end, chain);
    } else {
        flatten(label, chain, flat, titles, true);
    }
    chain.pop();
    let end = flat.source.len();
    if start < end {
        let link = InlineLink {
            range: start..end,
            destination: destination.to_owned(),
        };
        if source_attribution {
            flat.citation_links.push(link.clone());
        }
        flat.links.push(link);
    }
}

/// The most characters a resolved link title or a pure link shows; longer
/// text ends in an ellipsis so one link never runs across the column.
pub(super) const LINK_TEXT_MAX_CHARS: usize = 48;

/// `text` cut to [`LINK_TEXT_MAX_CHARS`] characters, ending in `…` when cut.
pub(super) fn capped_link_text(text: &str) -> std::borrow::Cow<'_, str> {
    if text.chars().count() <= LINK_TEXT_MAX_CHARS {
        return std::borrow::Cow::Borrowed(text);
    }
    let kept: String = text.chars().take(LINK_TEXT_MAX_CHARS - 1).collect();
    std::borrow::Cow::Owned(format!("{}…", kept.trim_end()))
}

/// Every channel of one inline presentation that addresses its text, with
/// the run style left generic so resolved and chain runs share one splitter.
pub(super) struct InlineChannels<'a, S> {
    pub(super) source: &'a mut String,
    pub(super) runs: &'a mut Vec<(Range<usize>, S)>,
    pub(super) links: &'a mut Vec<InlineLink>,
    pub(super) citation_links: &'a mut Vec<InlineLink>,
    pub(super) code_ranges: &'a mut Vec<Range<usize>>,
    pub(super) icon_offsets: &'a mut Vec<(usize, String)>,
}

/// Hoists trailing citation attributions out of one inline presentation.
///
/// Returns the stripped attribution links in source order. A trailing run
/// qualifies link by link from the end: each entry must be a recorded
/// `Source`/`Sources` attribution whose range ends exactly where the
/// remaining body ends (trailing whitespace ignored), so
/// `…Morrissey.[Source](https://…)` strips the chip while `see [Source](…).
/// Next.` and mid-sentence attributions stay inline. Stripped ranges are
/// dropped from every channel — links, runs, code ranges, and favicon
/// slots — and the body is end-trimmed, so all surviving ranges still
/// address the remaining source.
pub(super) fn split_trailing_citations<S>(channels: InlineChannels<'_, S>) -> Vec<InlineLink> {
    let InlineChannels {
        source,
        runs,
        links,
        citation_links,
        code_ranges,
        icon_offsets,
    } = channels;
    let mut stripped: Vec<InlineLink> = Vec::new();
    loop {
        let trimmed_len = source.trim_end().len();
        let qualifies = citation_links
            .last()
            .is_some_and(|last| last.range.end == trimmed_len);
        if !qualifies {
            break;
        }
        let Some(link) = citation_links.pop() else {
            break;
        };
        source.truncate(link.range.start);
        stripped.push(link);
    }
    stripped.reverse();
    if stripped.is_empty() {
        return stripped;
    }
    let end = source.trim_end().len();
    source.truncate(end);
    runs.retain_mut(|(range, _)| {
        if range.start >= end {
            false
        } else {
            range.end = range.end.min(end);
            true
        }
    });
    code_ranges.retain_mut(|range| {
        if range.start >= end {
            false
        } else {
            range.end = range.end.min(end);
            true
        }
    });
    links.retain(|link| link.range.end <= end);
    citation_links.retain(|link| link.range.end <= end);
    icon_offsets.retain(|(offset, _)| *offset < end);
    stripped
}

#[cfg(test)]
mod link_cap_tests {
    use super::{LINK_TEXT_MAX_CHARS, capped_link_text};

    #[test]
    fn long_link_text_ends_in_an_ellipsis_at_the_cap() {
        assert_eq!(capped_link_text("Short title"), "Short title");
        let long = "https://github.com/artisanstreet/editor/pull/502/files#diff-0123456789abcdef";
        let capped = capped_link_text(long);
        assert_eq!(capped.chars().count(), LINK_TEXT_MAX_CHARS);
        assert!(capped.ends_with('…'));
        assert!(long.starts_with(capped.trim_end_matches('…')));
    }
}

#[cfg(test)]
mod swatch_tests {
    use super::super::NoRichLinkTitles;
    use super::{SWATCH_SLOT, flatten_inline, flatten_inline_with_swatches};
    use crate::markdown::Span;

    fn text(value: &str) -> Span {
        Span::Text(value.to_owned())
    }

    fn code(value: &str) -> Span {
        Span::Code(value.to_owned())
    }

    #[test]
    fn a_bare_color_in_prose_becomes_a_lone_swatch() {
        let flat =
            flatten_inline_with_swatches(&[text("Coral (#FF6B6B) is closest")], &NoRichLinkTitles);
        assert_eq!(flat.source, format!("Coral ({SWATCH_SLOT}) is closest"));
        let [color] = flat.colors.as_slice() else {
            panic!("one color: {:?}", flat.colors);
        };
        assert!(color.alone);
        assert_eq!(color.value, "#FF6B6B");
        assert_eq!(&flat.source[color.range.clone()], SWATCH_SLOT);
    }

    #[test]
    fn a_code_span_of_only_a_color_becomes_a_lone_swatch_without_its_chip() {
        let flat = flatten_inline_with_swatches(
            &[text("roughly "), code("oklch(0.711 0.183 22.5)"), text(".")],
            &NoRichLinkTitles,
        );
        assert_eq!(flat.source, format!("roughly {SWATCH_SLOT}."));
        assert!(
            flat.code_ranges.is_empty(),
            "no code chip around a lone swatch"
        );
        assert!(flat.colors[0].alone);
        assert_eq!(flat.colors[0].value, "oklch(0.711 0.183 22.5)");
    }

    #[test]
    fn a_color_inside_longer_code_keeps_its_text_after_a_swatch() {
        let flat = flatten_inline_with_swatches(
            &[code("color: #333; background: red")],
            &NoRichLinkTitles,
        );
        assert_eq!(
            flat.source,
            format!("color: {SWATCH_SLOT}#333; background: red")
        );
        assert_eq!(flat.code_ranges.len(), 1);
        assert_eq!(flat.code_ranges[0], 0..flat.source.len());
        let color = &flat.colors[0];
        assert!(!color.alone);
        assert_eq!(color.value, "#333");
        assert_eq!(
            &flat.source[color.range.clone()],
            format!("{SWATCH_SLOT}#333")
        );
        assert_eq!(&flat.source[color.slot.clone()], SWATCH_SLOT);
    }

    #[test]
    fn the_plain_flattener_keeps_colors_as_text() {
        let flat = flatten_inline(&[text("Coral (#FF6B6B)"), code("#333")], &NoRichLinkTitles);
        assert_eq!(flat.source, "Coral (#FF6B6B)#333");
        assert!(flat.colors.is_empty());
    }
}
