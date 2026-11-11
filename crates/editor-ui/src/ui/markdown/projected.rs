use super::presenter::{PresenterError, ProjectedGeometry, ProjectedTextLeaf};
use super::visual::{
    BodyTone, LinkAction, interactive_leaf_with_opacity, interactive_table_leaf_with_opacity,
    prepare_inline, raw_leaf,
};
use super::{
    Block, BlockShape, CodeFence, IndexedLeafContent, LeafContext, LeafSpacing, MarkdownEngine,
    MarkdownIndex, MarkdownIndexError, TableAlignment, TableCell,
};
use crate::ui::rich::actions::{ResourceScope, ShowCopyMenu};
use crate::ui::theme::{ArtisanTheme, ProseTypography};
use aqua_core::Insets;
use aqua_text::document::{SelectionDocumentId, SelectionSource, SelectionSourceId};
use aqua_text::{FontFamily, FontWeight, TextStyle};
use aqua_ui::collection_runtime::{SelectionBatchOwner, SelectionSourceBatch};
use aqua_ui::element::{Element, ui};
use aqua_ui::style::{Length, Style};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

struct Leaf {
    range: Range<usize>,
    signature: u64,
    context: LeafContext,
    content: IndexedLeafContent,
    table_continues: bool,
    rich_without_source: bool,
    fence: Option<super::fence::Fence>,
    fallback_note: Option<String>,
    rich_index: Option<usize>,
}

struct LeafView<'a> {
    context: &'a LeafContext,
    content: &'a IndexedLeafContent,
    table_continues: bool,
    fence: Option<&'a super::fence::Fence>,
    fallback_note: Option<&'a str>,
    rich_index: Option<usize>,
}

impl Leaf {
    fn view(&self) -> LeafView<'_> {
        LeafView {
            context: &self.context,
            content: &self.content,
            table_continues: self.table_continues,
            fence: self.fence.as_ref(),
            fallback_note: self.fallback_note.as_deref(),
            rich_index: self.rich_index,
        }
    }
}

struct CachedSources {
    document: SelectionDocumentId,
    owner: Option<String>,
    prefix: Vec<u64>,
    sources: Arc<[SelectionSource]>,
    batch: Arc<SelectionSourceBatch>,
    rows: Vec<Range<usize>>,
}

pub fn leaf_signatures(
    source: &str,
    command: Option<&str>,
) -> Result<Vec<u64>, MarkdownIndexError> {
    let index = MarkdownIndex::build(source)?;
    (0..index.leaves().len())
        .map(|ordinal| {
            let context = index
                .context(ordinal)
                .ok_or(MarkdownIndexError::InvalidShape)?;
            let table_continues = context.table_source.is_some()
                && index
                    .context(ordinal + 1)
                    .and_then(|next| next.table_source)
                    == context.table_source;
            Ok(leaf_signature(
                source,
                &index.leaves()[ordinal].source_range(),
                context,
                table_continues,
                command.filter(|_| ordinal == 0),
            ))
        })
        .collect()
}

fn leaf_signature(
    source: &str,
    range: &Range<usize>,
    context: &LeafContext,
    table_continues: bool,
    command: Option<&str>,
) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    range.start.hash(&mut hasher);
    source.get(range.clone()).hash(&mut hasher);
    context.hash(&mut hasher);
    table_continues.hash(&mut hasher);
    command.hash(&mut hasher);
    hasher.finish()
}

pub(super) struct ProjectedDocument {
    leaves: Vec<Leaf>,
    cached_sources: RefCell<Option<Rc<CachedSources>>>,
}

