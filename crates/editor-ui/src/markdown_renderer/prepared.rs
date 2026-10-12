//! Per-document presentation prepared once when a body enters the cache.
//!
//! A frame used to rebuild every inline presentation (flattened `String`,
//! highlight `Vec`, links, bare-URL scan, citation split), copy every fence
//! into a fresh `SharedString`, and format every element id, for every
//! visible body. [`PreparedMarkdown`] holds all of that beside the parsed
//! document instead, in a tree that mirrors the blocks:
//!
//! - inline text as a [`SharedString`] with theme-independent style chains,
//!   so render only resolves colors, and memoizes the resolved runs until the
//!   theme's resolved styles change;
//! - fence display sources and validated token ranges;
//! - every element id and debug selector, formatted once per parent
//!   selector (see [`PreparedMarkdown::ids`]).
//!
//! Rich-link titles and favicons stay live: a leaf with openable links asks
//! the title source about each destination on every render, exactly as the
//! old flattener did (the probe is also what queues missing titles), and
//! only re-flattens when an answer differs from the one it last prepared.

use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{HighlightStyle, RenderImage, SharedString, px};

use crate::markdown::{Block, CodeFence, CodeTokenKind, MarkdownDocument, Span, TableAlignment};
use crate::markdown_cache::MarkdownWorkCounters;
use crate::selectable_text::{InlineSwatch, TextRunOverride};
use crate::theme::ArtisanTheme;

use super::code_copy::CODE_COPY_SELECTOR_SUFFIX;
use super::inline::{
    FlatInline, InlineChannels, InlineLayer, flatten_inline_with_swatches, resolve_chain,
    resolve_runs, split_trailing_citations,
};
use super::rich::{self, RichFence};
use super::{
    BlockScope, InlineLink, NoRichLinkTitles, RichLinkTitleSource, block_gaps, code_token_style,
    fence_display_source, item_marker, valid_code_range,
};

/// Index of one element id in a document's id table.
pub(super) type Slot = usize;

/// Slot of the Markdown root selector (`{selector}-markdown`).
pub(super) const ROOT_SLOT: Slot = 0;

/// Distinct parent selectors whose id tables one document keeps. A body
/// normally renders under one selector; a few identical short bodies (two
/// "Done." replies) share one document under different selectors.
const ID_TABLES_PER_DOCUMENT: usize = 4;

/// A parsed document plus everything its render needs, prepared once.
#[derive(Debug)]
pub(crate) struct PreparedMarkdown {
    document: Rc<MarkdownDocument>,
    root: PreparedBlocks,
    /// Element-id suffixes by slot, appended to the caller's selector.
    suffixes: Box<[String]>,
    /// Most recently used last.
    ids: RefCell<Vec<(SharedString, Rc<[SharedString]>)>>,
    counters: Rc<MarkdownWorkCounters>,
}

impl PreparedMarkdown {
    /// Prepares `document`, flattening every inline run once.
    pub(crate) fn new(document: Rc<MarkdownDocument>, counters: Rc<MarkdownWorkCounters>) -> Self {
        let mut slots = SlotTable::default();
        let root_slot = slots.push("-markdown".to_owned());
        debug_assert_eq!(
            root_slot, ROOT_SLOT,
            "the root selector takes the first slot"
        );
        let root = prepare_blocks(
            document.blocks(),
            "-markdown",
            BlockScope::Root,
            &mut slots,
            &counters,
        );
        Self {
            document,
            root,
            suffixes: slots.suffixes.into_boxed_slice(),
            ids: RefCell::new(Vec::new()),
            counters,
        }
    }

    /// The parsed document this presentation was prepared from.
    pub(crate) fn document(&self) -> &Rc<MarkdownDocument> {
        &self.document
    }

    pub(super) fn root(&self) -> &PreparedBlocks {
        &self.root
    }

    pub(super) fn counters(&self) -> &MarkdownWorkCounters {
        &self.counters
    }

