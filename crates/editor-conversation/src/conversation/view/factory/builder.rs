use super::super::status;
use super::{
    BlobAction, BlobResources, ConfigureRowsError, RealizedRow, RevealResources, RowFactory,
    RowSource, SnapshotRow, SourceResources, StoredResources, WorkResources,
};
use crate::conversation::fold::{Row, RowKey};
use crate::conversation::presentation::{
    BodyKey, MarkdownOwners, PresentationScope, ScopedCodeAction,
};
use aqua_text::document::SelectionSourceId;
use editor_ui::markdown::visual::LinkAction;
use editor_ui::rich::actions::ShowCopyMenu;
use editor_ui::theme::ArtisanTheme;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Instant;

impl RowFactory {
    pub fn new(
        rows: Vec<RealizedRow>,
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
        dispatch: super::super::Dispatch,
        work: WorkResources,
    ) -> Self {
        statuses.borrow_mut().set_scope(&scope);
        {
            let mut motion = work.motion.borrow_mut();
            motion.rebind_scope(&scope);
            motion.begin_frame();
        }
        let mut row_cache = super::super::row_cache::RowCache::default();
        row_cache.rebind(Some(scope.clone()));
        Self {
            rows: RowSource::Realized(rows),
            scope,
            markdown,
            statuses,
            theme,
            reduced_motion,
            now,
            allocate,
            invalidate,
            on_link,
            on_color,
            on_code,
            dispatch,
            copy: None,
            work,
            engine: Rc::new(RefCell::new(None)),
            projection_versions: HashMap::new(),
            shape_counts: HashMap::new(),
            pending_projections: RefCell::new(Vec::new()),
            pending_live: RefCell::new(Vec::new()),
            observations: None,
            admission: None,
            reveals: None,
            media: None,
            blob_images: None,
            source_resources: None,
            stored: None,
            row_cache: Rc::new(RefCell::new(row_cache)),
            realized_owners: RefCell::new(HashMap::new()),
            active: Rc::new(Cell::new(true)),
            live: Cell::new(true),
            settled: Rc::default(),
            style_epoch: 0,
            fingerprints: RefCell::default(),
            live_texts: RefCell::default(),
            inline_texts: RefCell::default(),
            unsettled: Cell::new(false),
            content_keyed: Cell::new(false),
        }
    }

    pub fn with_active(mut self, active: Rc<Cell<bool>>) -> Self {
        self.active = active;
        self
    }

    pub fn with_settled(
        mut self,
        settled: Rc<RefCell<HashMap<RowKey, u64>>>,
        style_epoch: u64,
    ) -> Self {
        self.settled = settled;
        self.style_epoch = style_epoch;
        self.fingerprints.get_mut().clear();
        self
    }

    pub fn with_row_cache(mut self, cache: Rc<RefCell<super::super::row_cache::RowCache>>) -> Self {
        cache.borrow_mut().rebind(Some(self.scope.clone()));
        self.row_cache = cache;
        self
    }

    pub fn with_sources(
        mut self,
        provider: editor_core::host::ScopedSourceProvider,
        document: aqua_text::document::SelectionDocumentId,
        emit: Rc<dyn Fn(super::super::row_sources::RowSourceAction)>,
    ) -> Result<Self, ConfigureRowsError> {
        if !(provider.is_available()
            && provider.host() == &self.scope.host
            && provider.instance() == self.scope.instance)
        {
            return Err(ConfigureRowsError::StaleSourceProvider);
        }
        self.source_resources = Some(SourceResources {
            provider,
            document,
            emit,
        });
        Ok(self)
    }

    pub fn with_stored(
        mut self,
        owner: Rc<RefCell<crate::conversation::stored::StoredConversation>>,
        provider: editor_core::host::ScopedSourceProvider,
    ) -> Result<Self, ConfigureRowsError> {
        if !(provider.is_available()
            && provider.host() == &self.scope.host
            && provider.instance() == self.scope.instance)
        {
            return Err(ConfigureRowsError::StaleStoredScope);
        }
        owner.borrow_mut().rebind(Some(self.scope.clone()));
        self.stored = Some(StoredResources { owner, provider });
        Ok(self)
    }