impl ProjectedDocument {
    pub(super) fn new(
        engine: &MarkdownEngine,
        source: &str,
        command: Option<&str>,
    ) -> Result<Self, MarkdownIndexError> {
        let index = MarkdownIndex::build(source)?;
        let mut leaves = Vec::with_capacity(index.leaves().len());
        for ordinal in 0..index.leaves().len() {
            let context = index
                .context(ordinal)
                .cloned()
                .ok_or(MarkdownIndexError::InvalidShape)?;
            let table_continues = context.table_source.is_some()
                && index
                    .context(ordinal + 1)
                    .and_then(|next| next.table_source)
                    == context.table_source;
            let mut content = index.normalize_leaf(ordinal, 0, source, engine)?;
            if let (0, Some(command), IndexedLeafContent::Block(Block::Paragraph { spans, .. })) =
                (ordinal, command, &mut content)
            {
                super::mark_command_spans(spans, command);
            }
            let rich = match &content {
                IndexedLeafContent::Block(Block::Code(code)) => {
                    crate::ui::rich::prepare(code.language.as_deref(), &code.source, code.closed)
                }
                _ => None,
            };
            let rich_without_source = matches!(
                &rich,
                Some(
                    crate::ui::rich::RichFence::Ready(_)
                        | crate::ui::rich::RichFence::Pending { .. }
                )
            ) || matches!(
                &content,
                IndexedLeafContent::Block(Block::Code(code)) if code.diagram.is_some()
            );
            let fallback_note = match rich {
                Some(crate::ui::rich::RichFence::Invalid { note }) => Some(note),
                _ => None,
            };
            let fence = match &content {
                IndexedLeafContent::Block(Block::Code(code))
                    if !rich_without_source && code.diagram.is_none() =>
                {
                    Some(super::fence::Fence::new(code))
                }
                _ => None,
            };
            let range = index.leaves()[ordinal].source_range();
            let signature = leaf_signature(source, &range, &context, table_continues, command.filter(|_| ordinal == 0));
            leaves.push(Leaf {
                range,
                signature,
                context,
                content,
                table_continues,
                rich_without_source,
                fence,
                fallback_note,
                rich_index: None,
            });
        }
        Ok(Self {
            leaves,
            cached_sources: RefCell::new(None),
        })
    }
    pub(super) fn len(&self) -> usize {
        self.leaves.len()
    }
    pub(super) fn signature(&self, row: usize) -> Option<u64> {
        Some(self.leaves.get(row)?.signature)
    }
    pub(super) fn depth(&self, row: usize) -> Option<usize> {
        Some(self.leaves.get(row)?.context.lists.len())
    }
    pub(super) fn range(&self, row: usize) -> Option<Range<usize>> {
        Some(self.leaves.get(row)?.range.clone())
    }