    /// Returns this document's element ids under `selector`, formatting
    /// them only the first time the selector is seen.
    pub(super) fn ids(&self, selector: &SharedString) -> Rc<[SharedString]> {
        let mut tables = self.ids.borrow_mut();
        if let Some(index) = tables.iter().position(|(known, _)| known == selector) {
            let entry = tables.remove(index);
            let ids = Rc::clone(&entry.1);
            tables.push(entry);
            return ids;
        }
        let ids: Rc<[SharedString]> = self
            .suffixes
            .iter()
            .map(|suffix| SharedString::from(format!("{selector}{suffix}")))
            .collect();
        if tables.len() >= ID_TABLES_PER_DOCUMENT {
            tables.remove(0);
        }
        tables.push((selector.clone(), Rc::clone(&ids)));
        ids
    }
}

#[derive(Default)]
struct SlotTable {
    suffixes: Vec<String>,
}

impl SlotTable {
    fn push(&mut self, suffix: String) -> Slot {
        self.suffixes.push(suffix);
        self.suffixes.len() - 1
    }
}

/// One block sequence with its collapsed top gaps.
#[derive(Debug)]
pub(super) struct PreparedBlocks {
    pub(super) blocks: Vec<PreparedBlock>,
    pub(super) gaps: Vec<f32>,
}

/// One prepared block: its own selector plus kind-specific data.
#[derive(Debug)]
pub(super) struct PreparedBlock {
    pub(super) selector: Slot,
    pub(super) kind: PreparedKind,
}

#[derive(Debug)]
pub(super) enum PreparedKind {
    Heading { level: u8, leaf: PreparedLeaf },
    Paragraph { leaf: PreparedLeaf },
    Code(Box<PreparedCode>),
    Html { id: Slot, source: SharedString },
    List(PreparedList),
    Table(PreparedTable),
}

#[derive(Debug)]
pub(super) struct PreparedCode {
    pub(super) code: Slot,
    pub(super) text: Slot,
    pub(super) copy: Slot,
    pub(super) copy_state: Slot,
    pub(super) fence: PreparedFence,
}

#[derive(Debug)]
pub(super) struct PreparedList {
    pub(super) list: Slot,
    pub(super) ordered: bool,
    pub(super) items: Vec<PreparedItem>,
}

#[derive(Debug)]
pub(super) struct PreparedItem {
    pub(super) selector: Slot,
    pub(super) marker: SharedString,
    /// The leaf an item with no blocks still renders, so it keeps its row.
    pub(super) empty: Option<PreparedLeaf>,
    pub(super) blocks: PreparedBlocks,
}

#[derive(Debug)]
pub(super) struct PreparedTable {
    pub(super) table: Slot,
    /// One authored alignment per column.
    pub(super) alignments: Box<[TableAlignment]>,
    /// Header row first, then body rows; every row holds one cell per column.
    pub(super) rows: Vec<Vec<PreparedCell>>,
}

#[derive(Debug)]
pub(super) struct PreparedCell {
    pub(super) selector: Slot,
    pub(super) content: Slot,
    /// `None` where the authored row is shorter than the header.
    pub(super) leaf: Option<PreparedLeaf>,
}

fn prepare_blocks(
    blocks: &[Block],
    parent: &str,
    scope: BlockScope,
    slots: &mut SlotTable,
    counters: &MarkdownWorkCounters,
) -> PreparedBlocks {
    let prepared = blocks
        .iter()
        .enumerate()
        .map(|(index, block)| prepare_block(index, block, parent, slots, counters))
        .collect();
    PreparedBlocks {
        blocks: prepared,
        gaps: block_gaps(blocks, scope),
    }
}

