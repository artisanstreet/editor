//! Immutable semantic text/fence data and presentation topology.
//! The worker prepares normalized payloads; publication installs UI-owned
//! style memos and stable numeric ordinals. The parsed AST is not retained.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{HighlightStyle, SharedString, px};

use crate::markdown::{Block, CodeFence, CodeTokenKind, MarkdownDocument, TableAlignment};
use crate::selectable_text::{InlineSwatch, SelectionLeaf, SelectionSource, TextRunOverride};
use crate::theme::{ArtisanTheme, Oklch};

use super::inline::{
    FlatInline, InlineLayer, flatten_inline_with_swatches, resolve_chain, resolve_runs,
};
use super::rich::{self, RichFence};
use super::{
    BlockScope, block_gaps, code_token_style, fence_display_source, item_marker, valid_code_range,
};

/// Index of one element id in a document's id table.
pub(super) type Slot = usize;

/// Slot of the Markdown root selector (`{selector}-markdown`).
pub(super) const ROOT_SLOT: Slot = 0;

/// One normalized content revision and its UI style memos.
#[derive(Debug)]
pub(crate) struct PreparedMarkdown {
    root: PreparedBlocks,
    flow: Vec<FlowRow>,
}

/// CPU normalization returned by the bounded worker. UI publication moves
/// owned arrays into presentation references and installs empty style memos.
pub(super) struct PreparedParts {
    inline: VecDeque<FlatInline>,
    fences: VecDeque<PreparedFenceData>,
}
impl PreparedParts {
    pub(super) fn resident_bytes(&self) -> usize {
        self.inline.capacity() * std::mem::size_of::<FlatInline>()
            + self.fences.capacity() * std::mem::size_of::<PreparedFenceData>()
            + self
                .inline
                .iter()
                .map(|flat| {
                    flat.source.capacity()
                        + rich::budget::vector(&flat.runs)
                        + rich::budget::vector(&flat.chains)
                        + flat.chains.iter().map(rich::budget::vector).sum::<usize>()
                        + rich::budget::vector(&flat.links)
                        + flat
                            .links
                            .iter()
                            .map(|link| link.destination.capacity())
                            .sum::<usize>()
                        + rich::budget::vector(&flat.code_ranges)
                        + rich::budget::vector(&flat.colors)
                        + flat
                            .colors
                            .iter()
                            .map(|color| color.value.capacity())
                            .sum::<usize>()
                })
                .sum::<usize>()
            + self
                .fences
                .iter()
                .map(PreparedFenceData::heap_bytes)
                .sum::<usize>()
    }
    pub(super) fn new(document: &MarkdownDocument, renderer: Option<&gpui::SvgRenderer>) -> Self {
        fn visit(
            blocks: &[Block],
            result: &mut PreparedParts,
            renderer: Option<&gpui::SvgRenderer>,
        ) {
            for block in blocks {
                match block {
                    Block::Heading { spans, .. } | Block::Paragraph { spans, .. } => {
                        result.inline.push_back(flatten_inline_with_swatches(spans))
                    }
                    Block::Code(fence) => result
                        .fences
                        .push_back(PreparedFenceData::new(fence, renderer)),
                    Block::List { items, .. } => {
                        for item in items {
                            if item.blocks.is_empty() {
                                result.inline.push_back(FlatInline::default());
                            }
                            visit(&item.blocks, result, renderer);
                        }
                    }
                    Block::Table(table) => {
                        for cells in std::iter::once(&table.header).chain(table.rows.iter()) {
                            for cell in cells.iter().take(table.alignments.len()) {
                                result
                                    .inline
                                    .push_back(flatten_inline_with_swatches(&cell.spans));
                            }
                        }
                    }
                    Block::Html { .. } => {}
                }
            }
        }
        let mut result = Self {
            inline: VecDeque::new(),
            fences: VecDeque::new(),
        };
        visit(document.blocks(), &mut result, renderer);
        result
    }
}