    pub(super) fn bind_rich<'a>(&mut self, entries: impl Iterator<Item = (usize, &'a CodeFence)>) {
        let mut indices: HashMap<_, VecDeque<usize>> = HashMap::new();
        for (index, fence) in entries {
            indices
                .entry((&fence.language, fence.source.as_str(), fence.closed))
                .or_default()
                .push_back(index);
        }
        for leaf in &mut self.leaves {
            if let IndexedLeafContent::Block(Block::Code(fence)) = &leaf.content {
                leaf.rich_index = indices
                    .get_mut(&(&fence.language, fence.source.as_str(), fence.closed))
                    .and_then(VecDeque::pop_front);
            }
        }
    }

    fn selection_sources(
        &self,
        theme: &ArtisanTheme,
        document: SelectionDocumentId,
        owner: Option<&str>,
        batch_owner: SelectionBatchOwner,
        revision: &Cell<u64>,
        prefix: &[u64],
        source_id: &mut dyn FnMut(usize) -> SelectionSourceId,
    ) -> Result<Rc<CachedSources>, PresenterError> {
        if let Some(cached) = self.cached_sources.borrow().as_ref() {
            if cached.document == document
                && cached.owner.as_deref() == owner
                && cached.prefix == prefix
            {
                return Ok(cached.clone());
            }
        }
        let mut sources = Vec::new();
        let mut rows = Vec::with_capacity(self.leaves.len());
        for (ordinal, leaf) in self.leaves.iter().enumerate() {
            let source_start = sources.len();
            let mut order = prefix.to_vec();
            order.push(ordinal as u64);
            match &leaf.content {
                IndexedLeafContent::Block(
                    Block::Paragraph { spans, .. } | Block::Heading { spans, .. },
                ) => {
                    let id = source_id(sources.len());
                    sources.push(
                        prepare_inline(
                            spans,
                            theme,
                            id,
                            order,
                            if leaf.context.lists.is_empty() {
                                "\n\n"
                            } else {
                                "\n"
                            },
                        )
                        .source,
                    );
                }
                IndexedLeafContent::Block(Block::Html { source }) => {
                    let id = source_id(sources.len());
                    sources.push(SelectionSource {
                        id,
                        order,
                        text: Arc::from(source.as_str()),
                        separator: Arc::from("\n"),
                        copies: Vec::new(),
                    });
                }
                IndexedLeafContent::Block(Block::Code(_)) => {
                    if let Some(fence) = &leaf.fence {
                        let offset = sources.len();
                        sources.extend(fence.sources(&order, source_id, offset));
                    } else if !leaf.rich_without_source {
                        return Err(PresenterError::Unavailable("projected code selection"));
                    }
                }
                IndexedLeafContent::TableRow {
                    cells, alignments, ..
                } => {
                    for (column, cell) in cells.iter().take(alignments.len()).enumerate() {
                        let id = source_id(sources.len());
                        let mut order = order.clone();
                        order.push(column as u64);
                        sources.push(
                            prepare_inline(
                                &cell.spans,
                                theme,
                                id,
                                order,
                                if column == 0 { "\n" } else { "\t" },
                            )
                            .source,
                        );
                    }
                }
                IndexedLeafContent::Block(Block::List { .. } | Block::Table(_)) => {
                    return Err(PresenterError::Unavailable("projected nested flow"));
                }
            }
            rows.push(source_start..sources.len());
        }
        let next_revision = revision
            .get()
            .checked_add(1)
            .ok_or(PresenterError::Unavailable(
                "selection batch revision exhausted",
            ))?;
        revision.set(next_revision);
        let sources: Arc<[SelectionSource]> = sources.into();
        let batch = Arc::new(SelectionSourceBatch::new(
            document,
            batch_owner,
            next_revision,
            sources.clone(),
        ));
        let cached = Rc::new(CachedSources {
            document,
            owner: owner.map(str::to_owned),
            prefix: prefix.to_vec(),
            sources,
            batch,
            rows,
        });
        *self.cached_sources.borrow_mut() = Some(cached.clone());
        Ok(cached)
    }

    pub(super) fn render(
        &self,
        row: usize,
        theme: &ArtisanTheme,
        tone: BodyTone,
        document: SelectionDocumentId,
        owner: Option<&str>,
        reveal: Option<&super::reveal::PreparedReveal>,
        batch_owner: SelectionBatchOwner,
        revision: &Cell<u64>,
        prefix: &[u64],
        source_id: &mut dyn FnMut(usize) -> SelectionSourceId,
        on_link: Option<Rc<dyn Fn(LinkAction)>>,
        on_color: Option<Rc<dyn Fn(ShowCopyMenu)>>,
        scope: Option<ResourceScope>,
        rich: Option<&dyn Fn(usize, &CodeFence) -> Result<Element, PresenterError>>,
        ordinary: Option<
            &dyn Fn(
                usize,
                &super::fence::Fence,
                &[SelectionSource],
                Option<&str>,
            ) -> Result<Element, PresenterError>,
        >,
    ) -> Result<
        (
            Element,
            Arc<[SelectionSource]>,
            Arc<SelectionSourceBatch>,
            ProjectedGeometry,
        ),
        PresenterError,
    > {
        let target = self
            .leaves
            .get(row)
            .ok_or(PresenterError::Unavailable("projected row"))?;
        let cached = self.selection_sources(
            theme,
            document,
            owner,
            batch_owner,
            revision,
            prefix,
            source_id,
        )?;
        let (element, geometry) = render_leaf(
            &target.view(),
            row,
            &cached.sources[cached.rows[row].clone()],
            cached.rows[row].start,
            row + 1 == self.leaves.len(),
            theme,
            tone,
            document,
            reveal,
            batch_owner,
            prefix,
            on_link,
            on_color,
            scope,
            rich,
            ordinary,
        )?;
        Ok((
            element,
            cached.sources.clone(),
            cached.batch.clone(),
            geometry,
        ))
    }
}