fn prepare_block(
    index: usize,
    block: &Block,
    parent: &str,
    slots: &mut SlotTable,
    counters: &MarkdownWorkCounters,
) -> PreparedBlock {
    let own = format!("{parent}-block-{index}");
    let selector = slots.push(own.clone());
    let kind = match block {
        Block::Heading { level, spans, .. } => PreparedKind::Heading {
            level: *level,
            leaf: PreparedLeaf::new(spans, counters),
        },
        Block::Paragraph { spans, .. } => PreparedKind::Paragraph {
            leaf: PreparedLeaf::new(spans, counters),
        },
        Block::Code(fence) => {
            let code_suffix = format!("{own}-code");
            let copy_suffix = format!("{code_suffix}-{CODE_COPY_SELECTOR_SUFFIX}");
            PreparedKind::Code(Box::new(PreparedCode {
                code: slots.push(code_suffix.clone()),
                text: slots.push(format!("{code_suffix}-text")),
                copy_state: slots.push(format!("{copy_suffix}-state")),
                copy: slots.push(copy_suffix),
                fence: PreparedFence::new(fence),
            }))
        }
        Block::Html { source } => PreparedKind::Html {
            id: slots.push(format!("{own}-html")),
            source: SharedString::from(source.clone()),
        },
        Block::List {
            ordered,
            start,
            items,
            ..
        } => {
            let base = start.unwrap_or(1);
            let items = items
                .iter()
                .enumerate()
                .map(|(position, item)| {
                    let item_suffix = format!("{own}-item-{position}");
                    PreparedItem {
                        selector: slots.push(item_suffix.clone()),
                        marker: SharedString::from(item_marker(
                            *ordered, base, position, item.task,
                        )),
                        empty: item
                            .blocks
                            .is_empty()
                            .then(|| PreparedLeaf::new(&[], counters)),
                        blocks: prepare_blocks(
                            &item.blocks,
                            &item_suffix,
                            BlockScope::Item,
                            slots,
                            counters,
                        ),
                    }
                })
                .collect();
            PreparedKind::List(PreparedList {
                list: slots.push(format!("{own}-list")),
                ordered: *ordered,
                items,
            })
        }
        Block::Table(table) => {
            let table_suffix = format!("{own}-table");
            let columns = table.alignments.len();
            let rows = std::iter::once(&table.header)
                .chain(table.rows.iter())
                .enumerate()
                .map(|(row_index, cells)| {
                    (0..columns)
                        .map(|column| {
                            let cell_suffix = format!("{table_suffix}-r{row_index}-c{column}");
                            PreparedCell {
                                selector: slots.push(cell_suffix.clone()),
                                content: slots.push(format!("{cell_suffix}-content")),
                                leaf: cells
                                    .get(column)
                                    .map(|cell| PreparedLeaf::new(&cell.spans, counters)),
                            }
                        })
                        .collect()
                })
                .collect();
            PreparedKind::Table(PreparedTable {
                table: slots.push(table_suffix),
                alignments: table.alignments.clone().into_boxed_slice(),
                rows,
            })
        }
    };
    PreparedBlock { selector, kind }
}

/// One openable destination the title source is consulted about.
#[derive(Debug)]
struct Probe {
    destination: String,
    /// Only HTTP(S) destinations resolve titles; `mailto:` only asks for a
    /// favicon.
    rich: bool,
}

/// What the title source answered about one destination this frame.
struct ProbeAnswer {
    title: Option<SharedString>,
    favicon: Option<Arc<RenderImage>>,
}

/// The part of an answer that shapes the flattened presentation.
#[derive(Debug, PartialEq)]
struct ProbeKey {
    title: Option<SharedString>,
    has_favicon: bool,
}

/// The title answers a leaf was last flattened for, with that presentation.
type TitledPresentation = (Box<[ProbeKey]>, Rc<LeafPresentation>);

/// Resolved highlight runs plus inline-code run overrides.
type ResolvedRuns = (Rc<[(Range<usize>, HighlightStyle)]>, Rc<[TextRunOverride]>);

/// One inline leaf (paragraph, heading, table cell, or empty list item).
#[derive(Debug)]
pub(super) struct PreparedLeaf {
    /// Presentation with no titles and no favicons: what every leaf without
    /// resolved link metadata renders.
    base: Rc<LeafPresentation>,
    probes: Box<[Probe]>,
    /// Presentation for the most recent non-empty title answers.
    titled: RefCell<Option<TitledPresentation>>,
}

impl PreparedLeaf {
    fn new(spans: &[Span], counters: &MarkdownWorkCounters) -> Self {
        counters.record_inline_flatten();
        let flat = flatten_inline_with_swatches(spans, &NoRichLinkTitles);
        let probes = flat
            .probes
            .iter()
            .map(|destination| Probe {
                rich: super::is_rich_link_destination(destination),
                destination: destination.clone(),
            })
            .collect();
        Self {
            base: Rc::new(LeafPresentation::from_flat(flat)),
            probes,
            titled: RefCell::new(None),
        }
    }

