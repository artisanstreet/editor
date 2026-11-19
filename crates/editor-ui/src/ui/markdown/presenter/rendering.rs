use super::super::visual::{BodyContext, BodyTone, LeafKind, Sources, interactive_leaf, raw_leaf};
use super::super::{Block, CodeFence};
use super::{
    BodyServices, CodeServices, MarkdownPresenter, PreparationProof, PresenterError,
    RenderedMarkdown, RetainedRich, RichCall, SelectionPlace, fence_key,
};
use crate::ui::kit::TextRole;
use crate::ui::rich::charts::{self};
use crate::ui::rich::{Prepared, RichFence};
use crate::ui::theme::{ArtisanTheme, ProseTypography};
use aqua_core::Insets;
use aqua_text::document::SelectionSourceId;
use aqua_ui::element::{Element, ui};
use aqua_ui::style::{Length, Style};
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;
use std::sync::Arc;

type RichQueues<'a> = HashMap<u64, VecDeque<&'a RetainedRich>>;

#[derive(Clone, Copy)]
struct Flow<'a> {
    id: &'a str,
    body: BodyContext<'a>,
    order: &'a [u64],
    services: BodyServices<'a>,
}

struct Spacing {
    top: f32,
    bottom: f32,
}

impl Spacing {
    const fn even(margin: f32) -> Self {
        Self {
            top: margin,
            bottom: margin,
        }
    }
}