pub const PROSE_MEASURE_PX: f32 = 672.0;

pub fn prose_measure(is_code_block: bool) -> Length {
    if is_code_block {
        Length::Auto
    } else {
        Length::Px(PROSE_MEASURE_PX)
    }
}

fn render_leaf(
    target: &LeafView<'_>,
    row: usize,
    target_sources: &[SelectionSource],
    slot_start: usize,
    final_row: bool,
    theme: &ArtisanTheme,
    tone: BodyTone,
    document: SelectionDocumentId,
    reveal: Option<&super::reveal::PreparedReveal>,
    batch_owner: SelectionBatchOwner,
    prefix: &[u64],
    on_link: Option<Rc<dyn Fn(LinkAction)>>,
    on_color: Option<Rc<dyn Fn(ShowCopyMenu)>>,
    scope: Option<ResourceScope>,
    rich: Option<&dyn Fn(usize, &CodeFence) -> Result<Element, PresenterError>>,
    ordinary: Option<
        &dyn Fn(
            usize,
            &super::fence::Fence,
            &[SelectionSource],
            Option<&str>,
        ) -> Result<Element, PresenterError>,
    >,
) -> Result<(Element, ProjectedGeometry), PresenterError> {
    let target_ids: Vec<_> = target_sources.iter().map(|source| source.id).collect();
    let text_key = |id: SelectionSourceId| format!("markdown-reveal-{batch_owner:?}-{}", id.0);
    let text_slots = matches!(
        target.content,
        IndexedLeafContent::Block(Block::Paragraph { .. } | Block::Heading { .. })
            | IndexedLeafContent::TableRow { .. }
    );
    let text: Vec<_> = if text_slots {
        target_sources
            .iter()
            .enumerate()
            .map(|(index, source)| ProjectedTextLeaf {
                key: text_key(source.id),
                slot: slot_start + index,
                text: source.text.clone(),
            })
            .collect()
    } else {
        Vec::new()
    };
    let tail = if final_row {
        text.last().map(|leaf| leaf.slot)
    } else {
        None
    };
    let opacity: Vec<Vec<aqua_core::scene::GpuiTextOpacityRange>> = if let Some(reveal) = reveal {
        reveal.set_tail(tail);
        text.iter()
            .map(|leaf| {
                reveal
                    .fades(leaf.slot, leaf.text.clone())
                    .into_iter()
                    .map(|fade| aqua_core::scene::GpuiTextOpacityRange {
                        bytes: fade.range,
                        opacity: fade.opacity,
                    })
                    .collect()
            })
            .collect()
    } else {
        (0..text.len()).map(|_| Vec::new()).collect()
    };
    let frame_key = format!("markdown-reveal-frame-{batch_owner:?}-{row}");
    let block_entrance = matches!(
        target.content,
        IndexedLeafContent::Block(Block::Code(_) | Block::Html { .. })
    );
    let mut row_order = prefix.to_vec();
    row_order.push(row as u64);
    let separator = if target.context.lists.is_empty() {
        "\n\n"
    } else {
        "\n"
    };
    let content = match target.content {
        IndexedLeafContent::Block(Block::Paragraph { spans, .. }) => interactive_leaf_with_opacity(
            spans,
            theme,
            tone,
            document,
            target_ids[0],
            row_order,
            separator,
            on_link,
            on_color,
            scope,
            None,
            opacity[0].clone(),
        )
        .map_err(PresenterError::Inline)?
        .element
        .with_key(text_key(target_ids[0])),
        IndexedLeafContent::Block(Block::Heading { level, spans, .. }) => {
            interactive_leaf_with_opacity(
                spans,
                theme,
                BodyTone::Foreground,
                document,
                target_ids[0],
                row_order,
                separator,
                on_link,
                on_color,
                scope,
                Some(*level),
                opacity[0].clone(),
            )
            .map_err(PresenterError::Inline)?
            .element
            .with_key(text_key(target_ids[0]))
        }
        IndexedLeafContent::Block(Block::Html { source }) => {
            raw_leaf(
                source,
                theme,
                tone,
                document,
                target_ids[0],
                row_order,
                "\n",
            )
            .element
        }
        IndexedLeafContent::TableRow {
            header,
            alignments,
            cells,
        } => table_row(
            theme,
            tone,
            document,
            &row_order,
            *header,
            target.table_continues,
            alignments,
            cells,
            &target_ids,
            &text_key,
            &opacity,
            on_link,
            on_color,
            scope,
        )?,
        IndexedLeafContent::Block(Block::Code(fence)) => {
            if let Some(code) = target.fence {
                if code.has_colors() && on_color.is_none() {
                    return Err(PresenterError::Unavailable("code color actions"));
                }
                ordinary.ok_or(PresenterError::Unavailable("code copy services"))?(
                    row,
                    code,
                    target_sources,
                    target.fallback_note,
                )?
            } else {
                rich.ok_or(PresenterError::Unavailable("projected rich or code block"))?(
                    target
                        .rich_index
                        .or_else(|| fence.diagram.as_ref().map(|_| row))
                        .ok_or(PresenterError::Unavailable("nested rich block"))?,
                    fence,
                )?
            }
        }
        _ => return Err(PresenterError::Unavailable("projected nested flow")),
    };
    let gap: f32 = target
        .context
        .spacing
        .iter()
        .copied()
        .map(indexed_gap)
        .sum();
    let indent = target.context.lists.len();
    let measure = prose_measure(matches!(
        target.content,
        IndexedLeafContent::Block(Block::Code(_))
    ));
    let mut frame = ui::Box::new()
        .width(Length::Pct(100.0))
        .style(Style::new().max_width(measure).padding_insets(Insets {
            left: crate::ui::rich::charts::scale::f32_of_count(indent)
                * ProseTypography::LIST_INDENT_PX,
            top: gap,
            ..Insets::default()
        }))
        .child(content);
    for (depth, level) in target
        .context
        .lists
        .iter()
        .enumerate()
        .filter(|(_, level)| level.first)
    {
        let marker = match level.task {
            Some(true) => "☑".to_owned(),
            Some(false) => "☐".to_owned(),
            None => level
                .number
                .map_or_else(|| "•".to_owned(), |number| format!("{number}.")),
        };
        let style = TextStyle {
            families: vec![FontFamily::Named(theme.typography.sans.family.to_owned())],
            size_px: ProseTypography::BODY_SIZE_PX,
            line_height_px: ProseTypography::BODY_LINE_PX,
            weight: if level.number.is_some() {
                FontWeight::NORMAL
            } else {
                ProseTypography::BODY_WEIGHT
            },
            letter_spacing_em: Some(theme.typography.tracking_em),
            ..TextStyle::default()
        };
        let marker = ui::Text::new(marker)
            .text_style(style)
            .gpui_source_color(theme.colors.text_muted.to_reference_text_color())
            .wrap(false)
            .build();
        frame = frame.child(
            ui::Box::new()
                .style(Style::new().absolute_at(
                    Some(
                        crate::ui::rich::charts::scale::f32_of_count(depth)
                            * ProseTypography::LIST_INDENT_PX,
                    ),
                    Some(gap),
                    None,
                    None,
                ))
                .child(marker)
                .build(),
        );
    }
    Ok((
        frame.build().with_key(frame_key.clone()),
        ProjectedGeometry {
            frame_key,
            text,
            tail,
            block_entrance,
        },
    ))
}

