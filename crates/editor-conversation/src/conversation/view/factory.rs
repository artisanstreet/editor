use super::status;
use crate::conversation::adapter::RowModel;
use crate::conversation::disclosure::RowDisclosure;
use crate::conversation::fold::{Row, RowKey};
use crate::conversation::presentation::{
    BodyKey, MarkdownOwners, PresentationScope, ScopedCodeAction,
};
use aqua_text::document::SelectionSourceId;
use artisan_api::Status;
use editor_ui::markdown::visual::LinkAction;
use editor_ui::rich::actions::ShowCopyMenu;
use editor_ui::theme::ArtisanTheme;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;
mod blobs;
mod bubbles;
mod builder;
mod fingerprint;
mod lifecycle;
mod prose;
mod realize;
mod requests;
mod source_fields;
mod stored_rows;
pub(crate) use self::blobs::image_caption;
pub(crate) use self::bubbles::message_author;
use self::bubbles::{authored, bubble_sender, media_width, truncate, user_bubble};
use self::stored_rows::plain_block;

type HeldByRow = (
    Rc<super::row_cache::RowOwner>,
    Option<Rc<crate::conversation::stored::StoredLeafLease>>,
);

#[derive(Debug)]
pub struct WorkResources {
    pub motion: Rc<RefCell<super::motion::RowMotion>>,
    pub names: HashMap<artisan_api::ItemId, String>,
    pub seconds: f64,
    pub timestamp: artisan_api::Timestamp,
}

struct RevealResources {
    owners: Rc<RefCell<super::reveal::MountedReveals>>,
    arrivals: Rc<
        RefCell<HashMap<artisan_api::ItemId, crate::conversation::admission::LiveBlockArrivals>>,
    >,
    live: HashMap<artisan_api::ItemId, crate::conversation::live::LiveBinding>,
    swift: bool,
}

enum RowSource {
    Realized(Vec<RealizedRow>),
    Snapshots {
        snapshots: Box<crate::conversation::adapter::RowSnapshots>,
        rows: Vec<SnapshotRow>,
    },
}

#[derive(Clone, Copy)]
struct SnapshotRow {
    disclosure: RowDisclosure,
    held: bool,
}

#[derive(Debug, Clone)]
pub enum BlobAction {
    Load {
        hash: artisan_api::BlobHash,
        retry: bool,
    },
    Open {
        hash: artisan_api::BlobHash,
        image: aqua::ImageHandle,
        opener_key: String,
    },
}

struct BlobResources {
    images: editor_core::host::BlobImages,
    emit: Rc<dyn Fn(artisan_api::ItemId, u64, BlobAction)>,
}

struct SourceResources {
    provider: editor_core::host::ScopedSourceProvider,
    document: aqua_text::document::SelectionDocumentId,
    emit: Rc<dyn Fn(super::row_sources::RowSourceAction)>,
}

