use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::num::NonZeroUsize;
use std::rc::Rc;
use std::sync::Arc;

use aqua::mailbox::Mailbox;
use aqua_core::GpuiHsla;
use aqua_text::document::{SelectionDocumentId, SelectionSource};
use aqua_ui::collection_runtime::{SelectionBatchOwner, SelectionSourceBatch};
use aqua_ui::element::Element;

use super::visual::LeafError;
use super::{CodeFence, MarkdownDocument, MarkdownError};
use crate::ui::rich::charts::{
    self, chart::ChartState, concentration::ConcentrationState, distribution::DistributionState,
    funnel::FunnelState, heatmap::HeatmapState, hierarchy::HierarchyState, sankey::SankeyState,
    spans::SpansState,
};
use crate::ui::rich::data::stats::StatsState;
use crate::ui::rich::narrative::callout::CalloutState;
use crate::ui::rich::narrative::compare::CompareState;
use crate::ui::rich::{self, RichFence};
use crate::ui::theme::ArtisanTheme;
mod chart_views;
mod code;
mod media;
mod narrative;
mod preparation;
pub use self::preparation::MentionRevision;
mod projection;
mod rendering;
mod stored_leaves;
use self::code::code_card;

#[derive(Debug, Clone)]
pub struct PreparationProof(Arc<()>);

#[derive(Debug)]
pub struct RenderedMarkdown {
    pub element: Element,
    pub selection_sources: Vec<SelectionSource>,
    pub proof: PreparationProof,
}

#[derive(Debug)]
pub struct ProjectedTextLeaf {
    pub key: String,
    pub slot: usize,
    pub text: Arc<str>,
}

#[derive(Debug)]
pub struct ProjectedRevealText {
    pub key: String,
    pub text: Arc<str>,
    pub end: usize,
    pub fades: Vec<super::reveal::Fade>,
}

#[derive(Debug)]
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

#[derive(Debug)]
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

impl std::fmt::Debug for ProjectedMarkdown {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProjectedMarkdown").finish_non_exhaustive()
    }
}

impl ProjectedMarkdown {
    pub fn reveal_text(&self, reveal: &super::reveal::PreparedReveal) -> Vec<ProjectedRevealText> {
        reveal.set_tail(self.geometry.tail);
        self.geometry
            .text
            .iter()
            .map(|leaf| {
                let fades = reveal.fades(leaf.slot, &leaf.text);
                let end = reveal.visible_end(leaf.slot, &leaf.text);
                ProjectedRevealText {
                    key: leaf.key.clone(),
                    text: Arc::clone(&leaf.text),
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
    pub proof: super::stored::LeafProof,
}

impl std::fmt::Debug for StoredProjectedMarkdown {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StoredProjectedMarkdown")
            .finish_non_exhaustive()
    }
}

impl StoredProjectedMarkdown {
    pub fn reveal_text(&self, reveal: &super::reveal::PreparedReveal) -> Vec<ProjectedRevealText> {
        reveal.set_tail(self.geometry.tail);
        self.geometry
            .text
            .iter()
            .map(|leaf| ProjectedRevealText {
                key: leaf.key.clone(),
                text: Arc::clone(&leaf.text),
                end: reveal.visible_end(leaf.slot, &leaf.text),
                fades: reveal.fades(leaf.slot, &leaf.text),
            })
            .collect()
    }
}

struct StoredProjectedLeaf {
    leaf: Rc<super::stored::StoredLeaf>,
    document: SelectionDocumentId,
    prefix: Vec<u64>,
    selection_owner: SelectionBatchOwner,
    selection_batch: Arc<SelectionSourceBatch>,
    sources: Arc<[SelectionSource]>,
    fence: Option<super::fence::Fence>,
    rich: Option<RetainedRich>,
    fallback_note: Option<editor_core::Problem>,
}

type CopyButton = dyn Fn(&str, &str) -> Element;
type HeatmapReadout = dyn Fn(f32, &str, &str) -> Element;
type ConcentrationFrame = dyn Fn(Element, &str, &str) -> Element;
type PlotLabel = dyn Fn(f32, f32, &str, GpuiHsla) -> Element;
type ConcentrationReadout = dyn Fn(f32, &str, &str, &str, GpuiHsla) -> Element;
type FileTreeFocus = (String, String, usize);
type ImageEntry = (
    Arc<rich::media::image::ImageBlock>,
    String,
    Rc<RefCell<rich::media::image::ImageState>>,
);
type ImageComparisonEntry = (
    String,
    Rc<RefCell<rich::media::image_compare::view::CompareState>>,
);

pub struct CodeServices {
    pub copy: Rc<CopyButton>,
    pub copy_enabled: bool,
    pub reference: Option<ReferenceServices>,
}

impl std::fmt::Debug for CodeServices {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CodeServices").finish_non_exhaustive()
    }
}

pub struct ReferenceServices {
    pub instance: Option<editor_core::HostInstance>,
    pub open: Rc<dyn Fn(rich::actions::OpenReference, Option<editor_core::HostInstance>)>,
}

impl std::fmt::Debug for ReferenceServices {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReferenceServices").finish_non_exhaustive()
    }
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

impl std::fmt::Debug for MediaServices {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MediaServices").finish_non_exhaustive()
    }
}

pub struct HeatmapServices {
    empty: Rc<dyn Fn(&str) -> Element>,
    readout: Rc<HeatmapReadout>,
    navigation_frame: Rc<dyn Fn(Element, GpuiHsla) -> Element>,
    concentration: ConcentrationServices,
}

impl std::fmt::Debug for HeatmapServices {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HeatmapServices").finish_non_exhaustive()
    }
}