    /// Returns this frame's presentation plus favicon images by offset.
    ///
    /// `spans` are the leaf's own spans from the cached document; they are
    /// only read when the title answers changed since the last flatten.
    pub(super) fn presentation(
        &self,
        spans: &[Span],
        titles: &dyn RichLinkTitleSource,
        counters: &MarkdownWorkCounters,
    ) -> (Rc<LeafPresentation>, Vec<(usize, Arc<RenderImage>)>) {
        if self.probes.is_empty() {
            return (Rc::clone(&self.base), Vec::new());
        }
        // Every frame asks about every destination, title before favicon,
        // as the flattener always has: the title probe is what queues a
        // missing title for resolution.
        let answers = self
            .probes
            .iter()
            .map(|probe| ProbeAnswer {
                title: probe
                    .rich
                    .then(|| titles.resolved_title(&probe.destination))
                    .flatten(),
                favicon: titles.favicon(&probe.destination),
            })
            .collect::<Vec<_>>();
        if answers
            .iter()
            .all(|answer| answer.title.is_none() && answer.favicon.is_none())
        {
            return (Rc::clone(&self.base), Vec::new());
        }
        let keys = answers
            .iter()
            .map(|answer| ProbeKey {
                title: answer.title.clone(),
                has_favicon: answer.favicon.is_some(),
            })
            .collect::<Box<[_]>>();
        let presentation = {
            let mut memo = self.titled.borrow_mut();
            match memo.as_ref() {
                Some((known, presentation)) if *known == keys => Rc::clone(presentation),
                _ => {
                    counters.record_inline_flatten();
                    let replay = ReplayTitles {
                        probes: &self.probes,
                        answers: &answers,
                    };
                    let presentation = Rc::new(LeafPresentation::from_flat(
                        flatten_inline_with_swatches(spans, &replay),
                    ));
                    *memo = Some((keys, Rc::clone(&presentation)));
                    presentation
                }
            }
        };
        let icons = presentation
            .icon_offsets
            .iter()
            .filter_map(|(offset, destination)| {
                answer_for(&self.probes, &answers, destination)
                    .and_then(|answer| answer.favicon.clone())
                    .map(|image| (*offset, image))
            })
            .collect();
        (presentation, icons)
    }
}

fn answer_for<'a>(
    probes: &[Probe],
    answers: &'a [ProbeAnswer],
    destination: &str,
) -> Option<&'a ProbeAnswer> {
    probes
        .iter()
        .position(|probe| probe.destination == destination)
        .and_then(|index| answers.get(index))
}

/// Serves this frame's recorded answers to a re-flatten, so the live title
/// source is asked exactly once per destination per frame.
struct ReplayTitles<'a> {
    probes: &'a [Probe],
    answers: &'a [ProbeAnswer],
}

impl RichLinkTitleSource for ReplayTitles<'_> {
    fn favicon(&self, destination: &str) -> Option<Arc<RenderImage>> {
        answer_for(self.probes, self.answers, destination).and_then(|answer| answer.favicon.clone())
    }

    fn resolved_title(&self, destination: &str) -> Option<SharedString> {
        answer_for(self.probes, self.answers, destination).and_then(|answer| answer.title.clone())
    }
}

/// One leaf's flattened presentation after trailing citations split off.
#[derive(Debug)]
pub(super) struct LeafPresentation {
    /// Body text every range addresses.
    pub(super) text: SharedString,
    runs: Box<[(Range<usize>, usize)]>,
    chains: Box<[Box<[InlineLayer]>]>,
    pub(super) link_ranges: Rc<[Range<usize>]>,
    pub(super) link_destinations: Rc<[SharedString]>,
    /// Color values the text writes, outside links: right-clicking one
    /// offers it in every notation. Each range covers its swatch, plus the
    /// value when the value shows.
    pub(super) color_ranges: Rc<[Range<usize>]>,
    /// The values of `color_ranges`, as written.
    pub(super) color_values: Rc<[SharedString]>,
    /// The squares painted in the swatch slots.
    pub(super) swatches: Rc<[InlineSwatch]>,
    /// What each swatch slot copies as in a text selection.
    pub(super) copies: Rc<[(Range<usize>, SharedString)]>,
    code_ranges: Box<[Range<usize>]>,
    pub(super) icon_offsets: Box<[(usize, String)]>,
    /// Trailing citation attributions, rendered as pills after the text.
    pub(super) citations: Box<[InlineLink]>,
    resolved: RefCell<Option<ResolvedLeaf>>,
}