impl PreparedMarkdown {
    #[cfg(test)]
    fn new(document: MarkdownDocument) -> Self {
        let parts = PreparedParts::new(&document, None);
        Self::from_indexed_parts(&document, parts, None)
    }
    pub(super) fn from_indexed_parts(
        document: &MarkdownDocument,
        mut parts: PreparedParts,
        style: Option<super::async_preparation::IndexedStyle>,
    ) -> Self {
        let mut slots = SlotTable::default();
        let root_slot = slots.push();
        debug_assert_eq!(
            root_slot, ROOT_SLOT,
            "the root selector takes the first slot"
        );
        let mut root = prepare_blocks(document.blocks(), BlockScope::Root, &mut slots, &mut parts);
        if let Some(style) = &style
            && let Some(header) = style.table_header
            && let Some(block) = root.blocks.first_mut().and_then(Rc::get_mut)
            && let PreparedKind::Table(table) = &mut block.kind
        {
            table.header_rows = usize::from(header);
            table.trailing_border = style.table_continues;
        }
        let mut flow = flatten_flow(&root);
        if let Some(style) = style
            && let Some(row) = flow.first_mut()
        {
            row.indent = style.context.lists.len();
            for (indent, level) in style.context.lists.iter().enumerate() {
                if level.first {
                    row.markers.push((
                        indent,
                        item_marker(
                            level.number.is_some(),
                            level.number.unwrap_or(1),
                            0,
                            level.task,
                        )
                        .into(),
                        level.number.is_some(),
                    ));
                }
            }
            row.gap = style.context.spacing.iter().map(indexed_gap).sum();
        }
        Self { root, flow }
    }

    /// Actual owned container capacities and retained logical string payloads.
    /// Theme memos reserve their maximum run arrays before lazy resolution.
    pub(crate) fn resident_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.root.heap_bytes()
            + rich::budget::vector(&self.flow)
            + self
                .flow
                .iter()
                .map(|row| {
                    rich::budget::vector(&row.markers)
                        + row
                            .markers
                            .iter()
                            .map(|(_, marker, _)| marker.len())
                            .sum::<usize>()
                })
                .sum::<usize>()
    }

    pub(crate) fn block_count(&self) -> usize {
        self.flow.len()
    }

    pub(super) fn selection_source(
        &self,
        scope: &SharedString,
        prefix: &[u64],
    ) -> Rc<SelectionSource> {
        let mut leaves = Vec::new();
        collect_selection(&self.root, prefix, &mut leaves, "\n\n");
        SelectionSource::new(scope.clone(), prefix.to_vec(), leaves)
    }

    pub(super) fn release_images(&self, cx: &mut gpui::App) {
        for row in &self.flow {
            if let FlowKind::Block(block) = &row.kind
                && let PreparedKind::Code(code) = &block.kind
                && let Some(diagram) = &code.fence.diagram
            {
                let image = diagram.image.clone();
                cx.defer(move |cx| {
                    cx.drop_image(image, None);
                    // Other owners may still draw this image in cached scenes.
                    cx.refresh_windows();
                });
            }
        }
    }

    pub(super) fn flow(&self) -> &[FlowRow] {
        &self.flow
    }
}

fn indexed_gap(spacing: &crate::markdown::LeafSpacing) -> f32 {
    use crate::markdown::{BlockShape, LeafSpacing};
    let shape = |shape: BlockShape| match shape {
        BlockShape::Paragraph => Block::Paragraph {
            spans: Vec::new(),
            range: 0..0,
        },
        BlockShape::Heading(level) => Block::Heading {
            level,
            spans: Vec::new(),
            range: 0..0,
        },
        BlockShape::Code => Block::Code(crate::markdown::CodeFence {
            language: None,
            closed: true,
            source: String::new(),
            tokens: None,
            diagram: None,
        }),
        BlockShape::Html => Block::Html {
            source: String::new(),
        },
        BlockShape::List => Block::List {
            ordered: false,
            start: None,
            items: Vec::new(),
            range: 0..0,
        },
        BlockShape::Table => Block::Table(crate::markdown::Table {
            alignments: Vec::new(),
            header: Vec::new(),
            rows: Vec::new(),
            range: 0..0,
        }),
    };
    match spacing {
        LeafSpacing::Item => crate::theme::ProseTypography::ITEM_GAP_PX,
        LeafSpacing::Sibling { previous: None, .. } => 0.0,
        LeafSpacing::Sibling {
            previous: Some(previous),
            current,
            in_item,
        } => {
            let scope = if *in_item {
                BlockScope::Item
            } else {
                BlockScope::Root
            };
            let bottom = super::block_margins(&shape(*previous), scope).1;
            if matches!(previous, BlockShape::Heading(2..=4)) {
                bottom
            } else {
                bottom.max(super::block_margins(&shape(*current), scope).0)
            }
        }
    }
}