pub struct ConcentrationServices {
    frame: Rc<ConcentrationFrame>,
    plot_label: Rc<PlotLabel>,
    readout: Rc<ConcentrationReadout>,
}

impl std::fmt::Debug for ConcentrationServices {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConcentrationServices")
            .finish_non_exhaustive()
    }
}

impl ConcentrationServices {
    #[expect(
        clippy::large_types_passed_by_value,
        reason = "each concentration chart factory closure captures its own theme copy"
    )]
    fn new(theme: ArtisanTheme) -> Self {
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
    #[expect(
        clippy::large_types_passed_by_value,
        reason = "each chart factory closure and the nested concentration services capture a theme copy"
    )]
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
    marks: Option<super::MessageMarks>,
    document: Option<MarkdownDocument>,
    projected: Option<super::projected::ProjectedDocument>,
    stored_leaves: RefCell<HashMap<(String, usize), StoredProjectedLeaf>>,
    selection_owner: SelectionBatchOwner,
    selection_revision: Cell<u64>,
    rich: Vec<Option<RetainedRich>>,
    fence_tokens: super::FenceTokenCache,
    next_rich_id: u64,
    proof: Arc<()>,
    leaf_proofs: Vec<(u64, Arc<()>)>,
    wake: Mailbox<()>,
    reveals: RefCell<HashMap<String, HashMap<usize, Rc<super::reveal::PreparedReveal>>>>,
    models: rich::media::model::ModelState,
    model_ids: RefCell<BTreeMap<String, BTreeSet<String>>>,
    model_hover: RefCell<HashMap<String, Rc<Cell<bool>>>>,
    audio: rich::media::audio::AudioState,
    audio_ids: RefCell<BTreeMap<String, BTreeSet<String>>>,
    media_services: RefCell<HashMap<String, MediaServices>>,
    media_invalidations: RefCell<HashMap<String, Rc<dyn Fn()>>>,
    images: RefCell<BTreeMap<String, ImageEntry>>,
    image_ids: RefCell<BTreeMap<String, BTreeSet<String>>>,
    image_comparisons: RefCell<BTreeMap<String, ImageComparisonEntry>>,
    image_comparison_ids: RefCell<BTreeMap<String, BTreeSet<String>>>,
    annotated: RefCell<HashMap<String, Rc<RefCell<rich::code::annotated::view::AnnotatedState>>>>,
    annotated_ids: RefCell<BTreeMap<String, BTreeSet<String>>>,
    file_trees: RefCell<HashMap<String, Rc<RefCell<rich::code::file_tree::FileTreeState>>>>,
    file_tree_ids: RefCell<BTreeMap<String, BTreeSet<String>>>,
    file_tree_focus: Rc<RefCell<HashMap<String, FileTreeFocus>>>,
    comparisons: CompareState,
    comparison_ids: RefCell<BTreeMap<String, BTreeSet<String>>>,
    stats: StatsState,
    stats_ids: RefCell<BTreeMap<String, BTreeSet<String>>>,
    callouts: CalloutState,
    callout_ids: RefCell<BTreeMap<String, BTreeSet<String>>>,
    heatmaps: HeatmapState,
    heatmap_ids: RefCell<BTreeMap<String, BTreeSet<String>>>,
    funnels: FunnelState,
    funnel_ids: RefCell<BTreeMap<String, BTreeSet<String>>>,
    spans: SpansState,
    spans_ids: RefCell<BTreeMap<String, BTreeSet<String>>>,
    hierarchies: HierarchyState,
    hierarchy_ids: RefCell<BTreeMap<String, BTreeSet<String>>>,
    sankeys: SankeyState,
    sankey_ids: RefCell<BTreeMap<String, BTreeSet<String>>>,
    charts: ChartState,
    chart_ids: RefCell<BTreeMap<String, BTreeSet<String>>>,
    concentrations: ConcentrationState,
    distributions: DistributionState,
    distribution_ids: RefCell<BTreeMap<String, BTreeSet<String>>>,
    concentration_ids: RefCell<BTreeMap<String, BTreeSet<String>>>,
    copies: Rc<RefCell<HashMap<String, HashMap<String, crate::ui::code_copy::CodeCopyController>>>>,
    diagrams: RefCell<HashMap<String, Rc<RefCell<super::diagram::view::DiagramState>>>>,
}

impl std::fmt::Debug for MarkdownPresenter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MarkdownPresenter").finish_non_exhaustive()
    }
}

struct RetainedRich {
    id: u64,
    source: CodeFence,
    prepared: RichFence,
}