#[derive(Debug)]
struct ResolvedLeaf {
    styles: Box<[HighlightStyle]>,
    mono_family: &'static str,
    highlights: Rc<[(Range<usize>, HighlightStyle)]>,
    overrides: Rc<[TextRunOverride]>,
}

impl LeafPresentation {
    fn from_flat(mut flat: FlatInline) -> Self {
        let citations = split_trailing_citations(InlineChannels {
            source: &mut flat.source,
            runs: &mut flat.runs,
            links: &mut flat.links,
            citation_links: &mut flat.citation_links,
            code_ranges: &mut flat.code_ranges,
            icon_offsets: &mut flat.icon_offsets,
        });
        // Colors past a stripped trailing citation left with it.
        let end = flat.source.len();
        let colors: Vec<_> = flat
            .colors
            .into_iter()
            .filter(|color| color.range.end <= end)
            .collect();
        let swatches = colors
            .iter()
            .filter_map(|color| {
                Some(InlineSwatch {
                    offset: color.slot.start + '\u{2060}'.len_utf8(),
                    color: super::rich::color_paint(&color.value)?,
                    alone: color.alone,
                })
            })
            .collect();
        // A lone swatch copies as its value; one beside its value as nothing.
        let copies = colors
            .iter()
            .map(|color| {
                let copy = if color.alone {
                    SharedString::from(color.value.clone())
                } else {
                    SharedString::default()
                };
                (color.slot.clone(), copy)
            })
            .collect();
        Self {
            text: SharedString::from(flat.source),
            runs: flat.runs.into_boxed_slice(),
            color_ranges: colors.iter().map(|color| color.range.clone()).collect(),
            color_values: colors
                .into_iter()
                .map(|color| SharedString::from(color.value))
                .collect(),
            swatches,
            copies,
            chains: flat.chains.into_iter().map(Vec::into_boxed_slice).collect(),
            link_ranges: flat.links.iter().map(|link| link.range.clone()).collect(),
            link_destinations: flat
                .links
                .into_iter()
                .map(|link| SharedString::from(link.destination))
                .collect(),
            code_ranges: flat.code_ranges.into_boxed_slice(),
            icon_offsets: flat.icon_offsets.into_boxed_slice(),
            citations: citations.into_boxed_slice(),
            resolved: RefCell::new(None),
        }
    }

    /// Resolved highlight runs and inline-code run overrides for `theme`.
    ///
    /// Reused while every chain resolves to the style it resolved to last
    /// time, so an unchanged theme resolves a handful of chain styles and
    /// allocates nothing.
    pub(super) fn resolved(&self, theme: &ArtisanTheme) -> ResolvedRuns {
        let mono_family = theme.typography.mono.family;
        let mut memo = self.resolved.borrow_mut();
        if let Some(resolved) = memo.as_ref()
            && resolved.mono_family == mono_family
            && resolved.styles.len() == self.chains.len()
            && self
                .chains
                .iter()
                .zip(resolved.styles.iter())
                .all(|(chain, style)| resolve_chain(chain, theme) == *style)
        {
            return (
                Rc::clone(&resolved.highlights),
                Rc::clone(&resolved.overrides),
            );
        }
        let styles = self
            .chains
            .iter()
            .map(|chain| resolve_chain(chain, theme))
            .collect::<Box<[_]>>();
        let highlights: Rc<[_]> = resolve_runs(&self.runs, &styles).into();
        // Inline code rides the frozen text-run contract: mono family plus
        // zero tracking per code range. Weight (400) and muted color already
        // ride the highlight runs.
        let overrides: Rc<[_]> = self
            .code_ranges
            .iter()
            .map(|range| TextRunOverride {
                range: range.clone(),
                font_family: Some(SharedString::from(mono_family)),
                letter_spacing: Some(px(0.0)),
            })
            .collect();
        *memo = Some(ResolvedLeaf {
            styles,
            mono_family,
            highlights: Rc::clone(&highlights),
            overrides: Rc::clone(&overrides),
        });
        (highlights, overrides)
    }
}