struct StoredResources {
    owner: Rc<RefCell<crate::conversation::stored::StoredConversation>>,
    provider: editor_core::host::ScopedSourceProvider,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Service {
    Sources,
    SourceProvider,
    Stored,
    Images,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    Row,
    Item,
    Run,
    Source,
    SourceField,
    SourceView,
    StoredSource,
    StoredText,
    PlainText,
    LiveSource,
    LeafGeometry,
    Reveal,
    QuestionCollection,
    QuestionView,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Staleness {
    Factory,
    RowCache,
    MotionScope,
    RowIdentity,
    Scope,
    StoredProof,
    LiveSnapshot,
    LeafGeometry,
    LiteralBatch,
    AnswerBatch,
    SourceBatch,
    DetailBatch,
    QuestionBatch,
}

#[derive(Debug, thiserror::Error)]
pub enum RealizeRowError {
    #[error("the row factory was built without {0:?}")]
    Missing(Service),
    #[error("the row's {0:?} is unavailable")]
    Unavailable(Part),
    #[error("the row's {0:?} changed while it was realized")]
    Changed(Part),
    #[error("the row's {0:?} belongs to an earlier presentation")]
    Stale(Staleness),
    #[error("no renderer draws row {key:?}")]
    Unrenderable { key: RowKey },
    #[error("the transcript has not measured its width yet")]
    Unmeasured,
    #[error("the media services width does not match the measured body")]
    MediaWidth,
    #[error("start the Markdown engine")]
    Engine(#[from] editor_ui::markdown::MarkdownError),
    #[error("present the row's Markdown body")]
    Body(#[from] crate::conversation::presentation::BodyError),
    #[error("present the row's stored body")]
    Stored(#[from] crate::conversation::stored::StoredBodyError),
    #[error("batch the row's selection sources")]
    Batch(#[from] super::resources::BatchRowError),
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigureRowsError {
    #[error("the conversation presentation belongs to another thread or host")]
    StaleScope,
    #[error("the source provider belongs to another host")]
    StaleSourceProvider,
    #[error("the stored conversation belongs to another host")]
    StaleStoredScope,
    #[error("the conversation has no row snapshot")]
    NoSnapshot,
    #[error("the factory was built without transcript observations")]
    NoObservations,
}

#[derive(Debug)]
pub struct RealizedRow {
    pub model: RowModel,
    pub disclosure: RowDisclosure,
    pub held: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AfterLayout {
    pub rebuild: bool,
}

pub type MediaLookup =
    Rc<dyn Fn(&BodyKey, f32) -> Option<editor_ui::markdown::presenter::MediaServices>>;

type LiveText = (Arc<ropey::Rope>, Rc<str>);

pub struct RowFactory {
    rows: RowSource,
    scope: PresentationScope,
    markdown: Rc<RefCell<MarkdownOwners>>,
    statuses: Rc<RefCell<status::StatusShimmers>>,
    theme: ArtisanTheme,
    reduced_motion: bool,
    now: Instant,
    allocate: Rc<dyn Fn() -> SelectionSourceId>,
    invalidate: Rc<dyn Fn()>,
    on_link: Rc<dyn Fn(LinkAction)>,
    on_color: Rc<dyn Fn(ShowCopyMenu)>,
    on_code: Rc<dyn Fn(ScopedCodeAction)>,
    dispatch: super::Dispatch,
    copy: Option<Rc<dyn Fn(super::source_actions::CopySource)>>,
    work: WorkResources,
    engine: Rc<RefCell<Option<editor_ui::markdown::MarkdownEngine>>>,
    projection_versions: HashMap<BodyKey, u64>,
    shape_counts: HashMap<BodyKey, usize>,
    pending_projections: RefCell<Vec<(BodyKey, crate::conversation::projection::BodyProjection)>>,
    pending_live: RefCell<Vec<(crate::conversation::live::LiveBinding, usize)>>,
    observations: Option<super::transcript::TranscriptObservations>,
    admission: Option<Rc<RefCell<crate::conversation::admission::RevealQueue>>>,
    reveals: Option<RevealResources>,
    media: Option<MediaLookup>,
    blob_images: Option<BlobResources>,
    source_resources: Option<SourceResources>,
    stored: Option<StoredResources>,
    row_cache: Rc<RefCell<super::row_cache::RowCache>>,
    realized_owners: RefCell<HashMap<RowKey, std::rc::Weak<super::row_cache::RowOwner>>>,
    active: Rc<Cell<bool>>,
    live: Cell<bool>,
    settled: Rc<RefCell<HashMap<RowKey, u64>>>,
    style_epoch: u64,
    fingerprints: RefCell<HashMap<usize, (RowKey, Option<u64>)>>,
    live_texts: RefCell<HashMap<usize, LiveText>>,
    inline_texts: RefCell<HashMap<(artisan_api::ItemId, artisan_api::Seq), Rc<str>>>,
    unsettled: Cell<bool>,
    content_keyed: Cell<bool>,
}

impl std::fmt::Debug for RowFactory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RowFactory").finish_non_exhaustive()
    }
}

#[derive(Debug)]
pub struct RowFingerprint<'a> {
    pub model: &'a RowModel,
    pub disclosure: &'a RowDisclosure,
    pub held: bool,
    pub body_version: Option<u64>,
    pub style_epoch: u64,
}

impl RowFingerprint<'_> {
    pub fn value(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.model.row.hash(&mut hasher);
        self.model.version.hash(&mut hasher);
        self.model.gap.to_bits().hash(&mut hasher);
        self.model.end.to_bits().hash(&mut hasher);
        let mut ui: Vec<_> = self.model.ui.iter().collect();
        ui.sort_by(|a, b| a.0.as_str().cmp(b.0.as_str()));
        for (id, state) in ui {
            id.hash(&mut hasher);
            state.hash(&mut hasher);
        }
        let mut answers: Vec<_> = self.model.answers.iter().collect();
        answers.sort_by(|a, b| a.0.as_str().cmp(b.0.as_str()));
        for (id, state) in answers {
            id.hash(&mut hasher);
            state.hash(&mut hasher);
        }
        format!("{:?}", self.disclosure).hash(&mut hasher);
        self.held.hash(&mut hasher);
        self.body_version.hash(&mut hasher);
        self.style_epoch.hash(&mut hasher);
        hasher.finish()
    }
}

pub fn row_settles_statically(model: &RowModel, now: Instant) -> bool {
    let status = matches!(model.row.semantic(), Row::Status(_));
    (status
        || matches!(
            model.row.semantic(),
            Row::Text { .. }
                | Row::Chain { .. }
                | Row::WorkEntry { .. }
                | Row::Work { .. }
                | Row::WorkHeader(_)
        ))
        && model
            .items
            .values()
            .all(|item| item.status != Status::Running)
        && model.ui.values().all(|ui| !ui.fresh(now))
        && (status || model.live.is_empty())
}

impl RowFactory {
    pub const fn scope(&self) -> &PresentationScope {
        &self.scope
    }
}

#[cfg(test)]
#[path = "../../../tests/conversation/view/factory.rs"]
mod tests;
