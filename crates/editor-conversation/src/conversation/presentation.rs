use super::controller::Conversation;
use super::fold::Slot;
use aqua::ui_app::UiApp;
use aqua_text::document::{SelectionDocumentId, SelectionSource, SelectionSourceId};
use aqua_ui::element::Element;
use artisan_api::{ItemId, ThreadId};
use editor_core::{Core, HostId, HostInstance};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use aqua_core::GpuiHsla;
use editor_assets::AssetId;
use editor_ui::markdown::MarkdownEngine;
use editor_ui::markdown::presenter::{
    CodeServices, HeatmapServices, MarkdownPresenter, MediaServices, PreparationProof,
};
use editor_ui::markdown::visual::{BodyTone, LinkAction};
use editor_ui::theme::ArtisanTheme;

fn prepare_body(
    presenter: &mut MarkdownPresenter,
    engine: &MarkdownEngine,
    source: &str,
    slot: Slot,
) -> Result<PreparationProof, editor_ui::markdown::presenter::PresenterError> {
    match slot {
        Slot::Text(super::fold::TextSection::User) => presenter.prepare_message(engine, source),
        _ => presenter.prepare(engine, source),
    }
}

#[derive(Debug, thiserror::Error)]
pub enum BodyError {
    #[error("the conversation presentation belongs to another thread or host")]
    StaleScope,
    #[error("conversation body {key:?} gave two selection sources one identity")]
    DuplicateSources { key: BodyKey },
    #[error("conversation body {key:?} reused another body's selection source")]
    ForeignSources { key: BodyKey },
    #[error("register the selection sources of conversation body {key:?}")]
    Register {
        key: BodyKey,
        #[source]
        source: aqua_text::document::SelectionSourceError,
    },
    #[error("conversation body {key:?} has not been prepared")]
    Unprepared { key: BodyKey },
    #[error("conversation body {key:?} changed while it was rendered")]
    StaleBody { key: BodyKey },
    #[error("present conversation body {key:?}")]
    Present {
        key: BodyKey,
        #[source]
        source: editor_ui::markdown::presenter::PresenterError,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct BodyKey {
    pub item: ItemId,
    pub slot: Slot,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PresentationScope {
    pub host: HostId,
    pub instance: HostInstance,
    pub thread: ThreadId,
    pub agent: Option<ItemId>,
    pub generation: u64,
}

impl PresentationScope {
    pub fn capture(conversation: &Conversation, core: &Core) -> Option<Self> {
        if !conversation.host_is_current(core) {
            return None;
        }
        let (host, thread) = conversation.open.as_ref()?;
        Some(Self {
            host: host.clone(),
            instance: conversation.host_instance()?,
            thread: thread.clone(),
            agent: conversation.agent.clone(),
            generation: conversation.generation(),
        })
    }

    pub fn current(&self, conversation: &Conversation, core: &Core) -> bool {
        Self::capture(conversation, core).as_ref() == Some(self)
    }
}

pub struct PreparedElement {
    pub element: Element,
    pub sources: Vec<SelectionSource>,
}

pub(crate) struct PreparedProjectedRow {
    pub row: aqua_ui::collection::BatchedCollectionRow,
    pub geometry: editor_ui::markdown::presenter::ProjectedGeometry,
    pub text: Vec<editor_ui::markdown::presenter::ProjectedRevealText>,
    pub proof: ProjectedProof,
}

#[derive(Clone)]
pub struct ProjectedProof {
    key: BodyKey,
    source: std::rc::Rc<str>,
    row: usize,
    range: std::ops::Range<usize>,
    proof: PreparationProof,
}

impl ProjectedProof {
    pub(crate) fn matches_row(&self, row: &super::fold::Row) -> bool {
        matches!(row.semantic(), super::fold::Row::Text { id, slot, block, .. }
            if *id == self.key.item && *slot == self.key.slot && *block == self.row)
    }
}

pub struct SelectionOwners {
    document: SelectionDocumentId,
    scope: Option<PresentationScope>,
    bodies: HashMap<BodyKey, HashSet<SelectionSourceId>>,
}

impl SelectionOwners {
    pub fn new(document: SelectionDocumentId) -> Self {
        Self {
            document,
            scope: None,
            bodies: HashMap::new(),
        }
    }

    fn document(&self) -> SelectionDocumentId {
        self.document
    }

    pub fn rebind(&mut self, app: &mut UiApp, scope: Option<PresentationScope>) {
        if self.scope == scope {
            return;
        }
        self.clear(app);
        self.scope = scope;
    }

    pub fn mount(
        &mut self,
        app: &mut UiApp,
        scope: &PresentationScope,
        key: BodyKey,
        prepared: PreparedElement,
    ) -> Result<Element, BodyError> {
        if self.scope.as_ref() != Some(scope) {
            return Err(BodyError::StaleScope);
        }
        let ids: HashSet<_> = prepared.sources.iter().map(|source| source.id).collect();
        if ids.len() != prepared.sources.len() {
            return Err(BodyError::DuplicateSources { key });
        }
        if !self
            .bodies
            .iter()
            .all(|(other, sources)| *other == key || sources.is_disjoint(&ids))
        {
            return Err(BodyError::ForeignSources { key });
        }
        app.register_selection_sources_in(self.document, prepared.sources)
            .map_err(|source| BodyError::Register {
                key: key.clone(),
                source,
            })?;
        if let Some(previous) = self.bodies.insert(key, ids.clone()) {
            for source in previous.difference(&ids) {
                app.unregister_selection_source_in(self.document, *source);
            }
        }
        Ok(prepared.element)
    }

    pub fn retain(&mut self, app: &mut UiApp, mut retained: impl FnMut(&BodyKey) -> bool) {
        self.bodies.retain(|key, sources| {
            if retained(key) {
                return true;
            }
            for source in sources.iter() {
                app.unregister_selection_source_in(self.document, *source);
            }
            false
        });
    }

    pub fn clear(&mut self, app: &mut UiApp) {
        for sources in self.bodies.drain().map(|(_, sources)| sources) {
            for source in sources {
                app.unregister_selection_source_in(self.document, source);
            }
        }
        self.scope = None;
    }
}

struct MarkdownBody {
    presenter: MarkdownPresenter,
    id: String,
    source: std::rc::Rc<str>,
    ids: Vec<SelectionSourceId>,
    rendered: Option<PreparationProof>,
    revision: u64,
    projected: HashMap<usize, (std::ops::Range<usize>, PreparationProof)>,
    batch: Option<std::sync::Arc<aqua_ui::collection::SelectionSourceBatch>>,
    prepared_item_seq: Option<artisan_api::Seq>,
    preparation: Option<PreparationProof>,
}

pub struct MarkdownOwners {
    scope: Option<PresentationScope>,
    selection: SelectionOwners,
    bodies: HashMap<BodyKey, MarkdownBody>,
    media: HashMap<BodyKey, (String, MediaServices)>,
}

pub struct MarkdownRender<'a> {
    pub key: BodyKey,
    pub id: &'a str,
    pub source: &'a str,
    pub theme: &'a ArtisanTheme,
    pub tone: BodyTone,
    pub order: &'a [u64],
    pub glyph: Rc<dyn Fn(AssetId, f32, GpuiHsla) -> Element>,
    pub invalidate: Rc<dyn Fn()>,
    pub on_link: Rc<dyn Fn(LinkAction)>,
    pub on_color: Rc<dyn Fn(editor_ui::rich::actions::ShowCopyMenu)>,
    pub reference_scope: Option<editor_ui::rich::actions::ResourceScope>,
    pub heatmap: &'a HeatmapServices,
}

pub enum CodeAction {
    Copy(editor_ui::code_copy::CopyEffect),
    Reference(editor_ui::rich::actions::OpenReference),
}

pub struct ScopedCodeAction {
    scope: PresentationScope,
    key: BodyKey,
    id: String,
    revision: u64,
    stored: Option<(
        std::rc::Weak<std::cell::RefCell<super::stored::StoredConversation>>,
        super::stored::StoredProof,
    )>,
    action: CodeAction,
}

impl ScopedCodeAction {
    pub(crate) fn stored(
        scope: PresentationScope,
        key: BodyKey,
        owner: std::rc::Weak<std::cell::RefCell<super::stored::StoredConversation>>,
        proof: super::stored::StoredProof,
        action: CodeAction,
    ) -> Self {
        Self {
            scope,
            key,
            id: String::new(),
            revision: 0,
            stored: Some((owner, proof)),
            action,
        }
    }
    pub fn accept(
        self,
        conversation: &Conversation,
        core: &Core,
        owners: &MarkdownOwners,
    ) -> Option<CodeAction> {
        if !self.scope.current(conversation, core) {
            return None;
        }
        if let Some((owner, proof)) = self.stored {
            let owner = owner.upgrade()?;
            let item = conversation.view.as_ref()?.item(&self.key.item)?;
            let valid = conversation
                .rows
                .iter()
                .any(|(row, _)| proof.accepts_row(&owner.borrow(), &self.scope, row, item));
            return valid.then_some(self.action);
        }
        if owners.scope.as_ref() != Some(&self.scope) {
            return None;
        }
        let body = owners.bodies.get(&self.key)?;
        (body.id == self.id && body.revision == self.revision).then_some(self.action)
    }
}

impl MarkdownOwners {
    pub fn new(document: SelectionDocumentId) -> Self {
        Self {
            scope: None,
            selection: SelectionOwners::new(document),
            bodies: HashMap::new(),
            media: HashMap::new(),
        }
    }

    pub fn rebind(&mut self, app: &mut UiApp, scope: Option<PresentationScope>) {
        if self.scope != scope {
            for body in self.bodies.values() {
                body.presenter.release(&body.id);
                if let Some(batch) = &body.batch {
                    app.release_selection_batch_owner(batch.document(), batch.owner());
                }
            }
            self.bodies.clear();
            self.media.clear();
            self.selection.rebind(app, scope.clone());
            self.scope = scope;
        }
    }

    pub(crate) fn set_realization_media(
        &mut self,
        scope: &PresentationScope,
        key: &BodyKey,
        id: &str,
        services: MediaServices,
    ) -> Result<(), BodyError> {
        if self.scope.as_ref() != Some(scope) {
            return Err(BodyError::StaleScope);
        }
        let body = self
            .bodies
            .get(key)
            .ok_or_else(|| BodyError::Unprepared { key: key.clone() })?;
        if body.id != id {
            return Err(BodyError::StaleBody { key: key.clone() });
        }
        body.presenter
            .set_media_services(id, services)
            .map_err(|source| BodyError::Present {
                key: key.clone(),
                source,
            })
    }

    pub fn render(
        &mut self,
        app: &mut UiApp,
        scope: &PresentationScope,
        engine: &MarkdownEngine,
        request: MarkdownRender<'_>,
        mut allocate: impl FnMut() -> SelectionSourceId,
    ) -> Result<Element, BodyError> {
        if self.scope.as_ref() != Some(scope) {
            return Err(BodyError::StaleScope);
        }
        let body = self
            .bodies
            .entry(request.key.clone())
            .or_insert_with(|| MarkdownBody {
                presenter: MarkdownPresenter::default(),
                id: request.id.to_owned(),
                source: std::rc::Rc::from(""),
                ids: Vec::new(),
                rendered: None,
                revision: 0,
                projected: HashMap::new(),
                batch: None,
                prepared_item_seq: None,
                preparation: None,
            });
        if body.id != request.id {
            body.presenter.release(&body.id);
            if let Some(batch) = body.batch.take() {
                app.release_selection_batch_owner(batch.document(), batch.owner());
            }
            body.id = request.id.to_owned();
            body.rendered = None;
            body.projected.clear();
            body.prepared_item_seq = None;
            body.preparation = None;
            self.selection.retain(app, |key| key != &request.key);
        }
        if *body.source != *request.source {
            body.rendered = None;
            body.prepared_item_seq = None;
            body.preparation = None;
            body.revision = body.revision.wrapping_add(1);
            self.selection.retain(app, |key| key != &request.key);
            body.source = std::rc::Rc::from(request.source);
        }
        body.preparation = Some(
            prepare_body(
                &mut body.presenter,
                engine,
                request.source,
                request.key.slot,
            )
            .map_err(|source| BodyError::Present {
                key: request.key.clone(),
                source,
            })?,
        );
        body.presenter
            .prepare_document(engine)
            .map_err(|source| BodyError::Present {
                key: request.key.clone(),
                source,
            })?;
        let presenter = &body.presenter;
        body.projected
            .retain(|row, (range, proof)| presenter.projected_is_current(*row, range, proof));
        body.rendered = None;
        let rendered = body
            .presenter
            .render_with_services(
                request.id,
                request.theme,
                request.tone,
                self.selection.document(),
                request.order,
                |ordinal| {
                    while body.ids.len() <= ordinal {
                        body.ids.push(allocate());
                    }
                    body.ids[ordinal]
                },
                request.glyph,
                request.invalidate,
                Some(request.on_link),
                Some(request.on_color),
                Some(request.heatmap),
                request.reference_scope,
            )
            .map_err(|source| BodyError::Present {
                key: request.key.clone(),
                source,
            })?;
        let element = self.selection.mount(
            app,
            scope,
            request.key,
            PreparedElement {
                element: rendered.element,
                sources: rendered.selection_sources,
            },
        )?;
        body.rendered = Some(rendered.proof);
        Ok(element)
    }

    pub fn prepare_on_realization(
        &mut self,
        scope: &PresentationScope,
        key: BodyKey,
        id: &str,
        engine: &MarkdownEngine,
        source: &str,
        item_seq: artisan_api::Seq,
    ) -> Result<(usize, u64), BodyError> {
        if self.scope.as_ref() != Some(scope) {
            return Err(BodyError::StaleScope);
        }
        let slot = key.slot;
        let media = self.media.get(&key).cloned();
        let failed = {
            let key = key.clone();
            move |source| BodyError::Present { key, source }
        };
        let replaced = BodyError::StaleBody { key: key.clone() };
        let body = self.bodies.entry(key).or_insert_with(|| MarkdownBody {
            presenter: MarkdownPresenter::default(),
            id: id.to_owned(),
            source: std::rc::Rc::from(""),
            ids: Vec::new(),
            rendered: None,
            revision: 0,
            projected: HashMap::new(),
            batch: None,
            prepared_item_seq: None,
            preparation: None,
        });
        if body.id != id {
            return Err(replaced);
        }
        if let Some((media_id, services)) = media {
            if media_id != id {
                return Err(replaced);
            }
            body.presenter
                .set_media_services(id, services)
                .map_err(failed.clone())?;
        }
        if *body.source != *source {
            body.rendered = None;
            body.prepared_item_seq = None;
            body.preparation = None;
            body.revision = body.revision.wrapping_add(1);
            body.source = std::rc::Rc::from(source);
        }
        body.preparation =
            Some(prepare_body(&mut body.presenter, engine, source, slot).map_err(failed)?);
        let presenter = &body.presenter;
        body.projected
            .retain(|row, (range, proof)| presenter.projected_is_current(*row, range, proof));
        body.prepared_item_seq = Some(item_seq);
        let count = body
            .presenter
            .projected_count()
            .expect("invariant: a prepared Markdown body has a projection");
        Ok((count, body.revision))
    }

    pub(crate) fn prepare_projected_row_with_geometry(
        &mut self,
        scope: &PresentationScope,
        request: MarkdownRender<'_>,
        row: usize,
        code: Option<&CodeServices>,
        reveal: Option<Rc<editor_ui::markdown::reveal::PreparedReveal>>,
        mut allocate: impl FnMut() -> SelectionSourceId,
    ) -> Result<PreparedProjectedRow, BodyError> {
        if self.scope.as_ref() != Some(scope) {
            return Err(BodyError::StaleScope);
        }
        let body = self
            .bodies
            .get_mut(&request.key)
            .ok_or_else(|| BodyError::Unprepared {
                key: request.key.clone(),
            })?;
        if body.id != request.id || *body.source != *request.source {
            return Err(BodyError::StaleBody {
                key: request.key.clone(),
            });
        }
        body.presenter
            .set_projected_reveal(request.id, row, reveal.clone());
        body.projected.remove(&row);
        let rendered = body
            .presenter
            .render_projected_with_code(
                row,
                request.id,
                request.theme,
                request.tone,
                self.selection.document(),
                request.order,
                |ordinal| {
                    while body.ids.len() <= ordinal {
                        body.ids.push(allocate());
                    }
                    body.ids[ordinal]
                },
                request.glyph,
                request.invalidate,
                Some(request.on_link),
                Some(request.on_color),
                request.reference_scope,
                Some(request.heatmap),
                code,
            )
            .map_err(|source| BodyError::Present {
                key: request.key.clone(),
                source,
            })?;
        if body
            .batch
            .as_ref()
            .is_none_or(|batch| batch.identity() != rendered.selection_batch.identity())
        {
            let ids: HashSet<_> = rendered
                .selection_batch
                .sources()
                .iter()
                .map(|source| source.id)
                .collect();
            if ids.len() != rendered.selection_batch.sources().len() {
                return Err(BodyError::DuplicateSources {
                    key: request.key.clone(),
                });
            }
            if !self
                .selection
                .bodies
                .iter()
                .all(|(other, sources)| *other == request.key || sources.is_disjoint(&ids))
            {
                return Err(BodyError::ForeignSources {
                    key: request.key.clone(),
                });
            }
            self.selection.bodies.insert(request.key.clone(), ids);
            body.batch = Some(rendered.selection_batch.clone());
        }
        let text = reveal
            .as_deref()
            .map_or_else(Vec::new, |reveal| rendered.reveal_text(reveal));
        let proof = ProjectedProof {
            key: request.key.clone(),
            source: body.source.clone(),
            row,
            range: rendered.source_range.clone(),
            proof: rendered.proof.clone(),
        };
        let geometry = rendered.geometry;
        let prepared = aqua_ui::collection::BatchedCollectionRow {
            element: rendered.element,
            batches: vec![rendered.selection_batch],
        };
        body.projected
            .insert(row, (rendered.source_range, rendered.proof));
        Ok(PreparedProjectedRow {
            row: prepared,
            geometry,
            text,
            proof,
        })
    }

    pub(crate) fn code_services(
        &self,
        scope: &PresentationScope,
        key: &BodyKey,
        theme: ArtisanTheme,
        reduced_motion: bool,
        copy_enabled: bool,
        emit: Rc<dyn Fn(ScopedCodeAction)>,
    ) -> Result<CodeServices, BodyError> {
        if self.scope.as_ref() != Some(scope) {
            return Err(BodyError::StaleScope);
        }
        let body = self
            .bodies
            .get(key)
            .ok_or_else(|| BodyError::Unprepared { key: key.clone() })?;
        let bind = {
            let scope = scope.clone();
            let key = key.clone();
            let id = body.id.clone();
            let revision = body.revision;
            Rc::new(move |action| {
                emit(ScopedCodeAction {
                    scope: scope.clone(),
                    key: key.clone(),
                    id: id.clone(),
                    revision,
                    stored: None,
                    action,
                })
            })
        };
        let copy = Rc::clone(&bind);
        let reference = Rc::clone(&bind);
        let scope_instance = scope.instance;
        Ok(body.presenter.code_services(
            &body.id,
            theme,
            reduced_motion,
            Rc::new(move |effect| copy(CodeAction::Copy(effect))),
            copy_enabled,
            Some(editor_ui::markdown::presenter::ReferenceServices {
                instance: Some(scope.instance),
                open: Rc::new(move |action, instance| {
                    if instance == Some(scope_instance) {
                        reference(CodeAction::Reference(action));
                    }
                }),
            }),
        ))
    }

    pub fn projected_range(&self, key: &BodyKey, row: usize) -> Option<std::ops::Range<usize>> {
        Some(self.bodies.get(key)?.projected.get(&row)?.0.clone())
    }

    pub(crate) fn projected_leaf_info(
        &self,
        scope: &PresentationScope,
        key: &BodyKey,
        row: usize,
    ) -> Option<editor_ui::markdown::presenter::ProjectedLeafInfo> {
        if self.scope.as_ref() != Some(scope) {
            return None;
        }
        self.bodies.get(key)?.presenter.projected_leaf_info(row)
    }

    pub(crate) fn projected_leaf_is_current(
        &self,
        scope: &PresentationScope,
        key: &BodyKey,
        row: usize,
        info: &editor_ui::markdown::presenter::ProjectedLeafInfo,
    ) -> bool {
        self.scope.as_ref() == Some(scope)
            && self
                .bodies
                .get(key)
                .is_some_and(|body| body.presenter.projected_leaf_is_current(row, info))
    }

    pub fn prepared_body_current(
        &self,
        scope: &PresentationScope,
        key: &BodyKey,
        source: &str,
        seq: artisan_api::Seq,
        row: usize,
    ) -> bool {
        self.scope.as_ref() == Some(scope)
            && self.bodies.get(key).is_some_and(|body| {
                *body.source == *source
                    && body.prepared_item_seq == Some(seq)
                    && body
                        .presenter
                        .projected_count()
                        .is_some_and(|count| row.checked_add(1) == Some(count))
                    && body
                        .preparation
                        .as_ref()
                        .is_some_and(|proof| body.presenter.is_current(proof))
            })
    }

    pub fn capture_projected(&self, key: &BodyKey, row: usize) -> Option<ProjectedProof> {
        let body = self.bodies.get(key)?;
        if row + 1 != body.presenter.projected_count()? {
            return None;
        }
        self.capture_projected_row(key, row)
    }

    fn capture_projected_row(&self, key: &BodyKey, row: usize) -> Option<ProjectedProof> {
        let body = self.bodies.get(key)?;
        let (range, proof) = body.projected.get(&row)?;
        Some(ProjectedProof {
            key: key.clone(),
            source: body.source.clone(),
            row,
            range: range.clone(),
            proof: proof.clone(),
        })
    }

    pub(crate) fn release_projected_reveal(&mut self, proof: &ProjectedProof) {
        if let Some(body) = self.bodies.get(&proof.key) {
            body.presenter
                .set_projected_reveal(&body.id, proof.row, None);
        }
    }

    pub fn accepts_projected(
        &self,
        scope: &PresentationScope,
        proof: &ProjectedProof,
        source: &str,
    ) -> bool {
        *proof.source == *source
            && self.accepts_projected_row(scope, proof)
            && self.bodies.get(&proof.key).is_some_and(|body| {
                body.source == proof.source
                    && proof.row + 1 == body.presenter.projected_count().unwrap_or(0)
            })
    }

    pub(crate) fn accepts_projected_row(
        &self,
        scope: &PresentationScope,
        proof: &ProjectedProof,
    ) -> bool {
        self.scope.as_ref() == Some(scope)
            && self.bodies.get(&proof.key).is_some_and(|body| {
                body.presenter
                    .projected_is_current(proof.row, &proof.range, &proof.proof)
            })
    }

    pub fn projected_current(
        &self,
        scope: &PresentationScope,
        key: &BodyKey,
        source: &str,
        row: usize,
        range: &std::ops::Range<usize>,
    ) -> bool {
        self.scope.as_ref() == Some(scope)
            && self.bodies.get(key).is_some_and(|body| {
                *body.source == *source
                    && body.projected.get(&row).is_some_and(|(actual, proof)| {
                        actual == range && body.presenter.projected_is_current(row, range, proof)
                    })
            })
    }

    pub fn retain(&mut self, app: &mut UiApp, mut retained: impl FnMut(&BodyKey) -> bool) {
        self.bodies.retain(|key, body| {
            if retained(key) {
                true
            } else {
                body.presenter.release(&body.id);
                if let Some(batch) = &body.batch {
                    app.release_selection_batch_owner(batch.document(), batch.owner());
                }
                false
            }
        });
        self.media.retain(|key, _| retained(key));
        self.selection
            .retain(app, |key| self.bodies.contains_key(key));
    }

    pub fn presenters(&self) -> impl Iterator<Item = (&BodyKey, &str, &MarkdownPresenter)> {
        self.bodies
            .iter()
            .map(|(key, body)| (key, body.id.as_str(), &body.presenter))
    }

    pub fn bind_media(&self, ctx: &aqua::AppCtx<'_>, window: aqua::prelude::AquaWindowId) {
        for body in self.bodies.values() {
            body.presenter.bind_media(ctx, window);
        }
    }

    pub fn drain_media(&self) -> bool {
        self.bodies.values().fold(false, |changed, body| {
            body.presenter.drain_media() || changed
        })
    }

    pub fn reveal_heatmaps(&self, app: &mut UiApp) -> bool {
        let mut changed = false;
        for body in self.bodies.values() {
            for (block, key, row) in body.presenter.heatmap_reveals(&body.id) {
                if app.reveal_collection_item(key.as_str(), row) {
                    body.presenter.heatmap_revealed(&block, &key, row);
                    changed = true;
                }
            }
            for (block, key, row) in body.presenter.funnel_reveals(&body.id) {
                if app.reveal_collection_item(key.as_str(), row) {
                    body.presenter.funnel_revealed(&block, &key, row);
                    changed = true;
                }
            }
            for (block, key, row) in body.presenter.distribution_reveals(&body.id) {
                if app.reveal_collection_item(key.as_str(), row) {
                    body.presenter.distribution_revealed(&block, &key, row);
                    changed = true;
                }
            }
            for (block, key, row) in body.presenter.spans_reveals(&body.id) {
                if app.reveal_collection_item(key.as_str(), row) {
                    body.presenter.spans_revealed(&block, &key, row);
                    changed = true;
                }
            }
            for (block, collection, row, index) in body.presenter.file_tree_focus_requests(&body.id)
            {
                changed |= app.reveal_collection_item(collection.as_str(), index);
                if app.focus_key(row.as_str()) {
                    body.presenter
                        .file_tree_focused(&block, &collection, &row, index);
                    changed = true;
                }
            }
        }
        changed
    }
}