fn collect_selection(
    blocks: &PreparedBlocks,
    prefix: &[u64],
    output: &mut Vec<SelectionLeaf>,
    separator: &'static str,
) {
    let leaf = |slot: Slot,
                presentation: &LeafPresentation,
                separator,
                output: &mut Vec<SelectionLeaf>| {
        let mut order = prefix.to_vec();
        order.push(slot as u64);
        output.push(SelectionLeaf {
            order,
            text: presentation.text.clone(),
            separator,
            copies: presentation.copies.clone(),
        });
    };
    for block in &blocks.blocks {
        match &block.kind {
            PreparedKind::Heading { leaf: content, .. }
            | PreparedKind::Paragraph { leaf: content } => {
                leaf(block.selector, &content.base, separator, output)
            }
            PreparedKind::Html { id, source } => {
                let mut order = prefix.to_vec();
                order.push(*id as u64);
                output.push(SelectionLeaf {
                    order,
                    text: source.clone(),
                    separator: "\n",
                    copies: Rc::from([]),
                });
            }
            PreparedKind::Code(code) => {
                // Rich controls have their own interaction; ordinary code participates in prose selection.
                if code.fence.rich.is_none() && code.fence.diagram.is_none() {
                    for (line, (range, text)) in code.fence.lines.iter().enumerate() {
                        let mut order = prefix.to_vec();
                        order.push(code.text as u64);
                        order.push(line as u64);
                        let copies = code
                            .fence
                            .colors
                            .copies
                            .iter()
                            .filter(|&(copy, _text)| {
                                copy.start >= range.start && copy.end <= range.end
                            })
                            .map(|(copy, text)| {
                                (
                                    copy.start - range.start..copy.end - range.start,
                                    text.clone(),
                                )
                            })
                            .collect();
                        output.push(SelectionLeaf {
                            order,
                            text: text.clone(),
                            separator: "\n",
                            copies,
                        });
                    }
                }
            }
            PreparedKind::List(list) => {
                for item in &list.items {
                    if let Some(content) = &item.empty {
                        leaf(item.selector, &content.base, "\n", output);
                    }
                    collect_selection(&item.blocks, prefix, output, "\n");
                }
            }
            PreparedKind::Table(table) => {
                for row in &table.rows {
                    for (column, cell) in row.iter().enumerate() {
                        if let Some(content) = &cell.leaf {
                            leaf(
                                cell.selector,
                                &content.base,
                                if column == 0 { "\n" } else { "\t" },
                                output,
                            );
                        }
                    }
                }
            }
        }
    }
}

/// One semantic row in the outer transcript list. Nested containers never
/// force their descendants to construct in one visible row.
#[derive(Debug)]
pub(super) struct FlowRow {
    pub(super) kind: FlowKind,
    pub(super) indent: usize,
    pub(super) markers: Vec<(usize, SharedString, bool)>,
    pub(super) gap: f32,
}
#[derive(Debug)]
pub(super) enum FlowKind {
    Block(Rc<PreparedBlock>),
    TableRow {
        block: Rc<PreparedBlock>,
        row: usize,
    },
    Empty {
        leaf: PreparedLeaf,
        slot: Slot,
    },
}
fn flatten_flow(root: &PreparedBlocks) -> Vec<FlowRow> {
    fn visit(blocks: &PreparedBlocks, indent: usize, output: &mut Vec<FlowRow>) {
        for (index, block) in blocks.blocks.iter().enumerate() {
            let start = output.len();
            match &block.kind {
                PreparedKind::List(list) => {
                    for (item_index, item) in list.items.iter().enumerate() {
                        let first = output.len();
                        if let Some(leaf) = &item.empty {
                            output.push(FlowRow {
                                kind: FlowKind::Empty {
                                    leaf: leaf.clone(),
                                    slot: item.selector,
                                },
                                indent: indent + 1,
                                markers: Vec::new(),
                                gap: 0.0,
                            });
                        }
                        visit(&item.blocks, indent + 1, output);
                        if let Some(row) = output.get_mut(first) {
                            row.markers
                                .push((indent, item.marker.clone(), list.ordered));
                            if item_index > 0 {
                                row.gap += crate::theme::ProseTypography::ITEM_GAP_PX;
                            }
                        }
                    }
                }
                PreparedKind::Table(table) => {
                    for row in 0..table.rows.len() {
                        output.push(FlowRow {
                            kind: FlowKind::TableRow {
                                block: block.clone(),
                                row,
                            },
                            indent,
                            markers: Vec::new(),
                            gap: 0.0,
                        });
                    }
                }
                _ => output.push(FlowRow {
                    kind: FlowKind::Block(block.clone()),
                    indent,
                    markers: Vec::new(),
                    gap: 0.0,
                }),
            }
            if let Some(row) = output.get_mut(start) {
                row.gap += blocks.gaps.get(index).copied().unwrap_or_default();
            }
        }
    }
    let mut rows = Vec::new();
    visit(root, 0, &mut rows);
    rows
}

