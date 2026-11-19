use super::super::visual::{BodyTone, LinkAction, interactive_leaf, raw_leaf};
use super::super::{Block, CodeFence};
use super::{
    CodeServices, HeatmapServices, MarkdownPresenter, PreparationProof, PresenterError,
    RenderedMarkdown, RetainedRich, fence_key,
};
use crate::ui::kit::TextRole;
use crate::ui::rich::charts::{self};
use crate::ui::rich::{Prepared, RichFence};
use crate::ui::theme::{ArtisanTheme, ProseTypography};
use aqua_core::{GpuiHsla, Insets};
use aqua_text::document::{SelectionDocumentId, SelectionSourceId};
use aqua_ui::element::{Element, ui};
use aqua_ui::style::{Length, Style};
use editor_assets::AssetId;
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;
use std::sync::Arc;

impl MarkdownPresenter {
    pub fn render(
        &self,
        id: &str,
        theme: &ArtisanTheme,
        tone: BodyTone,
        selection_document: SelectionDocumentId,
        order: &[u64],
        source_id: impl FnMut(usize) -> SelectionSourceId,
        glyph: Rc<dyn Fn(AssetId, f32, GpuiHsla) -> Element>,
        invalidate: Rc<dyn Fn()>,
    ) -> Result<RenderedMarkdown, PresenterError> {
        self.render_with_actions(
            id,
            theme,
            tone,
            selection_document,
            order,
            source_id,
            glyph,
            invalidate,
            None,
        )
    }

    fn render_with_actions(
        &self,
        id: &str,
        theme: &ArtisanTheme,
        tone: BodyTone,
        selection_document: SelectionDocumentId,
        order: &[u64],
        source_id: impl FnMut(usize) -> SelectionSourceId,
        glyph: Rc<dyn Fn(AssetId, f32, GpuiHsla) -> Element>,
        invalidate: Rc<dyn Fn()>,
        on_link: Option<Rc<dyn Fn(LinkAction)>>,
    ) -> Result<RenderedMarkdown, PresenterError> {
        self.render_with_context(
            id,
            theme,
            tone,
            selection_document,
            order,
            source_id,
            glyph,
            invalidate,
            on_link,
            None,
        )
    }

    pub(crate) fn render_with_context(
        &self,
        id: &str,
        theme: &ArtisanTheme,
        tone: BodyTone,
        selection_document: SelectionDocumentId,
        order: &[u64],
        source_id: impl FnMut(usize) -> SelectionSourceId,
        glyph: Rc<dyn Fn(AssetId, f32, GpuiHsla) -> Element>,
        invalidate: Rc<dyn Fn()>,
        on_link: Option<Rc<dyn Fn(LinkAction)>>,
        on_color: Option<Rc<dyn Fn(crate::ui::rich::actions::ShowCopyMenu)>>,
    ) -> Result<RenderedMarkdown, PresenterError> {
        self.render_with_services(
            id,
            theme,
            tone,
            selection_document,
            order,
            source_id,
            glyph,
            invalidate,
            on_link,
            on_color,
            None,
            None,
        )
    }

    pub(super) fn render_diagram(
        &self,
        id: &str,
        index: usize,
        fence: &CodeFence,
        theme: &ArtisanTheme,
        invalidate: &Rc<dyn Fn()>,
        code: Option<&CodeServices>,
    ) -> Element {
        let block_id = format!("{id}-diagram-{index}");
        let state = Rc::clone(
            self.diagrams
                .borrow_mut()
                .entry(block_id.clone())
                .or_insert_with(|| {
                    Rc::new(RefCell::new(
                        super::super::diagram::view::DiagramState::new(),
                    ))
                }),
        );
        let content = fence.diagram.as_ref().map_or_else(
            || ui::Box::new().build(),
            |diagram| {
                super::super::diagram::view::view(theme, &block_id, diagram, &state, invalidate)
            },
        );
        let copy = code
            .filter(|services| services.copy_enabled)
            .map(|services| {
                let source = fence.source.strip_suffix('\n').unwrap_or(&fence.source);
                (
                    (services.copy)(&format!("{block_id}-copy"), source),
                    if source.contains('\n') {
                        crate::ui::rich::code::card::CopyPlacement::TopRight
                    } else {
                        crate::ui::rich::code::card::CopyPlacement::CenterRight
                    },
                )
            });
        crate::ui::rich::code::card::render(theme, content, copy, true)
    }

