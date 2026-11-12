use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::num::NonZeroUsize;
use std::rc::Rc;
use std::sync::Arc;

use aqua::mailbox::Mailbox;
use aqua_core::{GpuiHsla, Insets};
use aqua_text::document::{SelectionDocumentId, SelectionSource, SelectionSourceId};
use aqua_ui::collection_runtime::{SelectionBatchOwner, SelectionSourceBatch};
use aqua_ui::element::{Element, ui};
use aqua_ui::style::{Length, Style};
use editor_assets::AssetId;

use super::visual::{BodyTone, LeafError, LinkAction, interactive_leaf, raw_leaf};
use super::{Block, CodeFence, MarkdownDocument, MarkdownEngine, MarkdownError};
use crate::ui::kit::TextRole;
use crate::ui::rich::charts::{
    self,
    chart::ChartState,
    concentration::ConcentrationState,
    distribution::DistributionState,
    funnel::{FunnelState, TextRole as FunnelTextRole},
    heatmap::{HeatmapState, TextRole as HeatmapTextRole},
    hierarchy::HierarchyState,
    sankey::SankeyState,
    spans::SpansState,
};
use crate::ui::rich::data::stats::StatsState;
use crate::ui::rich::narrative::callout::{CalloutState, GlyphFactory};
use crate::ui::rich::narrative::compare::CompareState;
use crate::ui::rich::{self, Prepared, RichFence};
use crate::ui::theme::{ArtisanTheme, ProseTypography};

#[derive(Clone)]
pub struct PreparationProof(Arc<()>);

pub struct RenderedMarkdown {
    pub element: Element,
    pub selection_sources: Vec<SelectionSource>,
    pub proof: PreparationProof,
}

pub struct ProjectedTextLeaf {
    pub key: String,
    pub slot: usize,
    pub text: Arc<str>,
}

pub struct ProjectedRevealText {
    pub key: String,
    pub text: Arc<str>,
    pub end: usize,
    pub fades: Vec<super::reveal::Fade>,
}

pub struct ProjectedLeafInfo {
    pub source_range: std::ops::Range<usize>,
    pub list_depth: usize,
    pub proof: PreparationProof,
}

impl ProjectedRevealText {
    pub fn apply_opacity(&self, app: &mut aqua::UiApp, node: aqua_core::NodeId) -> bool {
        let Some(record) = app.tree().get(node) else {
            return false;
        };
        let aqua_ui::tree::NodeKind::Text(text) = &record.kind else {
            return false;
        };
        if text.text() != self.text.as_ref() {
            return false;
        }
        let ranges: Arc<[aqua_core::scene::GpuiTextOpacityRange]> = self
            .fades
            .iter()
            .map(|fade| aqua_core::scene::GpuiTextOpacityRange {
                bytes: fade.range.clone(),
                opacity: fade.opacity,
            })
            .collect();
        app.tree_mut().set_gpui_text_opacity(node, ranges)
    }
}

pub struct ProjectedGeometry {
    pub frame_key: String,
    pub text: Vec<ProjectedTextLeaf>,
    pub tail: Option<usize>,
    pub block_entrance: bool,
}

pub struct ProjectedMarkdown {
    pub element: Element,
    pub selection_sources: Arc<[SelectionSource]>,
    pub selection_batch: Arc<SelectionSourceBatch>,
    pub geometry: ProjectedGeometry,
    pub source_range: std::ops::Range<usize>,
    pub proof: PreparationProof,
}

impl ProjectedMarkdown {
    pub fn reveal_text(&self, reveal: &super::reveal::PreparedReveal) -> Vec<ProjectedRevealText> {
        reveal.set_tail(self.geometry.tail);
        self.geometry
            .text
            .iter()
            .map(|leaf| {
                let fades = reveal.fades(leaf.slot, leaf.text.clone());
                let end = reveal.visible_end(leaf.slot, &leaf.text);
                ProjectedRevealText {
                    key: leaf.key.clone(),
                    text: leaf.text.clone(),
                    end,
                    fades,
                }
            })
            .collect()
    }
}

pub struct StoredProjectedMarkdown {
    pub element: Element,
    pub selection_batch: Arc<SelectionSourceBatch>,
    pub geometry: ProjectedGeometry,
    pub source_range: std::ops::Range<usize>,
    pub list_depth: usize,
    pub proof: super::stored::LeafProof,
}

impl StoredProjectedMarkdown {
    pub fn reveal_text(&self, reveal: &super::reveal::PreparedReveal) -> Vec<ProjectedRevealText> {
        reveal.set_tail(self.geometry.tail);
        self.geometry
            .text
            .iter()
            .map(|leaf| ProjectedRevealText {
                key: leaf.key.clone(),
                text: leaf.text.clone(),
                end: reveal.visible_end(leaf.slot, &leaf.text),
                fades: reveal.fades(leaf.slot, leaf.text.clone()),
            })
            .collect()
    }
}

struct StoredProjectedLeaf {
    leaf: Arc<super::stored::StoredLeaf>,
    document: SelectionDocumentId,
    prefix: Vec<u64>,
    selection_owner: SelectionBatchOwner,
    selection_batch: Arc<SelectionSourceBatch>,
    sources: Arc<[SelectionSource]>,
    fence: Option<super::fence::Fence>,
    rich: Option<RetainedRich>,
    fallback_note: Option<String>,
}

pub struct CodeServices {
    pub copy: Rc<dyn Fn(&str, &str) -> Element>,
    pub copy_enabled: bool,
    pub reference: Option<ReferenceServices>,
}

pub struct ReferenceServices {
    pub instance: Option<editor_core::HostInstance>,
    pub open:
        Rc<dyn Fn(crate::ui::rich::actions::OpenReference, Option<editor_core::HostInstance>)>,
}

#[derive(Clone)]
pub struct MediaServices {
    pub width: f32,
    pub fetcher: crate::ui::media::source::MediaFetcher,
    pub images: crate::ui::media::image::Images,
    pub player: editor_audio::Player,
    pub audio_assets: crate::ui::media::audio::AudioAssets,
    pub model_assets: crate::ui::media::model::load::ModelAssets,
    pub image_assets: aqua::ImageAssets,
    pub model_capability: Rc<dyn Fn() -> Option<crate::ui::media::model::load::RendererCapability>>,
    pub scale: f32,
    pub effect: Rc<dyn Fn(crate::ui::media::preview::Effect)>,
    pub wake: aqua::mailbox::Wake,
}

pub struct HeatmapServices {
    pub empty: Rc<dyn Fn(&str) -> Element>,
    pub readout: Rc<dyn Fn(f32, &str, &str) -> Element>,
    pub navigation_frame: Rc<dyn Fn(Element, GpuiHsla) -> Element>,
    pub concentration: ConcentrationServices,
}

pub struct ConcentrationServices {
    pub frame: Rc<dyn Fn(Element, &str, &str) -> Element>,
    pub plot_label: Rc<dyn Fn(f32, f32, &str, GpuiHsla) -> Element>,
    pub readout: Rc<dyn Fn(f32, &str, &str, &str, GpuiHsla) -> Element>,
}

impl ConcentrationServices {
    pub fn new(theme: ArtisanTheme) -> Self {
        Self {
            frame: Rc::new(move |plot, y, x| {
                charts::frame::concentration_frame(&theme, plot, y, x)
            }),
            plot_label: Rc::new(move |x, y, label, color| {
                charts::frame::plot_label(&theme, x, y, label, color)
            }),
            readout: Rc::new(move |x, heading, label, value, color| {
                charts::frame::readout_row(&theme, x, heading, label, value, Some(color))
            }),
        }
    }
}