#[derive(Default)]
struct SlotTable {
    count: usize,
}

impl SlotTable {
    fn push(&mut self) -> Slot {
        let slot = self.count;
        self.count += 1;
        slot
    }
}

/// One block sequence with its collapsed top gaps.
#[derive(Debug)]
pub(super) struct PreparedBlocks {
    pub(super) blocks: Vec<Rc<PreparedBlock>>,
    pub(super) gaps: Vec<f32>,
}

impl PreparedBlocks {
    fn heap_bytes(&self) -> usize {
        rich::budget::vector(&self.blocks)
            + rich::budget::vector(&self.gaps)
            + self
                .blocks
                .iter()
                .map(|block| std::mem::size_of::<PreparedBlock>() + block.heap_bytes())
                .sum::<usize>()
    }
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
    pub(super) header_rows: usize,
    pub(super) trailing_border: bool,
    /// One authored alignment per column.
    pub(super) alignments: Box<[TableAlignment]>,
    /// Header row first, then body rows; every row holds one cell per column.
    pub(super) rows: Vec<Vec<PreparedCell>>,
}

#[derive(Debug)]
pub(super) struct PreparedCell {
    pub(super) selector: Slot,
    /// `None` where the authored row is shorter than the header.
    pub(super) leaf: Option<PreparedLeaf>,
}

fn prepare_blocks(
    blocks: &[Block],
    scope: BlockScope,
    slots: &mut SlotTable,
    parts: &mut PreparedParts,
) -> PreparedBlocks {
    let prepared = blocks
        .iter()
        .map(|block| prepare_block(block, slots, parts))
        .collect();
    PreparedBlocks {
        blocks: prepared,
        gaps: block_gaps(blocks, scope),
    }
}

fn prepare_block(
    block: &Block,
    slots: &mut SlotTable,
    parts: &mut PreparedParts,
) -> Rc<PreparedBlock> {
    let selector = slots.push();
    let kind = match block {
        Block::Heading { level, .. } => PreparedKind::Heading {
            level: *level,
            leaf: PreparedLeaf::new(parts),
        },
        Block::Paragraph { .. } => PreparedKind::Paragraph {
            leaf: PreparedLeaf::new(parts),
        },
        Block::Code(_) => PreparedKind::Code(Box::new(PreparedCode {
            code: slots.push(),
            text: slots.push(),
            copy_state: slots.push(),
            copy: slots.push(),
            fence: PreparedFence::new(parts.fences.pop_front().expect("normalized code fence")),
        })),
        Block::Html { source } => PreparedKind::Html {
            id: slots.push(),
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
                .map(|(position, item)| PreparedItem {
                    selector: slots.push(),
                    marker: SharedString::from(item_marker(*ordered, base, position, item.task)),
                    empty: item.blocks.is_empty().then(|| PreparedLeaf::new(parts)),
                    blocks: prepare_blocks(&item.blocks, BlockScope::Item, slots, parts),
                })
                .collect();
            PreparedKind::List(PreparedList {
                ordered: *ordered,
                items,
            })
        }
        Block::Table(table) => {
            let columns = table.alignments.len();
            let rows = std::iter::once(&table.header)
                .chain(table.rows.iter())
                .map(|cells| {
                    (0..columns)
                        .map(|column| PreparedCell {
                            selector: slots.push(),
                            leaf: cells.get(column).map(|_| PreparedLeaf::new(parts)),
                        })
                        .collect()
                })
                .collect();
            PreparedKind::Table(PreparedTable {
                header_rows: 1,
                trailing_border: false,
                alignments: table.alignments.clone().into_boxed_slice(),
                rows,
            })
        }
    };
    Rc::new(PreparedBlock { selector, kind })
}