impl MarkdownPresenter {
    pub fn render(
        &self,
        id: &str,
        body: BodyContext<'_>,
        place: SelectionPlace<'_>,
        mut source_id: impl FnMut(usize) -> SelectionSourceId,
        services: BodyServices<'_>,
    ) -> Result<RenderedMarkdown, PresenterError> {
        let document = self
            .document
            .as_ref()
            .ok_or(PresenterError::Unavailable("unprepared"))?;
        let flow = Flow {
            id,
            body,
            order: place.order,
            services,
        };
        let mut column = ui::Box::new().layout("column").width(Length::Pct(100.0));
        let mut collected = Vec::new();
        let mut sources = Sources {
            document: place.document,
            next_id: &mut source_id,
            collected: &mut collected,
        };
        let mut previous_bottom: f32 = 0.0;
        let mut previous_zeroes_follower = false;
        let mut rich_fences: RichQueues<'_> = HashMap::new();
        for entry in self.rich.iter().flatten() {
            rich_fences
                .entry(fence_key(&entry.source))
                .or_default()
                .push_back(entry);
        }
        for (index, block) in document.blocks().iter().enumerate() {
            let (spacing, element) =
                self.render_block(flow, index, block, &mut sources, &mut rich_fences)?;
            let gap = if index == 0 {
                0.0
            } else if previous_zeroes_follower {
                previous_bottom
            } else {
                previous_bottom.max(spacing.top)
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
            previous_bottom = spacing.bottom;
            previous_zeroes_follower = matches!(block, Block::Heading { level: 2..=4, .. });
        }
        Ok(RenderedMarkdown {
            element: column.build(),
            selection_sources: collected,
            proof: PreparationProof(Arc::clone(&self.proof)),
        })
    }

    fn render_block(
        &self,
        flow: Flow<'_>,
        index: usize,
        block: &Block,
        sources: &mut Sources<'_>,
        rich_fences: &mut RichQueues<'_>,
    ) -> Result<(Spacing, Element), PresenterError> {
        let mut order = flow.order.to_vec();
        order.push(index as u64);
        Ok(match block {
            Block::Paragraph { spans, .. } => {
                let leaf = interactive_leaf(
                    spans,
                    flow.body,
                    sources.leaf(order, "\n\n"),
                    LeafKind::Prose,
                    Vec::new(),
                )
                .map_err(PresenterError::Inline)?;
                sources.collected.push(leaf.source);
                (
                    Spacing::even(ProseTypography::PARAGRAPH_MARGIN_PX),
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
                (
                    Spacing::even(ProseTypography::FENCE_MARGIN_PX),
                    self.render_rich(entry, flow.id, flow.body, flow.services)?,
                )
            }
            Block::Heading { level, spans, .. } => {
                let leaf = interactive_leaf(
                    spans,
                    flow.body.with_tone(BodyTone::Foreground),
                    sources.leaf(order, "\n\n"),
                    LeafKind::Heading(*level),
                    Vec::new(),
                )
                .map_err(PresenterError::Inline)?;
                sources.collected.push(leaf.source);
                let heading = ProseTypography::heading(*level);
                (
                    Spacing {
                        top: heading.margin_top_px,
                        bottom: heading.margin_bottom_px,
                    },
                    leaf.element,
                )
            }
            Block::Html { source } => {
                let place = sources.leaf(order, "\n");
                let leaf = raw_leaf(
                    source,
                    flow.body.theme,
                    flow.body.tone,
                    place.document,
                    place.id,
                    place.order,
                    place.separator,
                );
                sources.collected.push(leaf.source);
                (
                    Spacing::even(ProseTypography::PARAGRAPH_MARGIN_PX),
                    leaf.element,
                )
            }
            Block::List {
                ordered,
                start,
                items,
                ..
            } => (
                Spacing::even(ProseTypography::LIST_MARGIN_PX),
                super::super::list::render(*ordered, *start, items, flow.body, &order, sources)?,
            ),
            Block::Table(table) => (
                Spacing::even(ProseTypography::TABLE_MARGIN_PX),
                super::super::table_view::render(table, flow.body, &order, sources)?,
            ),
        })
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

    pub(super) fn render_rich(
        &self,
        entry: &RetainedRich,
        id: &str,
        body: BodyContext<'_>,
        services: BodyServices<'_>,
    ) -> Result<Element, PresenterError> {
        let theme = body.theme;
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
                .insert(id.to_owned(), Rc::clone(services.invalidate));
        }
        let call = RichCall {
            entry,
            envelope,
            id,
            theme,
            glyph: services.glyph,
            invalidate: services.invalidate,
            heatmap: services.charts,
            code: services.code,
            reference_scope: body.scope,
        };
        let element = match &envelope.block {
            Prepared::Model(model) => self.render_model(model, call)?,
            Prepared::Image(model) => self.render_image(model, call)?,
            Prepared::Audio(block) => self.render_audio(block, call)?,
            Prepared::ImageCompare(model) => self.render_image_compare(model, call)?,
            Prepared::AnnotatedCode(block) => self.render_annotated_code(block, call)?,
            Prepared::FileTree(tree) => self.render_file_tree(tree, call)?,
            Prepared::CodeRef(reference) => Self::render_code_ref(reference, call)?,
            Prepared::Compare(compare) => self.render_compare(compare, call)?,
            Prepared::Stats(stats) => self.render_stats(stats, call),
            Prepared::Callout(callout) => self.render_callout(callout, call),
            Prepared::Chart(charts::Prepared::Heatmap(spec)) => {
                self.render_heatmap_chart(spec, call)?
            }
            Prepared::Chart(charts::Prepared::Funnel(spec)) => {
                self.render_funnel_chart(spec, call)?
            }
            Prepared::Chart(charts::Prepared::Spans(layout)) => {
                self.render_spans_chart(layout, call)?
            }
            Prepared::Chart(charts::Prepared::Hierarchy(spec)) => {
                self.render_hierarchy_chart(spec, call)?
            }
            Prepared::Chart(charts::Prepared::Sankey(graph)) => {
                self.render_sankey_chart(graph, call)?
            }
            Prepared::Chart(charts::Prepared::Chart(style, model)) => {
                self.render_line_chart(style, model, call)?
            }
            Prepared::Chart(charts::Prepared::Distribution(spec)) => {
                self.render_distribution_chart(spec, call)?
            }
            Prepared::Chart(charts::Prepared::Concentration(spec)) => {
                self.render_concentration_chart(spec, call)?
            }
        };
        Ok(element)
    }
}