    pub fn with_copy(mut self, copy: Rc<dyn Fn(super::super::source_actions::CopySource)>) -> Self {
        self.copy = Some(copy);
        self
    }

    pub fn with_blob_images(
        mut self,
        images: editor_core::host::BlobImages,
        emit: Rc<dyn Fn(artisan_api::ItemId, u64, BlobAction)>,
    ) -> Self {
        self.blob_images = Some(BlobResources { images, emit });
        self
    }

    pub fn with_admission(
        mut self,
        admission: Rc<RefCell<crate::conversation::admission::RevealQueue>>,
    ) -> Self {
        self.admission = Some(admission);
        self
    }

    pub fn with_reveals(
        mut self,
        conversation: &crate::conversation::controller::Conversation,
        core: &editor_core::Core,
        owners: Rc<RefCell<super::super::reveal::MountedReveals>>,
        swift: bool,
    ) -> Result<Self, ConfigureRowsError> {
        if !self.scope.current(conversation, core) {
            return Err(ConfigureRowsError::StaleScope);
        }
        owners.borrow_mut().rebind(Some(self.scope.clone()));
        let mut live = HashMap::new();
        for target in conversation.view.iter().flat_map(|view| view.live.keys()) {
            if let artisan_api::LiveTarget::Text { item_id: id } = target
                && let Some(binding) =
                    crate::conversation::live::LiveBinding::capture(conversation, core, id)
            {
                live.entry(id.clone()).or_insert(binding);
            }
        }
        self.admission = Some(Rc::clone(&conversation.reveal));
        self.reveals = Some(RevealResources {
            owners,
            arrivals: Rc::clone(&conversation.arrivals),
            live,
            swift,
        });
        Ok(self)
    }

    pub fn with_lazy_rows(
        mut self,
        conversation: &crate::conversation::controller::Conversation,
        core: &editor_core::Core,
        composer_height: Option<f32>,
    ) -> Result<Self, ConfigureRowsError> {
        if !self.scope.current(conversation, core) {
            return Err(ConfigureRowsError::StaleScope);
        }
        let snapshots =
            crate::conversation::adapter::RowSnapshots::capture(conversation, composer_height)
                .ok_or(ConfigureRowsError::NoSnapshot)?;
        let frontier = conversation
            .reveal_frontier
            .as_ref()
            .and_then(|frontier| conversation.row_position(frontier));
        let mut rows = Vec::with_capacity(conversation.rows.len());
        for (index, (row, _)) in conversation.rows.iter().enumerate() {
            rows.push(SnapshotRow {
                disclosure: conversation.disclosures.row_disclosure(row),
                held: crate::conversation::admission::behind_reveal(index, row, frontier),
            });
            if let Row::Text { id, slot, .. } = row.semantic() {
                let key = BodyKey {
                    item: id.clone(),
                    slot: *slot,
                };
                if let Some(count) = conversation.body_shape_count(&key) {
                    self.shape_counts.insert(key.clone(), count);
                }
                if let Some(version) = conversation.body_version(&key) {
                    self.projection_versions.insert(key, version);
                }
            }
        }
        self.rows = RowSource::Snapshots { snapshots, rows };
        self.fingerprints.get_mut().clear();
        Ok(self)
    }

    pub fn with_media(
        mut self,
        media: Rc<dyn Fn(&BodyKey, f32) -> Option<editor_ui::markdown::presenter::MediaServices>>,
    ) -> Self {
        self.media = Some(media);
        self
    }

    pub fn with_observations(
        mut self,
        observations: super::super::transcript::TranscriptObservations,
    ) -> Self {
        self.observations = Some(observations);
        self
    }

    pub fn with_engine(
        mut self,
        engine: Rc<RefCell<Option<editor_ui::markdown::MarkdownEngine>>>,
    ) -> Self {
        self.engine = engine;
        self
    }
}