    pub fn render_with_services(
        &self,
        id: &str,
        theme: &ArtisanTheme,
        tone: BodyTone,
        selection_document: SelectionDocumentId,
        order: &[u64],
        mut source_id: impl FnMut(usize) -> SelectionSourceId,
        glyph: Rc<dyn Fn(AssetId, f32, GpuiHsla) -> Element>,
        invalidate: Rc<dyn Fn()>,
        on_link: Option<Rc<dyn Fn(LinkAction)>>,
        on_color: Option<Rc<dyn Fn(crate::ui::rich::actions::ShowCopyMenu)>>,
        heatmap: Option<&HeatmapServices>,
        reference_scope: Option<crate::ui::rich::actions::ResourceScope>,
    ) -> Result<RenderedMarkdown, PresenterError> {
        let document = self
            .document
            .as_ref()
            .ok_or(PresenterError::Unavailable("unprepared"))?;
        let mut column = ui::Box::new().layout("column").width(Length::Pct(100.0));
        let mut sources = Vec::new();
        let mut previous_bottom: f32 = 0.0;
        let mut previous_zeroes_follower = false;
        let mut rich_fences: HashMap<u64, VecDeque<&RetainedRich>> = HashMap::new();
        for entry in self.rich.iter().flatten() {
            rich_fences
                .entry(fence_key(&entry.source))
                .or_default()
                .push_back(entry);
        }
        for (index, block) in document.blocks().iter().enumerate() {
            let (top, bottom, element) = match block {
                Block::Paragraph { spans, .. } => {
                    let mut leaf_order = order.to_vec();
                    leaf_order.push(index as u64);
                    let leaf = interactive_leaf(
                        spans,
                        theme,
                        tone,
                        selection_document,
                        source_id(sources.len()),
                        leaf_order,
                        "\n\n",
                        on_link.clone(),
                        on_color.clone(),
                        reference_scope.clone(),
                        None,
                    )
                    .map_err(PresenterError::Inline)?;
                    sources.push(leaf.source);
                    (
                        ProseTypography::PARAGRAPH_MARGIN_PX,
                        ProseTypography::PARAGRAPH_MARGIN_PX,
                        leaf.element,
                    )
                }
                Block::Code(fence) => {
                    let Some(entry) = rich_fences
                        .get_mut(&fence_key(fence))
                        .and_then(VecDeque::pop_front)
                    else {
                        return Err(PresenterError::Unavailable("code fence or rich fallback"));
                    };
                    let element = self.render_rich(
                        entry,
                        id,
                        theme,
                        Rc::clone(&glyph),
                        Rc::clone(&invalidate),
                        heatmap,
                        None,
                        reference_scope.clone(),
                    )?;
                    (
                        ProseTypography::FENCE_MARGIN_PX,
                        ProseTypography::FENCE_MARGIN_PX,
                        element,
                    )
                }
                Block::Heading { level, spans, .. } => {
                    let mut leaf_order = order.to_vec();
                    leaf_order.push(index as u64);
                    let leaf = interactive_leaf(
                        spans,
                        theme,
                        BodyTone::Foreground,
                        selection_document,
                        source_id(sources.len()),
                        leaf_order,
                        "\n\n",
                        on_link.clone(),
                        on_color.clone(),
                        reference_scope.clone(),
                        Some(*level),
                    )
                    .map_err(PresenterError::Inline)?;
                    sources.push(leaf.source);
                    let heading = ProseTypography::heading(*level);
                    (
                        heading.margin_top_px,
                        heading.margin_bottom_px,
                        leaf.element,
                    )
                }
                Block::Html { source } => {
                    let mut leaf_order = order.to_vec();
                    leaf_order.push(index as u64);
                    let leaf = raw_leaf(
                        source,
                        theme,
                        tone,
                        selection_document,
                        source_id(sources.len()),
                        leaf_order,
                        "\n",
                    );
                    sources.push(leaf.source);
                    (
                        ProseTypography::PARAGRAPH_MARGIN_PX,
                        ProseTypography::PARAGRAPH_MARGIN_PX,
                        leaf.element,
                    )
                }
                Block::List {
                    ordered,
                    start,
                    items,
                    ..
                } => {
                    let mut prefix = order.to_vec();
                    prefix.push(index as u64);
                    let element = super::super::list::render(
                        *ordered,
                        *start,
                        items,
                        theme,
                        tone,
                        selection_document,
                        &prefix,
                        &mut source_id,
                        &mut sources,
                        on_link.clone(),
                        on_color.clone(),
                        reference_scope.clone(),
                    )?;
                    (
                        ProseTypography::LIST_MARGIN_PX,
                        ProseTypography::LIST_MARGIN_PX,
                        element,
                    )
                }
                Block::Table(table) => {
                    let mut prefix = order.to_vec();
                    prefix.push(index as u64);
                    let element = super::super::table_view::render(
                        table,
                        theme,
                        tone,
                        selection_document,
                        &prefix,
                        &mut source_id,
                        &mut sources,
                        on_link.clone(),
                        on_color.clone(),
                        reference_scope.clone(),
                    )?;
                    (
                        ProseTypography::TABLE_MARGIN_PX,
                        ProseTypography::TABLE_MARGIN_PX,
                        element,
                    )
                }
            };
            let gap = if index == 0 {
                0.0
            } else if previous_zeroes_follower {
                previous_bottom
            } else {
                previous_bottom.max(top)
            };
            column = column.child(
                ui::Box::new()
                    .width(Length::Pct(100.0))
                    .style(Style::new().padding_insets(Insets {
                        top: gap,
                        ..Insets::default()
                    }))
                    .child(element)
                    .build(),
            );
            previous_bottom = bottom;
            previous_zeroes_follower = matches!(block, Block::Heading { level: 2..=4, .. });
        }
        Ok(RenderedMarkdown {
            element: column.build(),
            selection_sources: sources,
            proof: PreparationProof(Arc::clone(&self.proof)),
        })
    }