impl PreparedBlock {
    fn heap_bytes(&self) -> usize {
        match &self.kind {
            PreparedKind::Heading { leaf, .. } | PreparedKind::Paragraph { leaf } => {
                leaf.resident_bytes()
            }
            PreparedKind::Html { source, .. } => source.len(),
            PreparedKind::Code(code) => {
                std::mem::size_of::<PreparedCode>() + code.fence.heap_bytes()
            }
            PreparedKind::List(list) => {
                rich::budget::vector(&list.items)
                    + list
                        .items
                        .iter()
                        .map(|item| {
                            item.marker.len()
                                + item.empty.as_ref().map_or(0, PreparedLeaf::resident_bytes)
                                + item.blocks.heap_bytes()
                        })
                        .sum::<usize>()
            }
            PreparedKind::Table(table) => {
                std::mem::size_of_val(table.alignments.as_ref())
                    + rich::budget::vector(&table.rows)
                    + table
                        .rows
                        .iter()
                        .map(|row| {
                            rich::budget::vector(row)
                                + row
                                    .iter()
                                    .filter_map(|cell| cell.leaf.as_ref())
                                    .map(PreparedLeaf::resident_bytes)
                                    .sum::<usize>()
                        })
                        .sum::<usize>()
            }
        }
    }
}

/// Resolved highlight runs plus inline-code run overrides.
type ResolvedRuns = (Rc<[(Range<usize>, HighlightStyle)]>, Rc<[TextRunOverride]>);