#[derive(Clone, Copy)]
pub struct BodyServices<'a> {
    pub glyph: &'a rich::narrative::callout::GlyphFactory,
    pub invalidate: &'a Rc<dyn Fn()>,
    pub charts: Option<&'a HeatmapServices>,
    pub code: Option<&'a CodeServices>,
}

impl std::fmt::Debug for BodyServices<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BodyServices")
            .field("charts", &self.charts.is_some())
            .field("code", &self.code.is_some())
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RevealSlot {
    pub start: usize,
    pub last: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct SelectionPlace<'a> {
    pub document: SelectionDocumentId,
    pub order: &'a [u64],
}

#[derive(Clone, Copy)]
struct RichCall<'a> {
    entry: &'a RetainedRich,
    envelope: &'a rich::Envelope,
    id: &'a str,
    theme: &'a ArtisanTheme,
    glyph: &'a rich::narrative::callout::GlyphFactory,
    invalidate: &'a Rc<dyn Fn()>,
    heatmap: Option<&'a HeatmapServices>,
    code: Option<&'a CodeServices>,
    reference_scope: Option<&'a rich::actions::ResourceScope>,
}

impl Default for MarkdownPresenter {
    fn default() -> Self {
        Self {
            source: String::new(),
            marks: None,
            document: None,
            projected: None,
            stored_leaves: RefCell::new(HashMap::new()),
            selection_owner: SelectionBatchOwner::new(),
            selection_revision: Cell::new(0),
            rich: Vec::new(),
            fence_tokens: super::FenceTokenCache::default(),
            next_rich_id: 0,
            proof: Arc::new(()),
            leaf_proofs: Vec::new(),
            wake: Mailbox::new(NonZeroUsize::MIN),
            reveals: RefCell::new(HashMap::new()),
            models: rich::media::model::ModelState::default(),
            model_ids: RefCell::new(BTreeMap::new()),
            model_hover: RefCell::new(HashMap::new()),
            audio: rich::media::audio::AudioState::default(),
            audio_ids: RefCell::new(BTreeMap::new()),
            media_services: RefCell::new(HashMap::new()),
            media_invalidations: RefCell::new(HashMap::new()),
            images: RefCell::new(BTreeMap::new()),
            image_ids: RefCell::new(BTreeMap::new()),
            image_comparisons: RefCell::new(BTreeMap::new()),
            image_comparison_ids: RefCell::new(BTreeMap::new()),
            annotated: RefCell::new(HashMap::new()),
            annotated_ids: RefCell::new(BTreeMap::new()),
            file_trees: RefCell::new(HashMap::new()),
            file_tree_ids: RefCell::new(BTreeMap::new()),
            file_tree_focus: Rc::new(RefCell::new(HashMap::new())),
            comparisons: CompareState::default(),
            comparison_ids: RefCell::new(BTreeMap::new()),
            stats: StatsState::default(),
            stats_ids: RefCell::new(BTreeMap::new()),
            callouts: CalloutState::default(),
            callout_ids: RefCell::new(BTreeMap::new()),
            heatmaps: HeatmapState::default(),
            heatmap_ids: RefCell::new(BTreeMap::new()),
            funnels: FunnelState::default(),
            funnel_ids: RefCell::new(BTreeMap::new()),
            spans: SpansState::default(),
            spans_ids: RefCell::new(BTreeMap::new()),
            hierarchies: HierarchyState::default(),
            hierarchy_ids: RefCell::new(BTreeMap::new()),
            sankeys: SankeyState::default(),
            sankey_ids: RefCell::new(BTreeMap::new()),
            charts: ChartState::default(),
            chart_ids: RefCell::new(BTreeMap::new()),
            concentrations: ConcentrationState::default(),
            distributions: DistributionState::default(),
            distribution_ids: RefCell::new(BTreeMap::new()),
            concentration_ids: RefCell::new(BTreeMap::new()),
            copies: Rc::new(RefCell::new(HashMap::new())),
            diagrams: RefCell::new(HashMap::new()),
        }
    }
}

fn fence_key(fence: &CodeFence) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    (&fence.language, fence.source.as_str(), fence.closed).hash(&mut hasher);
    hasher.finish()
}

impl MarkdownPresenter {
    pub fn is_current(&self, proof: &PreparationProof) -> bool {
        self.projected.is_some() && Arc::ptr_eq(&self.proof, &proof.0)
    }

    pub fn wake_mailbox(&self) -> &Mailbox<()> {
        &self.wake
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
}

fn enveloped(theme: &ArtisanTheme, envelope: &rich::Envelope, content: Element) -> Element {
    let current_theme = *theme;
    let text: charts::heatmap::EnvelopeTextFactory = Rc::new(move |value: &str, role, color| {
        rich::narrative::callout::text(&current_theme, value, role, color)
    });
    charts::heatmap::envelope(
        theme,
        envelope.title.as_deref(),
        envelope.subtitle.as_deref(),
        envelope.caption.as_deref(),
        content,
        &text,
    )
}

#[cfg(test)]
#[path = "../../../tests/markdown/presenter.rs"]
mod tests;