pub(super) fn stored_leaf_sources(
    leaf: &super::stored::StoredLeaf,
    row: usize,
    theme: &ArtisanTheme,
    prefix: &[u64],
    source_id: &mut dyn FnMut(usize) -> SelectionSourceId,
    fence: Option<&super::fence::Fence>,
    rich_without_source: bool,
) -> Result<(Arc<[SelectionSource]>, usize), PresenterError> {
    let mut sources = Vec::new();
    let mut bytes = fence.map_or(0, super::fence::Fence::resident_bytes);
    let mut order = prefix.to_vec();
    order.push(row as u64);
    match &leaf.content {
        IndexedLeafContent::Block(
            Block::Paragraph { spans, .. } | Block::Heading { spans, .. },
        ) => {
            let prepared = prepare_inline(
                spans,
                theme,
                source_id(0),
                order,
                if leaf.context.lists.is_empty() {
                    "\n\n"
                } else {
                    "\n"
                },
            );
            bytes = bytes.saturating_add(prepared.resident_bytes());
            sources.push(prepared.source);
        }
        IndexedLeafContent::Block(Block::Html { source }) => sources.push(SelectionSource {
            id: source_id(0),
            order,
            text: Arc::from(source.as_str()),
            separator: Arc::from("\n"),
            copies: Vec::new(),
        }),
        IndexedLeafContent::Block(Block::Code(_)) => {
            if let Some(fence) = fence {
                sources.extend(fence.sources(&order, source_id, 0));
            } else if !rich_without_source {
                return Err(PresenterError::Unavailable("projected code selection"));
            }
        }
        IndexedLeafContent::TableRow {
            cells, alignments, ..
        } => {
            for (column, cell) in cells.iter().take(alignments.len()).enumerate() {
                let mut cell_order = order.clone();
                cell_order.push(column as u64);
                let prepared = prepare_inline(
                    &cell.spans,
                    theme,
                    source_id(column),
                    cell_order,
                    if column == 0 { "\n" } else { "\t" },
                );
                bytes = bytes.saturating_add(prepared.resident_bytes());
                sources.push(prepared.source);
            }
        }
        IndexedLeafContent::Block(Block::List { .. } | Block::Table(_)) => {
            return Err(PresenterError::Unavailable("projected nested flow"));
        }
    }
    Ok((sources.into(), bytes))
}