/// Number of [`CodeTokenKind`] variants, for the resolved kind palette.
const CODE_TOKEN_KINDS: usize = 7;

fn kind_index(kind: CodeTokenKind) -> usize {
    match kind {
        CodeTokenKind::Comment => 0,
        CodeTokenKind::Str => 1,
        CodeTokenKind::Number => 2,
        CodeTokenKind::Keyword => 3,
        CodeTokenKind::Type => 4,
        CodeTokenKind::Function => 5,
        CodeTokenKind::Plain => 6,
    }
}

const ALL_KINDS: [CodeTokenKind; CODE_TOKEN_KINDS] = [
    CodeTokenKind::Comment,
    CodeTokenKind::Str,
    CodeTokenKind::Number,
    CodeTokenKind::Keyword,
    CodeTokenKind::Type,
    CodeTokenKind::Function,
    CodeTokenKind::Plain,
];

/// One fence's display source and validated token ranges.
#[derive(Debug)]
pub(super) struct PreparedFence {
    /// The fence body as displayed (final line terminator dropped).
    pub(super) source: SharedString,
    /// Whether the displayed body spans more than one line.
    pub(super) multiline: bool,
    /// Whether the fence is a closed `mermaid` block, drawn as a diagram
    /// when its source renders.
    pub(super) diagram: bool,
    /// The rich block an `artisan-*` fence draws, parsed once here.
    pub(super) rich: Option<RichFence>,
    /// The body as drawn: `source` with a swatch slot before each color
    /// value, the way an editor decorates CSS colors. Token ranges address
    /// this text; the copy action copies `source`.
    pub(super) shown: SharedString,
    /// The fence's color values, their swatches, and what each slot copies
    /// as, addressing `shown`.
    pub(super) colors: FenceColors,
    tokens: Box<[(Range<usize>, CodeTokenKind)]>,
    resolved: RefCell<Option<ResolvedFence>>,
}

#[derive(Debug)]
struct ResolvedFence {
    palette: [HighlightStyle; CODE_TOKEN_KINDS],
    highlights: Rc<[(Range<usize>, HighlightStyle)]>,
}

/// `display` with a swatch slot before each of `colors`, the fence colors
/// for the drawn text, and `tokens` moved past the slots inserted before
/// them. A token starting exactly at a color keeps its start, so the slot
/// takes the token's style; one ending there keeps its end.
fn decorate_colors(
    display: &str,
    colors: &[Range<usize>],
    tokens: Vec<(Range<usize>, CodeTokenKind)>,
) -> (
    SharedString,
    FenceColors,
    Vec<(Range<usize>, CodeTokenKind)>,
) {
    if colors.is_empty() {
        return (
            SharedString::from(display.to_owned()),
            FenceColors::default(),
            tokens,
        );
    }
    let slot = super::inline::SWATCH_SLOT;
    let shift = |offset: usize| {
        offset + slot.len() * colors.iter().filter(|color| color.start < offset).count()
    };
    let mut shown = String::with_capacity(display.len() + slot.len() * colors.len());
    let mut ranges = Vec::new();
    let mut swatches = Vec::new();
    let mut copies = Vec::new();
    let mut values = Vec::new();
    let mut cursor = 0;
    for color in colors {
        shown.push_str(&display[cursor..color.start]);
        let slot_start = shown.len();
        shown.push_str(slot);
        let slot_range = slot_start..shown.len();
        shown.push_str(&display[color.clone()]);
        ranges.push(slot_start..shown.len());
        let value = &display[color.clone()];
        if let Some(paint) = rich::color_paint(value) {
            swatches.push(InlineSwatch {
                offset: slot_start + '\u{2060}'.len_utf8(),
                color: paint,
                alone: false,
            });
        }
        copies.push((slot_range, SharedString::default()));
        values.push(SharedString::from(value.to_owned()));
        cursor = color.end;
    }
    shown.push_str(&display[cursor..]);
    let tokens = tokens
        .into_iter()
        .map(|(range, kind)| (shift(range.start)..shift(range.end), kind))
        .collect();
    let colors = FenceColors {
        ranges: ranges.into(),
        values: values.into(),
        swatches: swatches.into(),
        copies: copies.into(),
    };
    (SharedString::from(shown), colors, tokens)
}