/// A stable authored inline leaf. Fetched titles and favicons never alter
/// its text, layout, byte offsets or clipboard content.
#[derive(Clone, Debug)]
pub(super) struct PreparedLeaf {
    base: Rc<LeafPresentation>,
}
impl PreparedLeaf {
    fn resident_bytes(&self) -> usize {
        std::mem::size_of::<LeafPresentation>() + self.base.heap_bytes()
    }
    fn new(parts: &mut PreparedParts) -> Self {
        Self {
            base: Rc::new(LeafPresentation::from_flat(
                parts.inline.pop_front().expect("normalized inline leaf"),
            )),
        }
    }
    pub(super) fn presentation(&self) -> Rc<LeafPresentation> {
        self.base.clone()
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
    resolved: RefCell<Option<ResolvedLeaf>>,
}

#[derive(Debug)]
struct ResolvedLeaf {
    key: ([Oklch; 3], &'static str),
    highlights: Rc<[(Range<usize>, HighlightStyle)]>,
    overrides: Rc<[TextRunOverride]>,
}

impl LeafPresentation {
    fn heap_bytes(&self) -> usize {
        self.text.len()
            + std::mem::size_of_val(self.runs.as_ref())
            + std::mem::size_of_val(self.chains.as_ref())
            + self
                .chains
                .iter()
                .map(|chain| std::mem::size_of_val(chain.as_ref()))
                .sum::<usize>()
            + std::mem::size_of_val(self.link_ranges.as_ref())
            + rich::budget::shared(&self.link_destinations)
            + std::mem::size_of_val(self.color_ranges.as_ref())
            + rich::budget::shared(&self.color_values)
            + std::mem::size_of_val(self.swatches.as_ref())
            + std::mem::size_of_val(self.copies.as_ref())
            + self
                .copies
                .iter()
                .map(|(_, text)| text.len())
                .sum::<usize>()
            + std::mem::size_of_val(self.code_ranges.as_ref())
            + self.runs.len() * std::mem::size_of::<(Range<usize>, HighlightStyle)>()
            + self.code_ranges.len() * std::mem::size_of::<TextRunOverride>()
    }
    fn from_flat(flat: FlatInline) -> Self {
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
        let key = (
            [
                theme.colors.foreground,
                theme.colors.muted_foreground,
                theme.colors.banner_info,
            ],
            mono_family,
        );
        let mut memo = self.resolved.borrow_mut();
        if let Some(resolved) = memo.as_ref()
            && resolved.key == key
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
            key,
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
pub(super) struct PreparedFenceData {
    /// The fence body as displayed (final line terminator dropped).
    pub(super) source: SharedString,
    /// Whether the displayed body spans more than one line.
    pub(super) multiline: bool,
    /// Whether the fence is a closed `mermaid` block, drawn as a diagram
    /// when its source renders.
    pub(super) diagram: Option<super::diagram::MermaidDiagram>,
    /// The rich block an `artisan-*` fence draws, parsed once here.
    pub(super) rich: Option<RichFence>,
    /// Assembled display byte ranges and retained text per native code line.
    pub(super) lines: Arc<[(Range<usize>, SharedString)]>,
    /// The fence's color values, their swatches, and what each slot copies
    /// as, addressing assembled display byte offsets.
    pub(super) colors: FenceColors,
    tokens: Box<[(Range<usize>, CodeTokenKind)]>,
}

#[derive(Debug)]
pub(super) struct PreparedFence {
    data: PreparedFenceData,
    resolved: RefCell<Option<ResolvedFence>>,
}
impl std::ops::Deref for PreparedFence {
    type Target = PreparedFenceData;
    fn deref(&self) -> &Self::Target {
        &self.data
    }
}

#[derive(Debug)]
struct ResolvedFence {
    key: [Oklch; CODE_TOKEN_KINDS],
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
    let shift =
        |offset: usize| offset + slot.len() * colors.partition_point(|color| color.start < offset);
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

fn code_lines(text: &SharedString) -> Arc<[(Range<usize>, SharedString)]> {
    let mut start = 0;
    let mut lines = Vec::new();
    for part in text.split_inclusive('\n') {
        let end = start + part.trim_end_matches('\n').len();
        lines.push((start..end, SharedString::from(text[start..end].to_owned())));
        start += part.len();
    }
    if lines.is_empty() || text.ends_with('\n') {
        lines.push((start..start, SharedString::default()));
    }
    lines.into()
}

/// Color values in a fence's drawn text, ready for the selectable text.
#[derive(Clone, Debug, Default)]
pub(super) struct FenceColors {
    pub(super) ranges: Arc<[Range<usize>]>,
    pub(super) values: Arc<[SharedString]>,
    pub(super) swatches: Arc<[InlineSwatch]>,
    pub(super) copies: Arc<[(Range<usize>, SharedString)]>,
}

impl PreparedFenceData {
    fn heap_bytes(&self) -> usize {
        let rich = self.rich.as_ref().map_or(0, |rich| match rich {
            RichFence::Ready(prepared) => prepared.resident_bytes(),
            RichFence::Pending { kind } => kind.len(),
            RichFence::Invalid { note } => note.len(),
        });
        self.source.len()
            + std::mem::size_of_val(self.lines.as_ref())
            + self.lines.iter().map(|(_, line)| line.len()).sum::<usize>()
            + std::mem::size_of_val(self.tokens.as_ref())
            + self.tokens.len() * std::mem::size_of::<(Range<usize>, HighlightStyle)>()
            + std::mem::size_of_val(self.colors.ranges.as_ref())
            + rich::budget::shared(&self.colors.values)
            + std::mem::size_of_val(self.colors.swatches.as_ref())
            + std::mem::size_of_val(self.colors.copies.as_ref())
            + self
                .colors
                .copies
                .iter()
                .map(|(_, text)| text.len())
                .sum::<usize>()
            + rich
            + self.diagram.as_ref().map_or(0, |diagram| {
                super::rich::image_resident_bytes(&diagram.image)
            })
    }
    fn new(fence: &CodeFence, renderer: Option<&gpui::SvgRenderer>) -> Self {
        let rich = rich::prepare(
            fence.language.as_deref(),
            fence_display_source(&fence.source),
            fence.closed,
        );
        let display = fence_display_source(&fence.source);
        let tokens: Vec<_> = fence
            .tokens
            .as_deref()
            .unwrap_or_default()
            .iter()
            .filter_map(|token| valid_code_range(token, display))
            .collect();
        let diagram = fence
            .diagram
            .as_ref()
            .and_then(|svg| super::diagram::image(svg, renderer?.clone()));
        // Rich blocks and diagrams draw something else; plain code shows
        // its colors.
        let starts = if rich.is_none() && diagram.is_none() {
            let whole = 0..display.len();
            rich::color_ranges(display, std::slice::from_ref(&whole))
        } else {
            Vec::new()
        };
        let source = SharedString::from(display);
        let (shown, colors, tokens) = if starts.is_empty() {
            (source.clone(), FenceColors::default(), tokens)
        } else {
            decorate_colors(display, &starts, tokens)
        };
        Self {
            multiline: display.contains('\n'),
            diagram,
            rich,
            source,
            lines: code_lines(&shown),
            colors,
            tokens: tokens.into_boxed_slice(),
        }
    }
}

impl PreparedFence {
    fn new(data: PreparedFenceData) -> Self {
        Self {
            data,
            resolved: RefCell::new(None),
        }
    }

    /// Token highlight runs for `theme`, reused while the kind palette
    /// resolves unchanged.
    pub(super) fn highlights(&self, theme: &ArtisanTheme) -> Rc<[(Range<usize>, HighlightStyle)]> {
        let key = [
            theme.colors.muted_foreground,
            theme.colors.banner_success,
            theme.colors.banner_warning,
            theme.colors.accent_foreground,
            theme.colors.primary,
            theme.colors.question_to,
            theme.colors.foreground,
        ];
        let mut memo = self.resolved.borrow_mut();
        if let Some(resolved) = memo.as_ref()
            && resolved.key == key
        {
            return Rc::clone(&resolved.highlights);
        }
        let palette = ALL_KINDS.map(|kind| code_token_style(theme, kind));
        let highlights: Rc<[_]> = self
            .tokens
            .iter()
            .map(|(range, kind)| (range.clone(), palette[kind_index(*kind)]))
            .collect();
        *memo = Some(ResolvedFence {
            key,
            highlights: Rc::clone(&highlights),
        });
        highlights
    }
}

#[cfg(test)]
mod fence_color_tests {
    use super::*;
    use crate::markdown_renderer::inline::SWATCH_SLOT;

    #[test]
    fn nested_lists_and_tables_publish_independent_semantic_rows() {
        let document = crate::markdown::MarkdownEngine::new()
            .unwrap()
            .parse_document("- first\n  - nested\n- last\n\n| a | b |\n| --- | --- |\n| x | y |\n")
            .unwrap();
        let prepared = PreparedMarkdown::new(document);
        assert_eq!(prepared.block_count(), 5);
        assert_eq!(
            prepared
                .flow
                .iter()
                .map(|row| row.indent)
                .collect::<Vec<_>>(),
            vec![1, 2, 1, 0, 0]
        );
        assert!(matches!(
            prepared.flow[3].kind,
            FlowKind::TableRow { row: 0, .. }
        ));
        assert!(matches!(
            prepared.flow[4].kind,
            FlowKind::TableRow { row: 1, .. }
        ));
        let mut leaves = Vec::new();
        collect_selection(&prepared.root, &[41], &mut leaves, "\n\n");
        assert_eq!(
            leaves
                .iter()
                .map(|leaf| leaf.text.as_ref())
                .collect::<Vec<_>>(),
            vec!["first", "nested", "last", "a", "b", "x", "y"]
        );
        assert!(leaves.windows(2).all(|pair| pair[0].order < pair[1].order));
        assert!(leaves.iter().all(|leaf| leaf.order[0] == 41));
    }

    #[test]
    fn code_rows_preserve_empty_lines_and_byte_ranges() {
        let source = SharedString::from("\né\n\n");
        let lines = code_lines(&source);
        assert_eq!(
            lines
                .iter()
                .map(|(_, text)| text.as_ref())
                .collect::<Vec<_>>(),
            vec!["", "é", "", ""]
        );
        assert!(
            lines
                .iter()
                .all(|(range, text)| &source[range.clone()] == text.as_ref())
        );
        assert_eq!(lines[1].0, 1..3);
    }

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