pub(super) fn render_stored_leaf(
    leaf: &super::stored::StoredLeaf,
    row: usize,
    target_sources: &[SelectionSource],
    slot_start: usize,
    final_row: bool,
    theme: &ArtisanTheme,
    tone: BodyTone,
    document: SelectionDocumentId,
    reveal: Option<&super::reveal::PreparedReveal>,
    batch_owner: SelectionBatchOwner,
    prefix: &[u64],
    on_link: Option<Rc<dyn Fn(LinkAction)>>,
    on_color: Option<Rc<dyn Fn(ShowCopyMenu)>>,
    scope: Option<ResourceScope>,
    fence: Option<&super::fence::Fence>,
    fallback_note: Option<&str>,
    rich: Option<&dyn Fn(usize, &CodeFence) -> Result<Element, PresenterError>>,
    ordinary: Option<
        &dyn Fn(
            usize,
            &super::fence::Fence,
            &[SelectionSource],
            Option<&str>,
        ) -> Result<Element, PresenterError>,
    >,
) -> Result<(Element, ProjectedGeometry), PresenterError> {
    let target = LeafView {
        context: &leaf.context,
        content: &leaf.content,
        table_continues: leaf.table_continues,
        fence,
        fallback_note,
        rich_index: Some(row),
    };
    render_leaf(
        &target,
        row,
        target_sources,
        slot_start,
        final_row,
        theme,
        tone,
        document,
        reveal,
        batch_owner,
        prefix,
        on_link,
        on_color,
        scope,
        rich,
        ordinary,
    )
}