/// Color values in a fence's drawn text, ready for the selectable text.
#[derive(Debug, Default)]
pub(super) struct FenceColors {
    pub(super) ranges: Rc<[Range<usize>]>,
    pub(super) values: Rc<[SharedString]>,
    pub(super) swatches: Rc<[InlineSwatch]>,
    pub(super) copies: Rc<[(Range<usize>, SharedString)]>,
}

impl PreparedFence {
    fn new(fence: &CodeFence) -> Self {
        let display = fence_display_source(&fence.source);
        let tokens: Vec<_> = fence
            .tokens
            .as_deref()
            .unwrap_or_default()
            .iter()
            .filter_map(|token| valid_code_range(token, display))
            .collect();
        let rich = rich::prepare(fence.language.as_deref(), display, fence.closed);
        let diagram = fence.closed && fence.language.as_deref() == Some("mermaid");
        // Rich blocks and diagrams draw something else; plain code shows
        // its colors.
        let starts = if rich.is_none() && !diagram {
            let whole = 0..display.len();
            rich::color_ranges(display, std::slice::from_ref(&whole))
        } else {
            Vec::new()
        };
        let (shown, colors, tokens) = decorate_colors(display, &starts, tokens);
        Self {
            multiline: display.contains('\n'),
            diagram,
            rich,
            source: SharedString::from(display),
            shown,
            colors,
            tokens: tokens.into_boxed_slice(),
            resolved: RefCell::new(None),
        }
    }

    /// Token highlight runs for `theme`, reused while the kind palette
    /// resolves unchanged.
    pub(super) fn highlights(&self, theme: &ArtisanTheme) -> Rc<[(Range<usize>, HighlightStyle)]> {
        let palette = ALL_KINDS.map(|kind| code_token_style(theme, kind));
        let mut memo = self.resolved.borrow_mut();
        if let Some(resolved) = memo.as_ref()
            && resolved.palette == palette
        {
            return Rc::clone(&resolved.highlights);
        }
        let highlights: Rc<[_]> = self
            .tokens
            .iter()
            .map(|(range, kind)| (range.clone(), palette[kind_index(*kind)]))
            .collect();
        *memo = Some(ResolvedFence {
            palette,
            highlights: Rc::clone(&highlights),
        });
        highlights
    }
}

#[cfg(test)]
mod fence_color_tests {
    use super::{CodeTokenKind, decorate_colors};
    use crate::markdown_renderer::inline::SWATCH_SLOT;

    #[test]
    fn a_fence_shows_a_swatch_before_each_color_and_tokens_follow() {
        let css = "a { color: #fff; border: 1px solid rgb(0 0 0); }";
        let hex = css.find("#fff").expect("hex");
        let rgb = css.find("rgb(").expect("rgb");
        let tokens = vec![
            (hex..hex + 4, CodeTokenKind::Number),
            (rgb - 6..rgb - 1, CodeTokenKind::Keyword),
        ];
        let (shown, colors, tokens) = decorate_colors(css, &[hex..hex + 4, rgb..rgb + 10], tokens);
        assert_eq!(
            shown.as_ref(),
            format!("a {{ color: {SWATCH_SLOT}#fff; border: 1px solid {SWATCH_SLOT}rgb(0 0 0); }}")
        );
        // The hex token now starts at its slot and still ends after `#fff`.
        let (first, _) = &tokens[0];
        assert_eq!(&shown[first.clone()], format!("{SWATCH_SLOT}#fff"));
        // `solid` sits before the second slot, past the first.
        let (second, _) = &tokens[1];
        assert_eq!(&shown[second.clone()], "solid");
        assert_eq!(colors.values.len(), 2);
        assert_eq!(colors.swatches.len(), 2);
        assert!(colors.copies.iter().all(|(_, copy)| copy.is_empty()));
    }

    #[test]
    fn a_fence_without_colors_is_untouched() {
        let (shown, colors, tokens) =
            decorate_colors("let x = 1;", &[], vec![(8..9, CodeTokenKind::Number)]);
        assert_eq!(shown.as_ref(), "let x = 1;");
        assert!(colors.ranges.is_empty());
        assert_eq!(tokens[0].0, 8..9);
    }
}