impl HeatmapServices {
    pub fn new(theme: ArtisanTheme) -> Self {
        Self {
            empty: Rc::new(move |label| charts::frame::empty(&theme, label)),
            readout: Rc::new(move |x, heading, value| {
                charts::frame::readout(&theme, x, heading, value)
            }),
            navigation_frame: Rc::new(charts::frame::navigation),
            concentration: ConcentrationServices::new(theme),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PresenterError {
    #[error(transparent)]
    Parse(#[from] MarkdownError),
    #[error(transparent)]
    Index(#[from] super::MarkdownIndexError),
    #[error("Markdown inline rendering is unavailable: {0:?}")]
    Inline(LeafError),
    #[error("Markdown block rendering is unavailable: {0}")]
    Unavailable(&'static str),
}

pub struct MarkdownPresenter {
    source: String,
    document: Option<MarkdownDocument>,
    projected: Option<super::projected::ProjectedDocument>,
    stored_leaves: RefCell<HashMap<(String, usize), StoredProjectedLeaf>>,
    selection_owner: SelectionBatchOwner,
    selection_revision: Cell<u64>,
    rich: Vec<Option<RetainedRich>>,
    next_rich_id: u64,
    proof: Arc<()>,
    leaf_proofs: Vec<(u64, Arc<()>)>,
    wake: Mailbox<()>,
    reveals: RefCell<HashMap<String, HashMap<usize, Rc<super::reveal::PreparedReveal>>>>,
    models: crate::ui::rich::media::model::ModelState,
    model_ids: RefCell<HashMap<String, HashSet<String>>>,
    model_hover: RefCell<HashMap<String, Rc<Cell<bool>>>>,
    audio: crate::ui::rich::media::audio::AudioState,
    audio_ids: RefCell<HashMap<String, HashSet<String>>>,
    media_services: RefCell<HashMap<String, MediaServices>>,
    media_invalidations: RefCell<HashMap<String, Rc<dyn Fn()>>>,
    images: RefCell<
        HashMap<
            String,
            (
                Arc<crate::ui::rich::media::image::ImageBlock>,
                String,
                Rc<RefCell<crate::ui::rich::media::image::ImageState>>,
            ),
        >,
    >,
    image_ids: RefCell<HashMap<String, HashSet<String>>>,
    image_comparisons: RefCell<
        HashMap<
            String,
            (
                String,
                Rc<RefCell<crate::ui::rich::media::image_compare::view::CompareState>>,
            ),
        >,
    >,
    image_comparison_ids: RefCell<HashMap<String, HashSet<String>>>,
    annotated: RefCell<
        HashMap<String, Rc<RefCell<crate::ui::rich::code::annotated::view::AnnotatedState>>>,
    >,
    annotated_ids: RefCell<HashMap<String, HashSet<String>>>,
    file_trees:
        RefCell<HashMap<String, Rc<RefCell<crate::ui::rich::code::file_tree::FileTreeState>>>>,
    file_tree_ids: RefCell<HashMap<String, HashSet<String>>>,
    file_tree_focus: Rc<RefCell<HashMap<String, (String, String, usize)>>>,
    comparisons: CompareState,
    comparison_ids: RefCell<HashMap<String, HashSet<String>>>,
    stats: StatsState,
    stats_ids: RefCell<HashMap<String, HashSet<String>>>,
    callouts: CalloutState,
    callout_ids: RefCell<HashMap<String, HashSet<String>>>,
    heatmaps: HeatmapState,
    heatmap_ids: RefCell<HashMap<String, HashSet<String>>>,
    funnels: FunnelState,
    funnel_ids: RefCell<HashMap<String, HashSet<String>>>,
    spans: SpansState,
    spans_ids: RefCell<HashMap<String, HashSet<String>>>,
    hierarchies: HierarchyState,
    hierarchy_ids: RefCell<HashMap<String, HashSet<String>>>,
    sankeys: SankeyState,
    sankey_ids: RefCell<HashMap<String, HashSet<String>>>,
    charts: ChartState,
    chart_ids: RefCell<HashMap<String, HashSet<String>>>,
    concentrations: ConcentrationState,
    distributions: DistributionState,
    distribution_ids: RefCell<HashMap<String, HashSet<String>>>,
    concentration_ids: RefCell<HashMap<String, HashSet<String>>>,
    copies: Rc<RefCell<HashMap<String, HashMap<String, crate::ui::code_copy::CodeCopyController>>>>,
    diagrams: RefCell<HashMap<String, Rc<RefCell<super::diagram::view::DiagramState>>>>,
}

struct RetainedRich {
    id: u64,
    source: CodeFence,
    prepared: RichFence,
}

impl Default for MarkdownPresenter {
    fn default() -> Self {
        Self {
            source: String::new(),
            document: None,
            projected: None,
            stored_leaves: RefCell::new(HashMap::new()),
            selection_owner: SelectionBatchOwner::new(),
            selection_revision: Cell::new(0),
            rich: Vec::new(),
            next_rich_id: 0,
            proof: Arc::new(()),
            leaf_proofs: Vec::new(),
            wake: Mailbox::new(NonZeroUsize::MIN),
            reveals: RefCell::new(HashMap::new()),
            models: crate::ui::rich::media::model::ModelState::default(),
            model_ids: RefCell::new(HashMap::new()),
            model_hover: RefCell::new(HashMap::new()),
            audio: crate::ui::rich::media::audio::AudioState::default(),
            audio_ids: RefCell::new(HashMap::new()),
            media_services: RefCell::new(HashMap::new()),
            media_invalidations: RefCell::new(HashMap::new()),
            images: RefCell::new(HashMap::new()),
            image_ids: RefCell::new(HashMap::new()),
            image_comparisons: RefCell::new(HashMap::new()),
            image_comparison_ids: RefCell::new(HashMap::new()),
            annotated: RefCell::new(HashMap::new()),
            annotated_ids: RefCell::new(HashMap::new()),
            file_trees: RefCell::new(HashMap::new()),
            file_tree_ids: RefCell::new(HashMap::new()),
            file_tree_focus: Rc::new(RefCell::new(HashMap::new())),
            comparisons: CompareState::default(),
            comparison_ids: RefCell::new(HashMap::new()),
            stats: StatsState::default(),
            stats_ids: RefCell::new(HashMap::new()),
            callouts: CalloutState::default(),
            callout_ids: RefCell::new(HashMap::new()),
            heatmaps: HeatmapState::default(),
            heatmap_ids: RefCell::new(HashMap::new()),
            funnels: FunnelState::default(),
            funnel_ids: RefCell::new(HashMap::new()),
            spans: SpansState::default(),
            spans_ids: RefCell::new(HashMap::new()),
            hierarchies: HierarchyState::default(),
            hierarchy_ids: RefCell::new(HashMap::new()),
            sankeys: SankeyState::default(),
            sankey_ids: RefCell::new(HashMap::new()),
            charts: ChartState::default(),
            chart_ids: RefCell::new(HashMap::new()),
            concentrations: ConcentrationState::default(),
            distributions: DistributionState::default(),
            distribution_ids: RefCell::new(HashMap::new()),
            concentration_ids: RefCell::new(HashMap::new()),
            copies: Rc::new(RefCell::new(HashMap::new())),
            diagrams: RefCell::new(HashMap::new()),
        }
    }
}

impl MarkdownPresenter {
    pub fn prepare(
        &mut self,
        engine: &MarkdownEngine,
        source: &str,
    ) -> Result<PreparationProof, PresenterError> {
        self.prepare_with(engine, source, None)
    }

    pub fn prepare_message(
        &mut self,
        engine: &MarkdownEngine,
        source: &str,
    ) -> Result<PreparationProof, PresenterError> {
        let command = artisan_mentions::special_command(source)
            .and_then(|range| source.get(range))
            .map(str::to_owned);
        self.prepare_with(engine, source, command)
    }

    fn prepare_with(
        &mut self,
        engine: &MarkdownEngine,
        source: &str,
        command: Option<String>,
    ) -> Result<PreparationProof, PresenterError> {
        if self.document.is_none() || self.source != source {
            self.proof = Arc::new(());
            self.document = None;
            self.projected = None;
            let document = engine.parse_document(source)?;
            let mut projected =
                super::projected::ProjectedDocument::new(engine, source, command.as_deref())?;
            let mut previous = std::mem::take(&mut self.rich);
            let mut next: Vec<Option<RetainedRich>> =
                (0..document.blocks().len()).map(|_| None).collect();
            for (index, block) in document.blocks().iter().enumerate() {
                let Block::Code(fence) = block else {
                    continue;
                };
                let found = previous.iter().position(|entry| {
                    entry.as_ref().is_some_and(|entry| {
                        entry.source.language == fence.language
                            && entry.source.source == fence.source
                            && entry.source.closed == fence.closed
                    })
                });
                if let Some(found) = found {
                    next[index] = previous[found].take();
                }
            }
            for (index, block) in document.blocks().iter().enumerate() {
                if next[index].is_some() {
                    continue;
                }
                let Block::Code(fence) = block else {
                    continue;
                };
                let Some(prepared) =
                    rich::prepare(fence.language.as_deref(), &fence.source, fence.closed)
                else {
                    continue;
                };
                let old_entry = previous.get_mut(index).and_then(|entry| {
                    if entry
                        .as_ref()
                        .is_some_and(|entry| entry.source.language == fence.language)
                    {
                        entry.take()
                    } else {
                        None
                    }
                });
                let previous_id = old_entry.and_then(|entry| {
                    if matches!(&entry.prepared, RichFence::Ready(_))
                        && !matches!(&prepared, RichFence::Ready(_))
                    {
                        self.remove_rich(entry.id);
                        None
                    } else {
                        Some(entry.id)
                    }
                });
                let id = previous_id.unwrap_or_else(|| {
                    let id = self.next_rich_id;
                    self.next_rich_id = self.next_rich_id.wrapping_add(1);
                    id
                });
                next[index] = Some(RetainedRich {
                    id,
                    source: fence.clone(),
                    prepared,
                });
            }
            for removed in previous.into_iter().flatten() {
                self.remove_rich(removed.id);
            }
            self.rich = next;
            projected.bind_rich(
                self.rich.iter().enumerate().filter_map(|(index, entry)| {
                    entry.as_ref().map(|entry| (index, &entry.source))
                }),
            );
            let previous_proofs = std::mem::take(&mut self.leaf_proofs);
            self.leaf_proofs = (0..projected.len())
                .map(|row| {
                    let signature = projected.signature(row).unwrap_or_default();
                    match previous_proofs.get(row) {
                        Some((previous, proof)) if *previous == signature => {
                            (signature, proof.clone())
                        }
                        _ => (signature, Arc::new(())),
                    }
                })
                .collect();
            self.source.clear();
            self.source.push_str(source);
            self.document = Some(document);
            self.projected = Some(projected);
        }
        Ok(PreparationProof(self.proof.clone()))
    }

    fn remove_rich(&self, rich_id: u64) {
        let remove = |ids: &RefCell<HashMap<String, HashSet<String>>>,
                      suffix: &str,
                      remove: &dyn Fn(&str)| {
            for (owner, keys) in ids.borrow_mut().iter_mut() {
                let key = format!("{owner}-rich-{rich_id}-{suffix}");
                if keys.remove(&key) {
                    remove(&key);
                }
            }
        };
        remove(&self.model_ids, "model", &|key| {
            self.models.release(key);
            self.model_hover.borrow_mut().remove(key);
        });
        remove(&self.image_ids, "image", &|key| {
            self.images.borrow_mut().remove(key);
        });
        remove(&self.audio_ids, "audio", &|key| self.audio.release(key));
        remove(&self.image_comparison_ids, "image-compare", &|key| {
            self.image_comparisons.borrow_mut().remove(key);
        });
        remove(&self.annotated_ids, "annotated", &|key| {
            self.annotated.borrow_mut().remove(key);
        });
        remove(&self.file_tree_ids, "file-tree", &|key| {
            self.remove_file_tree(key)
        });
        remove(&self.comparison_ids, "compare", &|key| {
            self.comparisons.remove(key)
        });
        remove(&self.stats_ids, "stats", &|key| self.stats.remove(key));
        remove(&self.callout_ids, "callout", &|key| {
            self.callouts.remove(key)
        });
        remove(&self.heatmap_ids, "heatmap", &|key| {
            self.heatmaps.remove(key)
        });
        remove(&self.funnel_ids, "funnel", &|key| self.funnels.remove(key));
        remove(&self.spans_ids, "spans", &|key| self.spans.remove(key));
        remove(&self.hierarchy_ids, "hierarchy", &|key| {
            self.hierarchies.remove(key)
        });
        remove(&self.sankey_ids, "sankey", &|key| self.sankeys.remove(key));
        remove(&self.chart_ids, "chart", &|key| self.charts.remove(key));
        remove(&self.distribution_ids, "distribution", &|key| {
            self.distributions.remove(key)
        });
        remove(&self.concentration_ids, "concentration", &|key| {
            self.concentrations.remove(key)
        });
    }

    fn leaf_is_current(&self, row: usize, proof: &PreparationProof) -> bool {
        self.document.is_some()
            && self
                .leaf_proofs
                .get(row)
                .is_some_and(|(_, current)| Arc::ptr_eq(current, &proof.0))
    }

    pub fn is_current(&self, proof: &PreparationProof) -> bool {
        self.document.is_some() && Arc::ptr_eq(&self.proof, &proof.0)
    }

    pub fn wake_mailbox(&self) -> &Mailbox<()> {
        &self.wake
    }

    pub fn release_stored_leaf(&self, id: &str, ordinal: usize) {
        let removed = self
            .stored_leaves
            .borrow_mut()
            .remove(&(id.to_owned(), ordinal));
        if let Some(rich) = removed.and_then(|entry| entry.rich) {
            self.remove_rich(rich.id);
        }
    }

    pub fn prepare_stored_leaf(
        &mut self,
        owner: &super::stored::StoredOwner,
        id: &str,
        ordinal: usize,
        leaf: Arc<super::stored::StoredLeaf>,
        theme: &ArtisanTheme,
        document: SelectionDocumentId,
        prefix: &[u64],
        mut source_id: impl FnMut(usize) -> SelectionSourceId,
    ) -> Result<(), PresenterError> {
        if leaf.proof().owner() != id
            || usize::try_from(leaf.descriptor.ordinal).ok() != Some(ordinal)
            || !owner.leaf_is_current(leaf.proof())
        {
            return Err(PresenterError::Unavailable("stale stored leaf"));
        }
        let key = (id.to_owned(), ordinal);
        if self.stored_leaves.borrow().get(&key).is_some_and(|cached| {
            Arc::ptr_eq(&cached.leaf, &leaf)
                && cached.document == document
                && cached.prefix == prefix
        }) {
            return Ok(());
        }
        let mut rich = None;
        let mut fallback_note = None;
        let mut rich_without_source = false;
        let mut fence = None;
        if let super::IndexedLeafContent::Block(Block::Code(code)) = &leaf.content {
            if let Some(prepared) =
                rich::prepare(code.language.as_deref(), &code.source, code.closed)
            {
                rich_without_source =
                    matches!(prepared, RichFence::Ready(_) | RichFence::Pending { .. });
                if let RichFence::Invalid { note } = &prepared {
                    fallback_note = Some(note.clone());
                }
                let id = self.next_rich_id;
                self.next_rich_id = self.next_rich_id.wrapping_add(1);
                rich = Some(RetainedRich {
                    id,
                    source: code.clone(),
                    prepared,
                });
            }
            rich_without_source |= code.diagram.is_some();
            if !rich_without_source {
                fence = Some(super::fence::Fence::new(code));
            }
        }
        let (sources, bytes) = super::projected::stored_leaf_sources(
            &leaf,
            ordinal,
            theme,
            prefix,
            &mut source_id,
            fence.as_ref(),
            rich_without_source,
        )?;
        let bytes = bytes.saturating_add(rich.as_ref().map_or(0, |rich| {
            std::mem::size_of::<RetainedRich>()
                + rich.source.source.capacity()
                + rich.source.language.as_ref().map_or(0, String::capacity)
                + rich.prepared.resident_bytes()
        }));
        if bytes > super::stored::PREPARED_BYTES / 2 {
            return Err(PresenterError::Unavailable(
                "Markdown leaf exceeds its normalized-memory budget; open the full source",
            ));
        }
        let selection_owner = SelectionBatchOwner::new();
        let selection_batch = Arc::new(SelectionSourceBatch::new(
            document,
            selection_owner,
            1,
            sources.clone(),
        ));
        let previous = self.stored_leaves.borrow_mut().insert(
            key,
            StoredProjectedLeaf {
                leaf,
                document,
                prefix: prefix.to_vec(),
                selection_owner,
                selection_batch,
                sources,
                fence,
                rich,
                fallback_note,
            },
        );
        if let Some(previous) = previous.and_then(|entry| entry.rich) {
            self.remove_rich(previous.id);
        }
        Ok(())
    }

    pub fn render_stored_leaf(
        &self,
        owner: &super::stored::StoredOwner,
        id: &str,
        ordinal: usize,
        slot_start: usize,
        final_row: bool,
        theme: &ArtisanTheme,
        tone: BodyTone,
        glyph: Rc<dyn Fn(AssetId, f32, GpuiHsla) -> Element>,
        invalidate: Rc<dyn Fn()>,
        on_link: Option<Rc<dyn Fn(LinkAction)>>,
        on_color: Option<Rc<dyn Fn(crate::ui::rich::actions::ShowCopyMenu)>>,
        scope: Option<crate::ui::rich::actions::ResourceScope>,
        charts: Option<&HeatmapServices>,
        code: Option<&CodeServices>,
    ) -> Result<StoredProjectedMarkdown, PresenterError> {
        let entries = self.stored_leaves.borrow();
        let entry = entries
            .get(&(id.to_owned(), ordinal))
            .ok_or(PresenterError::Unavailable("unprepared stored leaf"))?;
        if !owner.leaf_is_current(entry.leaf.proof()) {
            return Err(PresenterError::Unavailable("stale stored leaf"));
        }
        let rich_scope = scope.clone();
        let rich = |index: usize, fence: &CodeFence| {
            if fence.diagram.is_some() {
                return Ok(self.render_diagram(id, index, fence, theme, &invalidate, code));
            }
            self.render_rich(
                entry
                    .rich
                    .as_ref()
                    .ok_or(PresenterError::Unavailable("nested rich block"))?,
                id,
                theme,
                glyph.clone(),
                invalidate.clone(),
                charts,
                code,
                rich_scope.clone(),
            )
        };
        let color_action = on_color.clone();
        let ordinary = |row: usize,
                        fence: &super::fence::Fence,
                        sources: &[SelectionSource],
                        note: Option<&str>| {
            let services = code.ok_or(PresenterError::Unavailable("code copy services"))?;
            let card = fence.render(
                &format!("{id}-code-{row}"),
                theme,
                entry.document,
                sources,
                services.copy_enabled.then(|| services.copy.clone()),
                color_action.clone(),
            );
            Ok(if let Some(note) = note {
                ui::Box::new()
                    .layout("column")
                    .width(Length::Pct(100.0))
                    .gap(6.0)
                    .child(card)
                    .child(crate::ui::rich::narrative::callout::text(
                        theme,
                        note,
                        TextRole::Label,
                        theme.colors.text_muted,
                    ))
                    .build()
            } else {
                card
            })
        };
        let reveal = self
            .reveals
            .borrow()
            .get(id)
            .and_then(|rows| rows.get(&ordinal))
            .cloned();
        let (element, geometry) = super::projected::render_stored_leaf(
            &entry.leaf,
            ordinal,
            &entry.sources,
            slot_start,
            final_row,
            theme,
            tone,
            entry.document,
            reveal.as_deref(),
            entry.selection_owner,
            &entry.prefix,
            on_link,
            on_color,
            scope,
            entry.fence.as_ref(),
            entry.fallback_note.as_deref(),
            Some(&rich),
            Some(&ordinary),
        )?;
        let start = usize::try_from(entry.leaf.descriptor.source_start)
            .map_err(|_| PresenterError::Unavailable("stored source range"))?;
        let end = usize::try_from(entry.leaf.descriptor.source_end)
            .map_err(|_| PresenterError::Unavailable("stored source range"))?;
        Ok(StoredProjectedMarkdown {
            element,
            selection_batch: entry.selection_batch.clone(),
            geometry,
            source_range: start..end,
            list_depth: entry.leaf.context.lists.len(),
            proof: entry.leaf.proof().clone(),
        })
    }

    pub fn projected_count(&self) -> Option<usize> {
        self.projected
            .as_ref()
            .map(super::projected::ProjectedDocument::len)
    }

    pub fn projected_range(&self, row: usize) -> Option<std::ops::Range<usize>> {
        self.projected.as_ref()?.range(row)
    }

    pub fn projected_leaf_info(&self, row: usize) -> Option<ProjectedLeafInfo> {
        let projected = self.projected.as_ref()?;
        Some(ProjectedLeafInfo {
            source_range: projected.range(row)?,
            list_depth: projected.depth(row)?,
            proof: self.leaf_proof(row)?,
        })
    }

    fn leaf_proof(&self, row: usize) -> Option<PreparationProof> {
        self.leaf_proofs
            .get(row)
            .map(|(_, proof)| PreparationProof(proof.clone()))
    }

    pub fn projected_leaf_is_current(&self, row: usize, info: &ProjectedLeafInfo) -> bool {
        self.leaf_is_current(row, &info.proof)
            && self.projected.as_ref().is_some_and(|projected| {
                projected.range(row).as_ref() == Some(&info.source_range)
                    && projected.depth(row) == Some(info.list_depth)
            })
    }

    pub fn projected_is_current(
        &self,
        row: usize,
        source_range: &std::ops::Range<usize>,
        proof: &PreparationProof,
    ) -> bool {
        self.leaf_is_current(row, proof) && self.projected_range(row).as_ref() == Some(source_range)
    }

    pub fn render_projected(
        &self,
        row: usize,
        theme: &ArtisanTheme,
        tone: BodyTone,
        document: SelectionDocumentId,
        order: &[u64],
        mut source_id: impl FnMut(usize) -> SelectionSourceId,
        on_link: Option<Rc<dyn Fn(LinkAction)>>,
        on_color: Option<Rc<dyn Fn(crate::ui::rich::actions::ShowCopyMenu)>>,
        scope: Option<crate::ui::rich::actions::ResourceScope>,
    ) -> Result<ProjectedMarkdown, PresenterError> {
        let projected = self
            .projected
            .as_ref()
            .ok_or(PresenterError::Unavailable("unprepared"))?;
        let source_range = projected
            .range(row)
            .ok_or(PresenterError::Unavailable("projected row"))?;
        let (element, selection_sources, selection_batch, geometry) = projected.render(
            row,
            theme,
            tone,
            document,
            None,
            None,
            self.selection_owner,
            &self.selection_revision,
            order,
            &mut source_id,
            on_link,
            on_color,
            scope,
            None,
            None,
        )?;
        Ok(ProjectedMarkdown {
            element,
            selection_sources,
            selection_batch,
            geometry,
            source_range,
            proof: self
                .leaf_proof(row)
                .ok_or(PresenterError::Unavailable("projected row"))?,
        })
    }

    pub fn render_projected_with_services(
        &self,
        row: usize,
        id: &str,
        theme: &ArtisanTheme,
        tone: BodyTone,
        document: SelectionDocumentId,
        order: &[u64],
        source_id: impl FnMut(usize) -> SelectionSourceId,
        glyph: Rc<dyn Fn(AssetId, f32, GpuiHsla) -> Element>,
        invalidate: Rc<dyn Fn()>,
        on_link: Option<Rc<dyn Fn(LinkAction)>>,
        on_color: Option<Rc<dyn Fn(crate::ui::rich::actions::ShowCopyMenu)>>,
        scope: Option<crate::ui::rich::actions::ResourceScope>,
        charts: Option<&HeatmapServices>,
    ) -> Result<ProjectedMarkdown, PresenterError> {
        self.render_projected_with_code(
            row, id, theme, tone, document, order, source_id, glyph, invalidate, on_link, on_color,
            scope, charts, None,
        )
    }

    pub fn render_projected_with_code(
        &self,
        row: usize,
        id: &str,
        theme: &ArtisanTheme,
        tone: BodyTone,
        document: SelectionDocumentId,
        order: &[u64],
        mut source_id: impl FnMut(usize) -> SelectionSourceId,
        glyph: Rc<dyn Fn(AssetId, f32, GpuiHsla) -> Element>,
        invalidate: Rc<dyn Fn()>,
        on_link: Option<Rc<dyn Fn(LinkAction)>>,
        on_color: Option<Rc<dyn Fn(crate::ui::rich::actions::ShowCopyMenu)>>,
        scope: Option<crate::ui::rich::actions::ResourceScope>,
        charts: Option<&HeatmapServices>,
        code: Option<&CodeServices>,
    ) -> Result<ProjectedMarkdown, PresenterError> {
        let projected = self
            .projected
            .as_ref()
            .ok_or(PresenterError::Unavailable("unprepared"))?;
        let source_range = projected
            .range(row)
            .ok_or(PresenterError::Unavailable("projected row"))?;
        let reference_scope = scope.clone();
        let rich = |index: usize, fence: &CodeFence| {
            if fence.diagram.is_some() {
                return Ok(self.render_diagram(id, index, fence, theme, &invalidate, code));
            }
            let entry = self
                .rich
                .get(index)
                .and_then(Option::as_ref)
                .ok_or(PresenterError::Unavailable("nested rich block"))?;
            self.render_rich(
                entry,
                id,
                theme,
                glyph.clone(),
                invalidate.clone(),
                charts,
                code,
                reference_scope.clone(),
            )
        };
        let color_action = on_color.clone();
        let ordinary = |ordinal: usize,
                        fence: &super::fence::Fence,
                        sources: &[SelectionSource],
                        note: Option<&str>| {
            let services = code.ok_or(PresenterError::Unavailable("code copy services"))?;
            let card = fence.render(
                &format!("{id}-code-{ordinal}"),
                theme,
                document,
                sources,
                services.copy_enabled.then(|| services.copy.clone()),
                color_action.clone(),
            );
            Ok(if let Some(note) = note {
                ui::Box::new()
                    .layout("column")
                    .width(Length::Pct(100.0))
                    .gap(6.0)
                    .child(card)
                    .child(crate::ui::rich::narrative::callout::text(
                        theme,
                        note,
                        TextRole::Label,
                        theme.colors.text_muted,
                    ))
                    .build()
            } else {
                card
            })
        };
        let reveal = self
            .reveals
            .borrow()
            .get(id)
            .and_then(|rows| rows.get(&row))
            .cloned();
        let (element, selection_sources, selection_batch, geometry) = projected.render(
            row,
            theme,
            tone,
            document,
            Some(id),
            reveal.as_deref(),
            self.selection_owner,
            &self.selection_revision,
            order,
            &mut source_id,
            on_link,
            on_color,
            scope,
            Some(&rich),
            Some(&ordinary),
        )?;
        Ok(ProjectedMarkdown {
            element,
            selection_sources,
            selection_batch,
            geometry,
            source_range,
            proof: self
                .leaf_proof(row)
                .ok_or(PresenterError::Unavailable("projected row"))?,
        })
    }

    #[cfg(feature = "test-support")]
    pub fn rich_identities(&self) -> Vec<u64> {
        self.rich.iter().flatten().map(|entry| entry.id).collect()
    }

    pub fn heatmap_reveals(&self, id: &str) -> Vec<(String, String, usize)> {
        self.rich.iter().filter_map(|block| {
            let entry = block.as_ref()?;
            if !matches!(&entry.prepared, RichFence::Ready(envelope) if matches!(envelope.block, Prepared::Chart(charts::Prepared::Heatmap(_)))) { return None; }
            let block_id = format!("{id}-rich-{}-heatmap", entry.id);
            let (key, row) = self.heatmaps.pending_reveal(&block_id)?;
            Some((block_id, key, row))
        }).collect()
    }

    pub fn heatmap_revealed(&self, id: &str, key: &str, row: usize) {
        self.heatmaps.revealed(id, key, row);
    }

    pub fn funnel_reveals(&self, id: &str) -> Vec<(String, String, usize)> {
        self.rich.iter().filter_map(|block| {
            let entry = block.as_ref()?;
            if !matches!(&entry.prepared, RichFence::Ready(envelope) if matches!(envelope.block, Prepared::Chart(charts::Prepared::Funnel(_)))) { return None; }
            let block_id = format!("{id}-rich-{}-funnel", entry.id);
            let (key, row) = self.funnels.pending_reveal(&block_id)?;
            Some((block_id, key, row))
        }).collect()
    }

    pub fn funnel_revealed(&self, id: &str, key: &str, row: usize) {
        self.funnels.revealed(id, key, row);
    }

    pub fn distribution_reveals(&self, id: &str) -> Vec<(String, String, usize)> {
        self.rich.iter().filter_map(|block| {
            let entry = block.as_ref()?;
            if !matches!(&entry.prepared, RichFence::Ready(envelope) if matches!(envelope.block, Prepared::Chart(charts::Prepared::Distribution(_)))) { return None; }
            let block_id = format!("{id}-rich-{}-distribution", entry.id);
            let (key, row) = self.distributions.pending_reveal(&block_id)?;
            Some((block_id, key, row))
        }).collect()
    }

    pub fn distribution_revealed(&self, id: &str, key: &str, row: usize) {
        self.distributions.revealed(id, key, row);
    }

    pub fn spans_reveals(&self, id: &str) -> Vec<(String, String, usize)> {
        self.rich.iter().filter_map(|block| {
            let entry = block.as_ref()?;
            if !matches!(&entry.prepared, RichFence::Ready(envelope) if matches!(envelope.block, Prepared::Chart(charts::Prepared::Spans(_)))) { return None; }
            let block_id = format!("{id}-rich-{}-spans", entry.id);
            let (key, row) = self.spans.pending_reveal(&block_id)?;
            Some((block_id, key, row))
        }).collect()
    }

    pub fn spans_revealed(&self, id: &str, key: &str, row: usize) {
        self.spans.revealed(id, key, row);
    }

    fn remove_file_tree(&self, id: &str) {
        if let Some(state) = self.file_trees.borrow_mut().remove(id) {
            state.borrow_mut().deactivate();
        }
        self.file_tree_focus.borrow_mut().remove(id);
    }

    pub fn file_tree_focus_requests(&self, owner: &str) -> Vec<(String, String, String, usize)> {
        let ids = self.file_tree_ids.borrow();
        let pending = self.file_tree_focus.borrow();
        ids.get(owner)
            .into_iter()
            .flatten()
            .filter_map(|block| {
                let (collection, row, index) = pending.get(block)?;
                Some((block.clone(), collection.clone(), row.clone(), *index))
            })
            .collect()
    }

    pub fn file_tree_focused(&self, block: &str, collection: &str, row: &str, index: usize) {
        let mut pending = self.file_tree_focus.borrow_mut();
        if pending
            .get(block)
            .is_some_and(|(c, r, i)| c == collection && r == row && *i == index)
        {
            pending.remove(block);
        }
    }

    pub fn set_projected_reveal(
        &self,
        owner: &str,
        row: usize,
        reveal: Option<Rc<super::reveal::PreparedReveal>>,
    ) {
        let mut reveals = self.reveals.borrow_mut();
        if let Some(reveal) = reveal {
            reveals
                .entry(owner.to_owned())
                .or_default()
                .insert(row, reveal);
        } else if let Some(rows) = reveals.get_mut(owner) {
            rows.remove(&row);
            if rows.is_empty() {
                reveals.remove(owner);
            }
        }
    }

    pub fn bind_media(&self, ctx: &aqua::AppCtx<'_>, window: aqua::prelude::AquaWindowId) {
        self.audio.bind(ctx, window);
        self.models.bind(ctx, window);
    }

    pub fn drain_media(&self) -> bool {
        let mut changed = self.audio.drain();
        if let Some(services) = self.media_services.borrow().values().next() {
            changed |= self.models.drain(&services.image_assets);
        }
        for (_, _, state) in self.images.borrow().values() {
            changed |= state.borrow_mut().refresh();
        }
        for (_, state) in self.image_comparisons.borrow().values() {
            changed |= state.borrow_mut().refresh();
        }
        if changed {
            let invalidations: Vec<_> = self
                .media_invalidations
                .borrow()
                .values()
                .cloned()
                .collect();
            for invalidate in invalidations {
                invalidate();
            }
        }
        changed
    }

    pub fn set_media_services(
        &self,
        owner: &str,
        services: MediaServices,
    ) -> Result<(), PresenterError> {
        if !services.width.is_finite() || services.width <= 0.0 {
            return Err(PresenterError::Unavailable("measured media width"));
        }
        self.media_services
            .borrow_mut()
            .insert(owner.to_owned(), services);
        Ok(())
    }

    pub fn release(&self, id: &str) {
        self.stored_leaves
            .borrow_mut()
            .retain(|(owner, _), _| owner != id);
        self.reveals.borrow_mut().remove(id);
        self.media_services.borrow_mut().remove(id);
        self.media_invalidations.borrow_mut().remove(id);
        let diagram_prefix = format!("{id}-diagram-");
        self.diagrams
            .borrow_mut()
            .retain(|key, _| !key.starts_with(&diagram_prefix));
        if let Some(keys) = self.model_ids.borrow_mut().remove(id) {
            for key in keys {
                self.models.release(&key);
                self.model_hover.borrow_mut().remove(&key);
            }
        }
        if let Some(keys) = self.image_ids.borrow_mut().remove(id) {
            for key in keys {
                self.images.borrow_mut().remove(&key);
            }
        }
        if let Some(keys) = self.audio_ids.borrow_mut().remove(id) {
            for key in keys {
                self.audio.release(&key);
            }
        }
        if let Some(keys) = self.image_comparison_ids.borrow_mut().remove(id) {
            for key in keys {
                self.image_comparisons.borrow_mut().remove(&key);
            }
        }
        self.copies.borrow_mut().remove(id);
        if let Some(keys) = self.annotated_ids.borrow_mut().remove(id) {
            for key in keys {
                self.annotated.borrow_mut().remove(&key);
            }
        }
        if let Some(keys) = self.file_tree_ids.borrow_mut().remove(id) {
            for key in keys {
                self.remove_file_tree(&key);
            }
        }
        if let Some(keys) = self.comparison_ids.borrow_mut().remove(id) {
            for key in keys {
                self.comparisons.remove(&key);
            }
        }
        if let Some(keys) = self.stats_ids.borrow_mut().remove(id) {
            for key in keys {
                self.stats.remove(&key);
            }
        }
        if let Some(keys) = self.callout_ids.borrow_mut().remove(id) {
            for key in keys {
                self.callouts.remove(&key);
            }
        }
        if let Some(keys) = self.heatmap_ids.borrow_mut().remove(id) {
            for key in keys {
                self.heatmaps.remove(&key);
            }
        }
        if let Some(keys) = self.funnel_ids.borrow_mut().remove(id) {
            for key in keys {
                self.funnels.remove(&key);
            }
        }
        if let Some(keys) = self.spans_ids.borrow_mut().remove(id) {
            for key in keys {
                self.spans.remove(&key);
            }
        }
        if let Some(keys) = self.hierarchy_ids.borrow_mut().remove(id) {
            for key in keys {
                self.hierarchies.remove(&key);
            }
        }
        if let Some(keys) = self.sankey_ids.borrow_mut().remove(id) {
            for key in keys {
                self.sankeys.remove(&key);
            }
        }
        if let Some(keys) = self.chart_ids.borrow_mut().remove(id) {
            for key in keys {
                self.charts.remove(&key);
            }
        }
        if let Some(keys) = self.distribution_ids.borrow_mut().remove(id) {
            for key in keys {
                self.distributions.remove(&key);
            }
        }
        if let Some(keys) = self.concentration_ids.borrow_mut().remove(id) {
            for key in keys {
                self.concentrations.remove(&key);
            }
        }
    }

    pub fn code_services(
        &self,
        owner: &str,
        theme: ArtisanTheme,
        reduced_motion: bool,
        effect: Rc<dyn Fn(crate::ui::code_copy::CopyEffect)>,
        copy_enabled: bool,
        reference: Option<ReferenceServices>,
    ) -> CodeServices {
        let states = self.copies.clone();
        let owner = owner.to_owned();
        CodeServices {
            copy_enabled,
            reference,
            copy: Rc::new(move |id, source| {
                let controller = states
                    .borrow_mut()
                    .entry(owner.clone())
                    .or_default()
                    .entry(id.to_owned())
                    .or_default()
                    .clone();
                let now = std::time::Instant::now();
                if controller.frame(now, reduced_motion).animating {
                    effect(crate::ui::code_copy::CopyEffect::RequestFrame);
                }
                let emit = effect.clone();
                crate::ui::code_copy::CodeCopyButton::new(
                    id.to_owned(),
                    source.to_owned(),
                    theme,
                    controller,
                    reduced_motion,
                    move |event| emit(event),
                )
                .build(now)
            }),
        }
    }

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

    pub fn render_with_actions(
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

    pub fn render_with_context(
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

    fn render_diagram(
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
                    Rc::new(RefCell::new(super::diagram::view::DiagramState::new()))
                }),
        );
        let content = fence.diagram.as_ref().map_or_else(
            || ui::Box::new().build(),
            |diagram| super::diagram::view::view(theme, &block_id, diagram, &state, invalidate),
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

    fn render_rich(
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
                    .style(Style::new().padding_insets(Insets {
                        top: 4.0,
                        bottom: 4.0,
                        ..Insets::default()
                    }))
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
                .insert(id.to_owned(), invalidate.clone());
        }
        let element = match &envelope.block {
            Prepared::Model(model) => {
                let media = self
                    .media_services
                    .borrow()
                    .get(id)
                    .cloned()
                    .ok_or(PresenterError::Unavailable("measured media services"))?;
                if !media.scale.is_finite() || media.scale <= 0.0 {
                    return Err(PresenterError::Unavailable("model device scale"));
                }
                let block_id = format!("{id}-rich-{}-model", entry.id);
                self.model_ids
                    .borrow_mut()
                    .entry(id.to_owned())
                    .or_default()
                    .insert(block_id.clone());
                let hovered = self
                    .model_hover
                    .borrow_mut()
                    .entry(block_id.clone())
                    .or_insert_with(|| Rc::new(Cell::new(false)))
                    .clone();
                let services = crate::ui::rich::media::model::services(
                    &block_id,
                    *theme,
                    media.model_assets.clone(),
                    media.model_capability.clone(),
                    glyph.clone(),
                    media.effect.clone(),
                    hovered,
                    invalidate.clone(),
                );
                let content = model.view(
                    &block_id,
                    media.width,
                    media.scale,
                    Some(&media.fetcher),
                    self.models.clone(),
                    &services,
                );
                let current_theme = *theme;
                let text = Rc::new(move |value: &str, role, color| {
                    crate::ui::rich::narrative::callout::text(&current_theme, value, role, color)
                });
                charts::heatmap::envelope(
                    theme,
                    envelope.title.as_deref(),
                    envelope.subtitle.as_deref(),
                    envelope.caption.as_deref(),
                    content,
                    text,
                )
            }
            Prepared::Image(model) => {
                let services = self
                    .media_services
                    .borrow()
                    .get(id)
                    .cloned()
                    .ok_or(PresenterError::Unavailable("measured media services"))?;
                let block_id = format!("{id}-rich-{}-image", entry.id);
                self.image_ids
                    .borrow_mut()
                    .entry(id.to_owned())
                    .or_default()
                    .insert(block_id.clone());
                let scope_key = format!(
                    "{}:{}",
                    services.fetcher.scope(),
                    services.fetcher.asset_owner("image", false)
                );
                let state = {
                    let mut states = self.images.borrow_mut();
                    let state = states.entry(block_id.clone()).or_insert_with(|| {
                        let state = crate::ui::rich::media::image::ImageState::new(
                            model,
                            services.fetcher.clone(),
                            services.images.clone(),
                        );
                        state.wake_mailbox().set_waker(services.wake.clone());
                        (
                            model.clone(),
                            scope_key.clone(),
                            Rc::new(RefCell::new(state)),
                        )
                    });
                    if !Arc::ptr_eq(&state.0, model) || state.1 != scope_key {
                        let replacement = crate::ui::rich::media::image::ImageState::new(
                            model,
                            services.fetcher.clone(),
                            services.images.clone(),
                        );
                        replacement.wake_mailbox().set_waker(services.wake.clone());
                        *state = (model.clone(), scope_key, Rc::new(RefCell::new(replacement)));
                    }
                    state
                        .2
                        .borrow()
                        .wake_mailbox()
                        .set_waker(services.wake.clone());
                    state.2.clone()
                };
                state.borrow_mut().refresh();
                let event_state = Rc::downgrade(&state);
                let event_id = block_id.clone();
                let effect = services.effect.clone();
                let refresh = invalidate.clone();
                let emit = Rc::new(move |action| {
                    let Some(state) = event_state.upgrade() else {
                        return;
                    };
                    match action {
                        crate::ui::rich::media::image::Action::Open { index, image } => {
                            let (fetcher, sources) = state.borrow().preview_sources();
                            effect(crate::ui::media::preview::Effect::Open(
                                crate::ui::media::preview::OpenPreview {
                                    fetcher,
                                    sources,
                                    index,
                                    image,
                                    opener_key: format!("{event_id}-image-{index}"),
                                },
                            ));
                        }
                        crate::ui::rich::media::image::Action::Retry(index) => {
                            if state.borrow_mut().retry(index) {
                                refresh();
                            }
                        }
                        crate::ui::rich::media::image::Action::CopyPath { index, position } => {
                            if let Some(source) = state.borrow().preview_sources().1.get(index) {
                                effect(crate::ui::media::preview::Effect::ShowCopyMenu(
                                    crate::ui::rich::actions::ShowCopyMenu {
                                        position,
                                        entries: vec![
                                            crate::ui::rich::actions::CopyEntry::primary(
                                                "Copy Image Path",
                                                source.raw.clone(),
                                            ),
                                        ],
                                    },
                                ));
                            }
                        }
                    }
                });
                let content = state
                    .borrow()
                    .view(model, *theme, &block_id, services.width, emit)
                    .map_err(PresenterError::Unavailable)?;
                let current_theme = *theme;
                let text = Rc::new(move |value: &str, role, color| {
                    crate::ui::rich::narrative::callout::text(&current_theme, value, role, color)
                });
                charts::heatmap::envelope(
                    theme,
                    envelope.title.as_deref(),
                    envelope.subtitle.as_deref(),
                    envelope.caption.as_deref(),
                    content,
                    text,
                )
            }
            Prepared::Audio(block) => {
                let services = self
                    .media_services
                    .borrow()
                    .get(id)
                    .cloned()
                    .ok_or(PresenterError::Unavailable("media services"))?;
                let block_id = format!("{id}-rich-{}-audio", entry.id);
                self.audio_ids
                    .borrow_mut()
                    .entry(id.to_owned())
                    .or_default()
                    .insert(block_id.clone());
                let click = Rc::new(move |element: Element, activate: Rc<dyn Fn()>| {
                    ui::Button::new()
                        .child(element)
                        .on_activate_event(move |event| {
                            if matches!(
                                event.source,
                                Some(aqua_ui::tree::ActivationSource::Pointer {
                                    button: aqua_core::PointerButton::Primary,
                                    ..
                                })
                            ) {
                                activate();
                            }
                        })
                        .build()
                        .focusable(false)
                });
                let content = block.view(
                    &block_id,
                    theme,
                    Some(&services.fetcher),
                    &services.audio_assets,
                    &services.player,
                    self.audio.clone(),
                    glyph.clone(),
                    click,
                    invalidate.clone(),
                );
                let current_theme = *theme;
                let text = Rc::new(move |value: &str, role, color| {
                    crate::ui::rich::narrative::callout::text(&current_theme, value, role, color)
                });
                charts::heatmap::envelope(
                    theme,
                    envelope.title.as_deref(),
                    envelope.subtitle.as_deref(),
                    envelope.caption.as_deref(),
                    content,
                    text,
                )
            }
            Prepared::ImageCompare(model) => {
                let services = self
                    .media_services
                    .borrow()
                    .get(id)
                    .cloned()
                    .ok_or(PresenterError::Unavailable("measured media services"))?;
                let block_id = format!("{id}-rich-{}-image-compare", entry.id);
                self.image_comparison_ids
                    .borrow_mut()
                    .entry(id.to_owned())
                    .or_default()
                    .insert(block_id.clone());
                let scope_key = format!(
                    "{}:{}",
                    services.fetcher.scope(),
                    services.fetcher.asset_owner("before", false)
                );
                let state = {
                    let mut states = self.image_comparisons.borrow_mut();
                    let entry = states.entry(block_id.clone()).or_insert_with(|| {
                        let state = crate::ui::rich::media::image_compare::view::CompareState::new(
                            model,
                            services.fetcher.clone(),
                            services.images.clone(),
                        );
                        state.wake_mailbox().set_waker(services.wake.clone());
                        (scope_key.clone(), Rc::new(RefCell::new(state)))
                    });
                    let changed = entry.0 != scope_key;
                    entry
                        .1
                        .borrow_mut()
                        .reconcile(model, services.fetcher.clone(), changed);
                    entry.0 = scope_key;
                    entry
                        .1
                        .borrow()
                        .wake_mailbox()
                        .set_waker(services.wake.clone());
                    entry.1.clone()
                };
                state.borrow_mut().refresh();
                let state_for_events = Rc::downgrade(&state);
                let refresh = invalidate.clone();
                let emit = Rc::new(move |action| {
                    if let Some(state) = state_for_events.upgrade()
                        && state.borrow_mut().dispatch(action)
                    {
                        refresh();
                    }
                });
                let content = state
                    .borrow()
                    .view(model, *theme, &block_id, services.width, emit)
                    .map_err(PresenterError::Unavailable)?;
                let current_theme = *theme;
                let text = Rc::new(move |value: &str, role, color| {
                    crate::ui::rich::narrative::callout::text(&current_theme, value, role, color)
                });
                charts::heatmap::envelope(
                    theme,
                    envelope.title.as_deref(),
                    envelope.subtitle.as_deref(),
                    envelope.caption.as_deref(),
                    content,
                    text,
                )
            }
            Prepared::AnnotatedCode(block) => {
                let services = code.ok_or(PresenterError::Unavailable("code services"))?;
                let reference = services
                    .reference
                    .as_ref()
                    .ok_or(PresenterError::Unavailable("reference services"))?;
                if reference_scope.is_some() && reference.instance.is_none() {
                    return Err(PresenterError::Unavailable("reference host instance"));
                }
                let block_id = format!("{id}-rich-{}-annotated", entry.id);
                self.annotated_ids
                    .borrow_mut()
                    .entry(id.to_owned())
                    .or_default()
                    .insert(block_id.clone());
                let state = self
                    .annotated
                    .borrow_mut()
                    .entry(block_id.clone())
                    .or_insert_with(|| {
                        Rc::new(RefCell::new(
                            crate::ui::rich::code::annotated::view::AnnotatedState::new(block),
                        ))
                    })
                    .clone();
                let content = block.render(
                    theme,
                    &block_id,
                    reference_scope,
                    reference.instance,
                    reference.open.clone(),
                    state,
                    invalidate.clone(),
                );
                let content = crate::ui::rich::code::card::envelope(
                    theme,
                    envelope.title.as_deref(),
                    envelope.subtitle.as_deref(),
                    envelope.caption.as_deref(),
                    content,
                );
                let copy = services.copy_enabled.then(|| {
                    (
                        (services.copy)(&format!("{block_id}-copy"), &block.copy()),
                        crate::ui::rich::code::card::CopyPlacement::TopRight,
                    )
                });
                crate::ui::rich::code::card::render(theme, content, copy, false)
            }
            Prepared::FileTree(tree) => {
                let services = code.ok_or(PresenterError::Unavailable("code services"))?;
                let reference = services
                    .reference
                    .as_ref()
                    .ok_or(PresenterError::Unavailable("reference services"))?;
                if reference_scope.is_some() && reference.instance.is_none() {
                    return Err(PresenterError::Unavailable("reference host instance"));
                }
                let block_id = format!("{id}-rich-{}-file-tree", entry.id);
                self.file_tree_ids
                    .borrow_mut()
                    .entry(id.to_owned())
                    .or_default()
                    .insert(block_id.clone());
                let state = self
                    .file_trees
                    .borrow_mut()
                    .entry(block_id.clone())
                    .or_default()
                    .clone();
                if state.borrow_mut().reconcile(tree) {
                    self.file_tree_focus.borrow_mut().remove(&block_id);
                }
                let tree_for_events = tree.clone();
                let state_for_events = state.clone();
                let event_id = block_id.clone();
                let pending = self.file_tree_focus.clone();
                let open = reference.open.clone();
                let instance = reference.instance;
                let refresh = invalidate.clone();
                let emit = Rc::new(move |action| {
                    let effect = state_for_events.borrow_mut().dispatch(
                        &tree_for_events,
                        &event_id,
                        reference_scope.clone(),
                        action,
                    );
                    if let Some(focus) = effect.focus {
                        pending.borrow_mut().insert(event_id.clone(), focus);
                    }
                    if let Some(request) = effect.open {
                        open(request, instance);
                    }
                    if effect.changed || pending.borrow().contains_key(&event_id) {
                        refresh();
                    }
                });
                let content = tree.render(theme, &block_id, state, emit);
                let current_theme = *theme;
                let text = Rc::new(move |value: &str, role, color| {
                    crate::ui::rich::narrative::callout::text(&current_theme, value, role, color)
                });
                charts::heatmap::envelope(
                    theme,
                    envelope.title.as_deref(),
                    envelope.subtitle.as_deref(),
                    envelope.caption.as_deref(),
                    content,
                    text,
                )
            }
            Prepared::CodeRef(reference) => {
                let services = code.ok_or(PresenterError::Unavailable("code services"))?;
                let open = services
                    .reference
                    .as_ref()
                    .ok_or(PresenterError::Unavailable("reference services"))?;
                if reference_scope.is_some() && open.instance.is_none() {
                    return Err(PresenterError::Unavailable("reference host instance"));
                }
                let block_id = format!("{id}-rich-{}-code-ref", entry.id);
                let content = reference.render(
                    theme,
                    &block_id,
                    reference_scope,
                    open.instance,
                    open.open.clone(),
                );
                let content = crate::ui::rich::code::card::envelope(
                    theme,
                    envelope.title.as_deref(),
                    envelope.subtitle.as_deref(),
                    envelope.caption.as_deref(),
                    content,
                );
                let copy = services.copy_enabled.then(|| {
                    (
                        (services.copy)(&format!("{block_id}-copy"), &reference.copy()),
                        crate::ui::rich::code::card::CopyPlacement::TopRight,
                    )
                });
                crate::ui::rich::code::card::render(theme, content, copy, false)
            }
            Prepared::Compare(compare) => {
                let services = heatmap.ok_or(PresenterError::Unavailable("chart services"))?;
                let block_id = format!("{id}-rich-{}-compare", entry.id);
                self.comparison_ids
                    .borrow_mut()
                    .entry(id.to_owned())
                    .or_default()
                    .insert(block_id.clone());
                let content = compare.view(
                    block_id,
                    theme,
                    self.comparisons.clone(),
                    services.navigation_frame.clone(),
                    invalidate.clone(),
                );
                let current_theme = *theme;
                let text = Rc::new(move |value: &str, role, color| {
                    crate::ui::rich::narrative::callout::text(&current_theme, value, role, color)
                });
                charts::heatmap::envelope(
                    theme,
                    envelope.title.as_deref(),
                    envelope.subtitle.as_deref(),
                    envelope.caption.as_deref(),
                    content,
                    text,
                )
            }
            Prepared::Stats(stats) => {
                let block_id = format!("{id}-rich-{}-stats", entry.id);
                self.stats_ids
                    .borrow_mut()
                    .entry(id.to_owned())
                    .or_default()
                    .insert(block_id.clone());
                let content = stats.view(
                    block_id,
                    theme,
                    self.stats.clone(),
                    glyph.clone(),
                    Rc::new(charts::frame::stats_hint),
                    invalidate.clone(),
                );
                let current_theme = *theme;
                let text = Rc::new(move |value: &str, role, color| {
                    crate::ui::rich::narrative::callout::text(&current_theme, value, role, color)
                });
                charts::heatmap::envelope(
                    theme,
                    envelope.title.as_deref(),
                    envelope.subtitle.as_deref(),
                    envelope.caption.as_deref(),
                    content,
                    text,
                )
            }
            Prepared::Callout(callout) => {
                if envelope.subtitle.is_some() || envelope.caption.is_some() {
                    return Err(PresenterError::Unavailable("rich envelope"));
                }
                let glyph: GlyphFactory = glyph.clone();
                let block_id = format!("{id}-rich-{}-callout", entry.id);
                self.callout_ids
                    .borrow_mut()
                    .entry(id.to_owned())
                    .or_default()
                    .insert(block_id.clone());
                callout.view(
                    block_id,
                    theme,
                    self.callouts.clone(),
                    glyph,
                    invalidate.clone(),
                )
            }
            Prepared::Chart(charts::Prepared::Heatmap(spec)) => {
                let services = heatmap.ok_or(PresenterError::Unavailable("heatmap services"))?;
                let current_theme = *theme;
                let text = Rc::new(
                    move |role: HeatmapTextRole,
                          value: &str,
                          size,
                          line,
                          weight,
                          color: crate::ui::theme::Oklch| {
                        if role == HeatmapTextRole::Ramp {
                            return crate::ui::rich::charts::frame::mark_text(
                                &current_theme,
                                value,
                                size,
                                line,
                                weight,
                                color,
                                true,
                            );
                        }
                        let style = aqua_text::TextStyle {
                            families: vec![aqua_text::FontFamily::Named(
                                current_theme.typography.sans.family.to_owned(),
                            )],
                            size_px: size,
                            line_height_px: line,
                            weight,
                            ..aqua_text::TextStyle::default()
                        };
                        let mut text = ui::Text::new(value.to_owned())
                            .text_style(style)
                            .gpui_source_color(color.to_reference_text_color())
                            .wrap(false);
                        if role == HeatmapTextRole::RowLabel {
                            text = text.overflow(aqua_text::overflow::OverflowMode::EndEllipsis);
                        } else {
                            text = text.clip(true);
                        }
                        text.build()
                    },
                );
                let block_id = format!("{id}-rich-{}-heatmap", entry.id);
                self.heatmap_ids
                    .borrow_mut()
                    .entry(id.to_owned())
                    .or_default()
                    .insert(block_id.clone());
                let content = spec.view(
                    block_id,
                    theme,
                    self.heatmaps.clone(),
                    text,
                    services.empty.clone(),
                    services.readout.clone(),
                    services.navigation_frame.clone(),
                    invalidate.clone(),
                );
                let text = Rc::new(move |value: &str, role, color| {
                    crate::ui::rich::narrative::callout::text(&current_theme, value, role, color)
                });
                charts::heatmap::envelope(
                    theme,
                    envelope.title.as_deref(),
                    envelope.subtitle.as_deref(),
                    envelope.caption.as_deref(),
                    content,
                    text,
                )
            }
            Prepared::Chart(charts::Prepared::Funnel(spec)) => {
                let services = heatmap.ok_or(PresenterError::Unavailable("chart services"))?;
                let current_theme = *theme;
                let text = Rc::new(
                    move |role: FunnelTextRole, value: &str, size, line, weight, color| {
                        let text = crate::ui::rich::charts::frame::mark_text(
                            &current_theme,
                            value,
                            size,
                            line,
                            weight,
                            color,
                            role != FunnelTextRole::RowLabel,
                        );
                        if role == FunnelTextRole::RowLabel {
                            text.style(Style::new().clip(true))
                        } else {
                            text
                        }
                    },
                );
                let block_id = format!("{id}-rich-{}-funnel", entry.id);
                self.funnel_ids
                    .borrow_mut()
                    .entry(id.to_owned())
                    .or_default()
                    .insert(block_id.clone());
                let content = spec.view(
                    block_id,
                    theme,
                    self.funnels.clone(),
                    text,
                    services.empty.clone(),
                    services.navigation_frame.clone(),
                    invalidate.clone(),
                );
                let text = Rc::new(move |value: &str, role, color| {
                    crate::ui::rich::narrative::callout::text(&current_theme, value, role, color)
                });
                charts::heatmap::envelope(
                    theme,
                    envelope.title.as_deref(),
                    envelope.subtitle.as_deref(),
                    envelope.caption.as_deref(),
                    content,
                    text,
                )
            }
            Prepared::Chart(charts::Prepared::Spans(layout)) => {
                let shared = heatmap.ok_or(PresenterError::Unavailable("chart services"))?;
                let block_id = format!("{id}-rich-{}-spans", entry.id);
                self.spans_ids
                    .borrow_mut()
                    .entry(id.to_owned())
                    .or_default()
                    .insert(block_id.clone());
                let current_theme = *theme;
                let services = charts::spans::Services {
                    text: Rc::new(move |value, size, color| {
                        let style = aqua_text::TextStyle {
                            families: vec![aqua_text::FontFamily::Named(
                                current_theme.typography.sans.family.to_owned(),
                            )],
                            size_px: size,
                            line_height_px: current_theme.density.control_sm,
                            ..aqua_text::TextStyle::default()
                        };
                        ui::Text::new(value.to_owned())
                            .text_style(style)
                            .gpui_source_color(color.to_reference_text_color())
                            .wrap(false)
                            .overflow(aqua_text::overflow::OverflowMode::EndEllipsis)
                            .build()
                    }),
                    bar_label: Rc::new(move |label: charts::spans::BarLabel| {
                        let style = aqua_text::TextStyle {
                            families: vec![aqua_text::FontFamily::Named(
                                current_theme.typography.sans.family.to_owned(),
                            )],
                            size_px: current_theme.typography.control_text,
                            line_height_px: label.row_pitch - 4.0,
                            weight: aqua_text::FontWeight::MEDIUM,
                            ..aqua_text::TextStyle::default()
                        };
                        let text = ui::Text::new(label.text)
                            .text_style(style)
                            .gpui_source_color(current_theme.colors.text.to_reference_text_color())
                            .wrap(false)
                            .clip(true)
                            .build();
                        let bar = ui::Box::new()
                            .width(Length::Pct((label.x1 - label.x0) * 100.0))
                            .height(label.row_pitch - 4.0)
                            .style(
                                Style::new()
                                    .clip(true)
                                    .padding_insets(Insets {
                                        left: 6.0,
                                        right: 6.0,
                                        ..Insets::default()
                                    })
                                    .align(aqua_ui::AlignItems::CENTER),
                            )
                            .child(text)
                            .build();
                        ui::Box::new()
                            .layout("row")
                            .width(Length::Pct(100.0))
                            .height(label.row_pitch - 4.0)
                            .style(Style::new().absolute_at(Some(0.0), Some(2.0), None, None))
                            .child(
                                ui::Box::new()
                                    .width(Length::Pct(label.x0 * 100.0))
                                    .style(Style::new().flex_shrink(0.0))
                                    .build(),
                            )
                            .child(bar)
                            .build()
                    }),
                    marker: Rc::new(move |x, label, color| {
                        charts::frame::plot_label_positioned(
                            &current_theme,
                            x,
                            0.0,
                            label,
                            color,
                            true,
                        )
                    }),
                    ticks: Rc::new(move |ticks: Vec<charts::spans::Tick>| {
                        charts::frame::tick_strip(
                            &current_theme,
                            ticks
                                .into_iter()
                                .map(|tick| charts::distribution::Tick {
                                    at: tick.at,
                                    label: tick.label,
                                })
                                .collect(),
                        )
                    }),
                    readout: Rc::new(move |x, heading, rows| {
                        charts::frame::readout_rows(
                            &current_theme,
                            x,
                            heading.unwrap_or(""),
                            rows.into_iter()
                                .map(|(color, label, value)| charts::distribution::ReadoutRow {
                                    color,
                                    label,
                                    value,
                                })
                                .collect(),
                        )
                    }),
                    empty: shared.empty.clone(),
                    navigation: shared.navigation_frame.clone(),
                };
                let content = charts::spans::view(
                    block_id,
                    theme,
                    layout.clone(),
                    self.spans.clone(),
                    &services,
                    invalidate.clone(),
                );
                let text = Rc::new(move |value: &str, role, color| {
                    crate::ui::rich::narrative::callout::text(&current_theme, value, role, color)
                });
                charts::heatmap::envelope(
                    theme,
                    envelope.title.as_deref(),
                    envelope.subtitle.as_deref(),
                    envelope.caption.as_deref(),
                    content,
                    text,
                )
            }
            Prepared::Chart(charts::Prepared::Hierarchy(spec)) => {
                let shared = heatmap.ok_or(PresenterError::Unavailable("chart services"))?;
                let block_id = format!("{id}-rich-{}-hierarchy", entry.id);
                self.hierarchy_ids
                    .borrow_mut()
                    .entry(id.to_owned())
                    .or_default()
                    .insert(block_id.clone());
                let current_theme = *theme;
                let trail_id = block_id.clone();
                let services = charts::hierarchy::Services {
                    label: Rc::new(move |part: charts::hierarchy::PartLabel| {
                        let text_style = |weight| aqua_text::TextStyle {
                            families: vec![aqua_text::FontFamily::Named(
                                current_theme.typography.sans.family.to_owned(),
                            )],
                            size_px: 11.0,
                            line_height_px: 14.0,
                            weight,
                            ..aqua_text::TextStyle::default()
                        };
                        let name = ui::Text::new(part.name)
                            .text_style(text_style(aqua_text::FontWeight::MEDIUM))
                            .gpui_source_color(current_theme.colors.text.to_reference_text_color())
                            .wrap(false)
                            .clip(true)
                            .build();
                        let mut label = ui::Box::new()
                            .layout(if part.stacked { "column" } else { "row" })
                            .width(part.width * part.plot_width)
                            .height(part.height)
                            .gap(if part.stacked { 0.0 } else { 6.0 })
                            .style(
                                Style::new()
                                    .absolute_at(
                                        Some(part.x * part.plot_width),
                                        Some(part.y * part.plot_height),
                                        None,
                                        None,
                                    )
                                    .clip(true)
                                    .padding_insets(Insets {
                                        left: 6.0,
                                        right: 6.0,
                                        top: if part.stacked { 3.0 } else { 0.0 },
                                        bottom: if part.stacked { 3.0 } else { 0.0 },
                                    })
                                    .align(if part.stacked {
                                        aqua_ui::AlignItems::START
                                    } else {
                                        aqua_ui::AlignItems::CENTER
                                    }),
                            );
                        label = label.child(name);
                        if let Some(value) = part.value {
                            label = label.child(
                                ui::Text::new(value)
                                    .text_style(text_style(aqua_text::FontWeight::NORMAL))
                                    .gpui_source_color(
                                        current_theme.colors.text_muted.to_reference_text_color(),
                                    )
                                    .wrap(false)
                                    .clip(true)
                                    .build(),
                            );
                        }
                        label.build()
                    }),
                    readout: Rc::new(move |high, x, heading, rows| {
                        let card = charts::frame::readout_rows_at(
                            &current_theme,
                            x,
                            heading,
                            rows.into_iter()
                                .map(|(color, label, value)| charts::distribution::ReadoutRow {
                                    color,
                                    label,
                                    value,
                                })
                                .collect(),
                            high,
                        );
                        card.on_input(
                            aqua_ui::input::InputKind::PointerMove,
                            aqua_ui::input::EventPhase::Bubble,
                            |_| aqua_ui::input::InputResponse {
                                stop_propagation: true,
                                ..Default::default()
                            },
                        )
                        .on_input(
                            aqua_ui::input::InputKind::PointerDown,
                            aqua_ui::input::EventPhase::Bubble,
                            |_| aqua_ui::input::InputResponse {
                                stop_propagation: true,
                                ..Default::default()
                            },
                        )
                        .on_input(
                            aqua_ui::input::InputKind::PointerUp,
                            aqua_ui::input::EventPhase::Bubble,
                            |_| aqua_ui::input::InputResponse {
                                stop_propagation: true,
                                ..Default::default()
                            },
                        )
                    }),
                    trail: Rc::new(move |entries, select| {
                        let mut trail = ui::Box::new()
                            .layout("row")
                            .gap(2.0)
                            .style(Style::new().wrap(true).align(aqua_ui::AlignItems::CENTER));
                        for (step, (node, name, current)) in entries.into_iter().enumerate() {
                            if step > 0 {
                                trail = trail.child(crate::ui::rich::narrative::callout::text(
                                    &current_theme,
                                    "/",
                                    TextRole::Caption,
                                    current_theme.colors.text_muted,
                                ));
                            }
                            let choose = select.clone();
                            trail = trail.child(
                                crate::ui::button::Button::text(
                                    format!("{trail_id}-trail-{node}"),
                                    current_theme,
                                    name,
                                )
                                .variant(crate::ui::button::ButtonVariant::Ghost)
                                .disabled(current)
                                .on_activate(move |_| choose(node))
                                .build(),
                            );
                        }
                        trail.build()
                    }),
                    click: Rc::new(move |element, handler| {
                        ui::Button::new()
                            .width(Length::Pct(100.0))
                            .height(Length::Pct(100.0))
                            .child(element)
                            .on_activate_event(move |event| {
                                if let Some(aqua_ui::tree::ActivationSource::Pointer {
                                    up,
                                    button: aqua_core::PointerButton::Primary,
                                    ..
                                }) = event.source
                                {
                                    handler(up.position);
                                }
                            })
                            .build()
                            .focusable(false)
                    }),
                    navigation: shared.navigation_frame.clone(),
                };
                let content = charts::hierarchy::view(
                    block_id,
                    theme,
                    spec.tree.clone(),
                    spec.layout,
                    spec.unit.clone(),
                    charts::hierarchy::levels(spec.layout, spec.depth),
                    self.hierarchies.clone(),
                    &services,
                    invalidate.clone(),
                );
                let text = Rc::new(move |value: &str, role, color| {
                    crate::ui::rich::narrative::callout::text(&current_theme, value, role, color)
                });
                charts::heatmap::envelope(
                    theme,
                    envelope.title.as_deref(),
                    envelope.subtitle.as_deref(),
                    envelope.caption.as_deref(),
                    content,
                    text,
                )
            }
            Prepared::Chart(charts::Prepared::Sankey(graph)) => {
                let shared = heatmap.ok_or(PresenterError::Unavailable("chart services"))?;
                let block_id = format!("{id}-rich-{}-sankey", entry.id);
                self.sankey_ids
                    .borrow_mut()
                    .entry(id.to_owned())
                    .or_default()
                    .insert(block_id.clone());
                let current_theme = *theme;
                let label = Rc::new(move |label: charts::sankey::NodeLabel| {
                    let style = aqua_text::TextStyle {
                        families: vec![aqua_text::FontFamily::Named(
                            current_theme.typography.sans.family.to_owned(),
                        )],
                        size_px: current_theme.typography.control_text,
                        line_height_px: label.line_height,
                        ..aqua_text::TextStyle::default()
                    };
                    let name = ui::Text::new(label.name)
                        .text_style(style.clone())
                        .gpui_source_color(current_theme.colors.text.to_reference_text_color())
                        .wrap(false)
                        .overflow(aqua_text::overflow::OverflowMode::EndEllipsis)
                        .build();
                    let value = ui::Text::new(label.value)
                        .text_style(style)
                        .gpui_source_color(
                            current_theme.colors.text_muted.to_reference_text_color(),
                        )
                        .wrap(false)
                        .build();
                    let (left, right) = if label.last {
                        (None, Some(label.x))
                    } else {
                        (Some(label.x), None)
                    };
                    ui::Box::new()
                        .layout("row")
                        .gap(4.0)
                        .height(label.line_height)
                        .style(
                            Style::new()
                                .absolute_at(left, Some(label.y), right, None)
                                .max_width(Length::Px(label.max_width))
                                .min_width(Length::Px(0.0))
                                .clip(true)
                                .opacity(label.alpha),
                        )
                        .child(
                            ui::Box::new()
                                .style(Style::new().flex_grow(1.0).min_width(Length::Px(0.0)))
                                .child(name)
                                .build(),
                        )
                        .child(
                            ui::Box::new()
                                .style(Style::new().flex_shrink(0.0))
                                .child(value)
                                .build(),
                        )
                        .build()
                });
                let readout = Rc::new(
                    move |x, heading: &str, rows: Vec<(Option<GpuiHsla>, String, String)>| {
                        charts::frame::readout_rows(
                            &current_theme,
                            x,
                            heading,
                            rows.into_iter()
                                .map(|(color, label, value)| charts::distribution::ReadoutRow {
                                    color,
                                    label,
                                    value,
                                })
                                .collect(),
                        )
                    },
                );
                let content = charts::sankey::view(
                    block_id,
                    theme,
                    graph.clone(),
                    self.sankeys.clone(),
                    label,
                    readout,
                    shared.empty.clone(),
                    shared.navigation_frame.clone(),
                    invalidate.clone(),
                );
                let text = Rc::new(move |value: &str, role, color| {
                    crate::ui::rich::narrative::callout::text(&current_theme, value, role, color)
                });
                charts::heatmap::envelope(
                    theme,
                    envelope.title.as_deref(),
                    envelope.subtitle.as_deref(),
                    envelope.caption.as_deref(),
                    content,
                    text,
                )
            }
            Prepared::Chart(charts::Prepared::Chart(style, model)) => {
                let shared = heatmap.ok_or(PresenterError::Unavailable("chart services"))?;
                let block_id = format!("{id}-rich-{}-chart", entry.id);
                self.chart_ids
                    .borrow_mut()
                    .entry(id.to_owned())
                    .or_default()
                    .insert(block_id.clone());
                let current_theme = *theme;
                let legend_id = block_id.clone();
                let reset_id = format!("{block_id}-reset");
                let services = charts::chart::Services {
                    press: Rc::new(charts::chart::press),
                    frame: Rc::new(
                        move |plot, y_ticks, x_ticks, end_labels, y_title, x_title| {
                            let ticks = |values: Vec<charts::chart::plot::Tick>| {
                                values
                                    .into_iter()
                                    .map(|tick| charts::distribution::Tick {
                                        at: tick.at,
                                        label: tick.label,
                                    })
                                    .collect()
                            };
                            let frame = charts::distribution::PlotFrame {
                                x_ticks: ticks(x_ticks),
                                y_ticks: ticks(y_ticks),
                                y_title,
                                x_title,
                                y_axis_width: 48.0,
                                height: 240.0,
                            };
                            charts::frame::numeric_frame_with_labels(
                                &current_theme,
                                plot,
                                frame,
                                end_labels
                                    .into_iter()
                                    .map(|label| (label.y, label.text, label.color))
                                    .collect(),
                            )
                        },
                    ),
                    label: Rc::new(move |label| {
                        charts::frame::plot_label_positioned(
                            &current_theme,
                            label.x,
                            label.y,
                            &label.text,
                            label.color,
                            label.below,
                        )
                    }),
                    readout: Rc::new(move |x, heading, rows| {
                        charts::frame::readout_rows(
                            &current_theme,
                            x,
                            heading,
                            rows.into_iter()
                                .map(|(color, label, value)| charts::distribution::ReadoutRow {
                                    color,
                                    label,
                                    value,
                                })
                                .collect(),
                        )
                    }),
                    legend: Rc::new(move |entries, toggle| {
                        let mut legend = ui::Box::new()
                            .layout("row")
                            .gap(12.0)
                            .style(Style::new().wrap(true).align(aqua_ui::AlignItems::CENTER));
                        for (index, (name, color, hidden)) in entries.into_iter().enumerate() {
                            let color = if hidden {
                                current_theme
                                    .colors
                                    .text
                                    .at(crate::ui::theme::Opacity::Rule)
                                    .to_reference_paint()
                            } else {
                                color
                            };
                            let swatch = ui::Box::new()
                                .width(12.0)
                                .height(4.0)
                                .radius(crate::ui::theme::RadiusTokens::value(
                                    crate::ui::theme::RadiusStep::X2s,
                                ))
                                .style(
                                    Style::new()
                                        .flex_shrink(0.0)
                                        .gpui_background(aqua_core::GpuiBackground::Solid(color)),
                                )
                                .build();
                            let activate = toggle.clone();
                            let button = crate::ui::button::Button::text(
                                format!("{legend_id}-legend-{index}"),
                                current_theme,
                                name,
                            )
                            .variant(crate::ui::button::ButtonVariant::Ghost)
                            .icon_slot(swatch)
                            .on_activate(move |_| activate(index))
                            .build();
                            legend = legend.child(button);
                        }
                        legend.build()
                    }),
                    reset: Rc::new(move |reset| {
                        crate::ui::button::Button::icon_text(
                            reset_id.clone(),
                            current_theme,
                            AssetId::TABLER_ZOOM_OUT,
                            "Reset zoom",
                        )
                        .variant(crate::ui::button::ButtonVariant::Ghost)
                        .on_activate(move |_| reset())
                        .build()
                    }),
                    empty: shared.empty.clone(),
                    navigation: shared.navigation_frame.clone(),
                };
                let content = charts::chart::view(
                    block_id,
                    theme,
                    style.clone(),
                    model.clone(),
                    self.charts.clone(),
                    &services,
                    invalidate.clone(),
                );
                let text = Rc::new(move |value: &str, role, color| {
                    crate::ui::rich::narrative::callout::text(&current_theme, value, role, color)
                });
                charts::heatmap::envelope(
                    theme,
                    envelope.title.as_deref(),
                    envelope.subtitle.as_deref(),
                    envelope.caption.as_deref(),
                    content,
                    text,
                )
            }
            Prepared::Chart(charts::Prepared::Distribution(spec)) => {
                let shared = heatmap.ok_or(PresenterError::Unavailable("chart services"))?;
                let block_id = format!("{id}-rich-{}-distribution", entry.id);
                self.distribution_ids
                    .borrow_mut()
                    .entry(id.to_owned())
                    .or_default()
                    .insert(block_id.clone());
                let current_theme = *theme;
                let services = charts::distribution::Services {
                    frame: Rc::new(move |plot, frame| {
                        charts::frame::numeric_frame(&current_theme, plot, frame)
                    }),
                    readout: Rc::new(move |x, heading, rows| {
                        charts::frame::readout_rows(&current_theme, x, heading, rows)
                    }),
                    plot_label: Rc::new(move |x, y, label, color| {
                        charts::frame::plot_label(&current_theme, x, y, label, color)
                    }),
                    legend: Rc::new(move |entries| charts::frame::legend(&current_theme, entries)),
                    text: Rc::new(move |value, size, line, weight, color, ellipsis| {
                        let style = aqua_text::TextStyle {
                            families: vec![aqua_text::FontFamily::Named(
                                current_theme.typography.sans.family.to_owned(),
                            )],
                            size_px: size,
                            line_height_px: line,
                            weight,
                            ..aqua_text::TextStyle::default()
                        };
                        let text = ui::Text::new(value.to_owned())
                            .text_style(style)
                            .gpui_source_color(color.to_reference_text_color())
                            .wrap(false);
                        if ellipsis {
                            text.overflow(aqua_text::overflow::OverflowMode::EndEllipsis)
                                .build()
                        } else {
                            text.clip(true).build()
                        }
                    }),
                    empty: shared.empty.clone(),
                    navigation: shared.navigation_frame.clone(),
                    hover_fill: Rc::new(move || {
                        let (top, bottom) = crate::ui::hover::hover_fill_stops(
                            &current_theme,
                            current_theme.colors.text,
                        );
                        aqua_core::GpuiBackground::LinearGradient {
                            angle: 180.0,
                            color_space: aqua_core::GpuiGradientColorSpace::Oklab,
                            stops: [
                                aqua_core::GpuiGradientStop {
                                    percentage: 0.0,
                                    color: top.to_reference_paint(),
                                },
                                aqua_core::GpuiGradientStop {
                                    percentage: 1.0,
                                    color: bottom.to_reference_paint(),
                                },
                            ],
                        }
                    }),
                    tick_strip: Rc::new(move |ticks| {
                        charts::frame::tick_strip(&current_theme, ticks)
                    }),
                };
                let content = charts::distribution::view(
                    block_id,
                    theme,
                    spec.clone(),
                    self.distributions.clone(),
                    &services,
                    invalidate.clone(),
                );
                let text = Rc::new(move |value: &str, role, color| {
                    crate::ui::rich::narrative::callout::text(&current_theme, value, role, color)
                });
                charts::heatmap::envelope(
                    theme,
                    envelope.title.as_deref(),
                    envelope.subtitle.as_deref(),
                    envelope.caption.as_deref(),
                    content,
                    text,
                )
            }
            Prepared::Chart(charts::Prepared::Concentration(spec)) => {
                let services = heatmap.ok_or(PresenterError::Unavailable("chart services"))?;
                let block_id = format!("{id}-rich-{}-concentration", entry.id);
                self.concentration_ids
                    .borrow_mut()
                    .entry(id.to_owned())
                    .or_default()
                    .insert(block_id.clone());
                let content = spec.view(
                    block_id,
                    theme,
                    self.concentrations.clone(),
                    services.concentration.frame.clone(),
                    services.concentration.plot_label.clone(),
                    services.concentration.readout.clone(),
                    services.empty.clone(),
                    services.navigation_frame.clone(),
                    invalidate.clone(),
                );
                let current_theme = *theme;
                let text = Rc::new(move |value: &str, role, color| {
                    crate::ui::rich::narrative::callout::text(&current_theme, value, role, color)
                });
                charts::heatmap::envelope(
                    theme,
                    envelope.title.as_deref(),
                    envelope.subtitle.as_deref(),
                    envelope.caption.as_deref(),
                    content,
                    text,
                )
            }
        };
        Ok(element)
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
                Block::Code(_) => {
                    let Some(Some(entry)) = self.rich.get(index) else {
                        return Err(PresenterError::Unavailable("code fence or rich fallback"));
                    };
                    let element = self.render_rich(
                        entry,
                        id,
                        theme,
                        glyph.clone(),
                        invalidate.clone(),
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
                    let element = super::list::render(
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
                    let element = super::table_view::render(
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
            proof: PreparationProof(self.proof.clone()),
        })
    }
}