    pub(super) fn render_rich(
        &self,
        entry: &RetainedRich,
        id: &str,
        theme: &ArtisanTheme,
        glyph: Rc<dyn Fn(AssetId, f32, GpuiHsla) -> Element>,
        invalidate: Rc<dyn Fn()>,
        heatmap: Option<&HeatmapServices>,
        code: Option<&CodeServices>,
        reference_scope: Option<crate::ui::rich::actions::ResourceScope>,
    ) -> Result<Element, PresenterError> {
        let envelope = match &entry.prepared {
            RichFence::Ready(envelope) => envelope,
            RichFence::Pending { kind } => {
                return Ok(ui::Box::new()
                    .width(Length::Pct(100.0))
                    .child(crate::ui::rich::narrative::callout::text(
                        theme,
                        &format!("Drawing {kind}\u{2026}"),
                        TextRole::Label,
                        theme.colors.text_muted,
                    ))
                    .build());
            }
            RichFence::Invalid { .. } => {
                return Err(PresenterError::Unavailable("code fence or rich fallback"));
            }
        };
        if matches!(
            &envelope.block,
            Prepared::Model(_)
                | Prepared::Image(_)
                | Prepared::Audio(_)
                | Prepared::ImageCompare(_)
        ) {
            self.media_invalidations
                .borrow_mut()
                .insert(id.to_owned(), Rc::clone(&invalidate));
        }
        let element = match &envelope.block {
            Prepared::Model(model) => {
                self.render_model(model, entry, envelope, id, theme, glyph, invalidate)?
            }
            Prepared::Image(model) => {
                self.render_image(model, entry, envelope, id, theme, invalidate)?
            }
            Prepared::Audio(block) => {
                self.render_audio(block, entry, envelope, id, theme, glyph, invalidate)?
            }
            Prepared::ImageCompare(model) => {
                self.render_image_compare(model, entry, envelope, id, theme, invalidate)?
            }
            Prepared::AnnotatedCode(block) => self.render_annotated_code(
                block,
                entry,
                envelope,
                id,
                theme,
                invalidate,
                code,
                reference_scope,
            )?,
            Prepared::FileTree(tree) => self.render_file_tree(
                tree,
                entry,
                envelope,
                id,
                theme,
                invalidate,
                code,
                reference_scope,
            )?,
            Prepared::CodeRef(reference) => {
                Self::render_code_ref(reference, entry, envelope, id, theme, code, reference_scope)?
            }
            Prepared::Compare(compare) => {
                self.render_compare(compare, entry, envelope, id, theme, invalidate, heatmap)?
            }
            Prepared::Stats(stats) => {
                self.render_stats(stats, entry, envelope, id, theme, glyph, invalidate)
            }
            Prepared::Callout(callout) => {
                self.render_callout(callout, entry, envelope, id, theme, glyph, invalidate)
            }
            Prepared::Chart(charts::Prepared::Heatmap(spec)) => {
                self.render_heatmap_chart(spec, entry, envelope, id, theme, invalidate, heatmap)?
            }
            Prepared::Chart(charts::Prepared::Funnel(spec)) => {
                self.render_funnel_chart(spec, entry, envelope, id, theme, invalidate, heatmap)?
            }
            Prepared::Chart(charts::Prepared::Spans(layout)) => {
                self.render_spans_chart(layout, entry, envelope, id, theme, invalidate, heatmap)?
            }
            Prepared::Chart(charts::Prepared::Hierarchy(spec)) => {
                self.render_hierarchy_chart(spec, entry, envelope, id, theme, invalidate, heatmap)?
            }
            Prepared::Chart(charts::Prepared::Sankey(graph)) => {
                self.render_sankey_chart(graph, entry, envelope, id, theme, invalidate, heatmap)?
            }
            Prepared::Chart(charts::Prepared::Chart(style, model)) => self.render_line_chart(
                style, model, entry, envelope, id, theme, invalidate, heatmap,
            )?,
            Prepared::Chart(charts::Prepared::Distribution(spec)) => self
                .render_distribution_chart(spec, entry, envelope, id, theme, invalidate, heatmap)?,
            Prepared::Chart(charts::Prepared::Concentration(spec)) => self
                .render_concentration_chart(
                    spec, entry, envelope, id, theme, invalidate, heatmap,
                )?,
        };
        Ok(element)
    }
}