fn margins(shape: BlockShape, item: bool) -> (f32, f32) {
    match shape {
        BlockShape::Heading(level) => {
            let h = ProseTypography::heading(level);
            (h.margin_top_px, h.margin_bottom_px)
        }
        BlockShape::Paragraph => {
            let v = if item {
                ProseTypography::ITEM_PARAGRAPH_MARGIN_PX
            } else {
                ProseTypography::PARAGRAPH_MARGIN_PX
            };
            (v, v)
        }
        BlockShape::Html => (
            ProseTypography::PARAGRAPH_MARGIN_PX,
            ProseTypography::PARAGRAPH_MARGIN_PX,
        ),
        BlockShape::Code => (
            ProseTypography::FENCE_MARGIN_PX,
            ProseTypography::FENCE_MARGIN_PX,
        ),
        BlockShape::List => {
            let v = if item {
                ProseTypography::NESTED_LIST_MARGIN_PX
            } else {
                ProseTypography::LIST_MARGIN_PX
            };
            (v, v)
        }
        BlockShape::Table => (
            ProseTypography::TABLE_MARGIN_PX,
            ProseTypography::TABLE_MARGIN_PX,
        ),
    }
}
fn indexed_gap(spacing: LeafSpacing) -> f32 {
    match spacing {
        LeafSpacing::Item => ProseTypography::ITEM_GAP_PX,
        LeafSpacing::Sibling { previous: None, .. } => 0.0,
        LeafSpacing::Sibling {
            previous: Some(previous),
            current,
            in_item,
        } => {
            let bottom = margins(previous, in_item).1;
            if matches!(previous, BlockShape::Heading(2..=4)) {
                bottom
            } else {
                bottom.max(margins(current, in_item).0)
            }
        }
    }
}

fn table_row(
    theme: &ArtisanTheme,
    tone: BodyTone,
    document: SelectionDocumentId,
    order: &[u64],
    header: bool,
    continues: bool,
    alignments: &[TableAlignment],
    cells: &[TableCell],
    ids: &[SelectionSourceId],
    text_key: &dyn Fn(SelectionSourceId) -> String,
    opacity: &[Vec<aqua_core::scene::GpuiTextOpacityRange>],
    on_link: Option<Rc<dyn Fn(LinkAction)>>,
    on_color: Option<Rc<dyn Fn(ShowCopyMenu)>>,
    scope: Option<ResourceScope>,
) -> Result<Element, PresenterError> {
    let count = alignments.len();
    if count == 0 {
        return Ok(ui::Box::new().build());
    }
    #[expect(
        clippy::cast_precision_loss,
        reason = "the preview bounds table columns"
    )]
    let width = 100.0 / count as f32;
    let mut grid = ui::Box::new().layout("row").width(Length::Pct(100.0));
    for (column, alignment) in alignments.iter().enumerate() {
        let align = match alignment {
            TableAlignment::Unset | TableAlignment::Left => aqua_ui::AlignItems::START,
            TableAlignment::Center => aqua_ui::AlignItems::CENTER,
            TableAlignment::Right => aqua_ui::AlignItems::END,
        };
        let mut style = Style::new()
            .width(Length::Pct(width))
            .min_width(Length::Px(0.0))
            .align(align)
            .padding_insets(Insets {
                left: if column > 0 { 8.0 } else { 0.0 },
                right: if column + 1 < count { 8.0 } else { 0.0 },
                top: if header { 0.0 } else { 8.0 },
                bottom: 8.0,
            });
        if header {
            style = style.justify(aqua_ui::JustifyContent::END);
        }
        if continues {
            style = style.gpui_border(
                Insets {
                    bottom: 1.0,
                    ..Insets::default()
                },
                theme.colors.line.to_reference_paint(),
                false,
            );
        }
        let mut cell = ui::Box::new().layout("column").style(style);
        if let (Some(value), Some(id)) = (cells.get(column), ids.get(column)) {
            let mut order = order.to_vec();
            order.push(column as u64);
            cell = cell.child(
                interactive_table_leaf_with_opacity(
                    &value.spans,
                    theme,
                    tone,
                    header,
                    document,
                    *id,
                    order,
                    if column == 0 { "\n" } else { "\t" },
                    on_link.clone(),
                    on_color.clone(),
                    scope.clone(),
                    opacity.get(column).cloned().unwrap_or_default(),
                )
                .map_err(PresenterError::Inline)?
                .element
                .with_key(text_key(*id)),
            );
        }
        grid = grid.child(cell.build());
    }
    Ok(grid.build())
}
