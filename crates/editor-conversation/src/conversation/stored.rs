use super::controller::Conversation;
use super::presentation::{BodyKey, PresentationScope};
use super::projection::BodyProjection;
use editor_core::{Core, host::ScopedSourceProvider};
use editor_ui::markdown::literal::{LiteralOwner, LiteralPreparation};
use editor_ui::markdown::presenter::{
    BodyServices, HeatmapServices, MarkdownPresenter, RevealSlot, SelectionPlace,
    StoredProjectedMarkdown,
};
use editor_ui::markdown::stored::{
    LeafPreparation, PAGE_LEAVES, PagePreparation, StoredOwner, StoredPreparation, StoredSource,
};
use editor_ui::markdown::visual::BodyContext;

pub const ROW_WINDOW: usize = 16 * PAGE_LEAVES;
const WINDOW_EDGE: usize = 4 * PAGE_LEAVES;
const WINDOW_SHIFT: usize = 8 * PAGE_LEAVES;

pub fn row_window(
    current: Option<&std::ops::Range<usize>>,
    count: usize,
    reached: Option<usize>,
) -> std::ops::Range<usize> {
    let len = ROW_WINDOW.min(count);
    let start = match (current, reached) {
        (None, _) => count - len,
        (Some(window), Some(reached))
            if window.start > 0 && reached < window.start + WINDOW_EDGE =>
        {
            window.start.saturating_sub(WINDOW_SHIFT)
        }
        (Some(window), Some(reached))
            if window.end < count && reached + WINDOW_EDGE >= window.end =>
        {
            window.start + WINDOW_SHIFT
        }
        (Some(window), _) => window.start,
    };
    let start = start.min(count - len);
    start..start + len
}

pub fn retired_leaves(
    window: &std::ops::Range<usize>,
    retained: impl IntoIterator<Item = usize>,
    mounted: impl Fn(usize) -> bool,
) -> Vec<usize> {
    let mut retired: Vec<usize> = retained
        .into_iter()
        .filter(|ordinal| !window.contains(ordinal) && !mounted(*ordinal))
        .collect();
    retired.sort_unstable();
    retired.dedup();
    retired
}

use aqua_text::document::SelectionSourceId;
use aqua_ui::collection::SelectionSourceBatch;
use artisan_api::{ContentRef, SourceIndexFormat, TextContent};
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::rc::{Rc, Weak};
use std::sync::Arc;

#[derive(Debug, thiserror::Error)]
pub enum StoredBodyError {
    #[error("the stored conversation belongs to another thread or host")]
    StaleScope,
    #[error("stored body {key:?} changed under leaf {ordinal}")]
    StaleLeaf { key: BodyKey, ordinal: usize },
    #[error("stored body {key:?} has not been prepared")]
    Unprepared { key: BodyKey },
    #[error("present leaf {ordinal} of stored body {key:?}")]
    Present {
        key: BodyKey,
        ordinal: usize,
        #[source]
        source: editor_ui::markdown::presenter::PresenterError,
    },
}

#[derive(Debug, Clone)]
pub struct StoredProof {
    scope: PresentationScope,
    key: BodyKey,
    source: StoredSource,
    leaf: editor_ui::markdown::stored::LeafProof,
    ordinal: usize,
    seq: artisan_api::Seq,
}

impl StoredProof {
    pub(crate) fn accepts(
        &self,
        owner: &StoredConversation,
        scope: &PresentationScope,
        row: &super::fold::Row,
        item: &artisan_api::Item,
    ) -> bool {
        self.ordinal + 1 == self.source.block_count() && self.accepts_row(owner, scope, row, item)
    }

    pub(crate) fn accepts_row(
        &self,
        owner: &StoredConversation,
        scope: &PresentationScope,
        row: &super::fold::Row,
        item: &artisan_api::Item,
    ) -> bool {
        self.scope == *scope
            && owner.scope.as_ref() == Some(scope)
            && item.id == self.key.item
            && item.seq == self.seq
            && matches!(row.semantic(), super::fold::Row::Text { id, slot, block, .. } if *id == self.key.item && *slot == self.key.slot && *block == self.ordinal)
            && row.visual().is_none()
            && matches!(self.key.slot.text(item), Some(TextContent::Stored(reference)) if reference == self.source.source_ref())
            && self.source.format() == SourceIndexFormat::Markdown
            && owner.owner.descriptor(&self.source, self.ordinal).is_some()
            && owner.owner.source_is_current(&self.source)
            && owner.owner.leaf_is_current(&self.leaf)
    }
}

struct Body {
    owner: String,
    source: ContentRef,
    version: u64,
    window: Option<std::ops::Range<usize>>,
    presenter: MarkdownPresenter,
    ids: HashMap<usize, Vec<SelectionSourceId>>,
    batches: HashMap<usize, Arc<SelectionSourceBatch>>,
    leases: HashMap<usize, Rc<StoredLeafLease>>,
}

struct PendingRelease {
    batch: Arc<SelectionSourceBatch>,
    _leaf: Rc<editor_ui::markdown::stored::StoredLeaf>,
    key: BodyKey,
    ordinal: usize,
}

pub(crate) struct StoredLeafTarget<'a> {
    pub(crate) key: &'a BodyKey,
    pub(crate) source: &'a StoredSource,
    pub(crate) ordinal: usize,
    pub(crate) leaf: Rc<editor_ui::markdown::stored::StoredLeaf>,
}

pub(crate) struct StoredLeafServices<'a> {
    pub(crate) invalidate: &'a Rc<dyn Fn()>,
    pub(crate) reveal: Option<Rc<editor_ui::markdown::reveal::PreparedReveal>>,
    pub(crate) media: Option<editor_ui::markdown::presenter::MediaServices>,
    pub(crate) code_emit: Rc<dyn Fn(super::presentation::CodeAction)>,
    pub(crate) instance: editor_core::HostInstance,
    pub(crate) reduced_motion: bool,
}

pub(crate) struct StoredLeafLease {
    leaf: Rc<editor_ui::markdown::stored::StoredLeaf>,
    batch: Arc<SelectionSourceBatch>,
    cleanup: Rc<RefCell<Vec<PendingRelease>>>,
    key: BodyKey,
    ordinal: usize,
}

impl Drop for StoredLeafLease {
    fn drop(&mut self) {
        self.cleanup.borrow_mut().push(PendingRelease {
            batch: Arc::clone(&self.batch),
            _leaf: Rc::clone(&self.leaf),
            key: self.key.clone(),
            ordinal: self.ordinal,
        });
    }
}

impl Body {
    fn release(&self) {
        self.presenter.release(&self.owner);
    }
}

#[derive(Default)]
pub struct StoredConversation {
    scope: Option<PresentationScope>,
    owner: StoredOwner,
    literal: LiteralOwner,
    bodies: BTreeMap<BodyKey, Body>,
    cleanup: Rc<RefCell<Vec<PendingRelease>>>,
    live: Vec<Weak<StoredLeafLease>>,
    order: VecDeque<(BodyKey, usize)>,
}

impl std::fmt::Debug for StoredConversation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StoredConversation").finish_non_exhaustive()
    }
}

impl StoredConversation {
    pub fn rebind(&mut self, scope: Option<PresentationScope>) {
        if self.scope != scope {
            self.owner.clear();
            self.literal.clear();
            self.order.clear();
            for body in std::mem::take(&mut self.bodies).into_values() {
                body.release();
            }
            self.scope = scope;
        }
    }

    #[cfg(feature = "qualification")]
    pub fn wake_mailbox(&self) -> Option<&aqua::mailbox::Mailbox<()>> {
        self.owner.wake_mailbox()
    }

    pub fn ensure(
        &mut self,
        scope: &PresentationScope,
        provider: &ScopedSourceProvider,
        key: BodyKey,
        source: ContentRef,
        version: u64,
    ) -> Result<StoredPreparation, StoredBodyError> {
        if self.scope.as_ref() != Some(scope)
            || !provider.is_available()
            || provider.host() != &scope.host
            || provider.instance() != scope.instance
        {
            return Err(StoredBodyError::StaleScope);
        }
        let format = if key.slot.literal() {
            SourceIndexFormat::Literal
        } else {
            SourceIndexFormat::Markdown
        };
        let name = format!("{}:{}:index", key.item, key.slot);
        let result = self
            .owner
            .ensure(&name, provider.clone(), source.clone(), format);
        if self
            .bodies
            .get(&key)
            .is_some_and(|body| body.source != source)
        {
            if let Some(body) = self.bodies.remove(&key) {
                body.release();
            }
            self.order.retain(|(existing, _)| existing != &key);
        }
        let body = self.bodies.entry(key).or_insert_with(|| Body {
            owner: name,
            source,
            version,
            window: None,
            presenter: MarkdownPresenter::default(),
            ids: HashMap::new(),
            batches: HashMap::new(),
            leases: HashMap::new(),
        });
        body.version = version;
        Ok(result)
    }

    fn retire_outside(&mut self, key: &BodyKey, window: &std::ops::Range<usize>) {
        let Some(body) = self.bodies.get_mut(key) else {
            return;
        };
        let retired = retired_leaves(
            window,
            body.leases
                .keys()
                .chain(body.ids.keys())
                .chain(body.batches.keys())
                .copied(),
            |ordinal| {
                body.leases
                    .get(&ordinal)
                    .is_some_and(|lease| Rc::strong_count(lease) > 1)
            },
        );
        for ordinal in retired {
            body.presenter.release_stored_leaf(&body.owner, ordinal);
            body.presenter
                .set_projected_reveal(&body.owner, ordinal, None);
            body.ids.remove(&ordinal);
            body.batches.remove(&ordinal);
            body.leases.remove(&ordinal);
        }
        let leases = &body.leases;
        self.order
            .retain(|(other, ordinal)| other != key || leases.contains_key(ordinal));
    }

    pub fn drain_cleanup(&mut self, app: &mut aqua::UiApp) {
        self.live.retain(|lease| lease.strong_count() > 0);
        let current: std::collections::HashSet<_> = self
            .live
            .iter()
            .filter_map(Weak::upgrade)
            .flat_map(|lease| {
                lease
                    .batch
                    .sources()
                    .iter()
                    .map(|source| (lease.batch.document(), source.id))
                    .collect::<Vec<_>>()
            })
            .collect();
        let active: Vec<_> = self.live.iter().filter_map(Weak::upgrade).collect();
        let released = std::mem::take(&mut *self.cleanup.borrow_mut());
        for release in released {
            let batch = &release.batch;
            if !active.iter().any(|lease| {
                lease.batch.document() == batch.document() && lease.batch.owner() == batch.owner()
            }) {
                app.release_selection_batch_owner(batch.document(), batch.owner());
            }
            for source in batch.sources() {
                if !current.contains(&(batch.document(), source.id)) {
                    app.unregister_selection_source_in(batch.document(), source.id);
                }
            }
            if !active
                .iter()
                .any(|lease| lease.key == release.key && lease.ordinal == release.ordinal)
                && let Some(body) = self.bodies.get(&release.key)
                && !body.leases.contains_key(&release.ordinal)
            {
                body.presenter
                    .release_stored_leaf(&body.owner, release.ordinal);
                body.presenter
                    .set_projected_reveal(&body.owner, release.ordinal, None);
            }
        }
    }

    pub fn bind(&self, ctx: &aqua::AppCtx<'_>, window: aqua::prelude::AquaWindowId) {
        if let Some(wake) = self.owner.wake_mailbox() {
            ctx.bind_mailbox(window, wake);
        }
        if let Some(wake) = self.literal.wake_mailbox() {
            ctx.bind_mailbox(window, wake);
        }
        for body in self.bodies.values() {
            ctx.bind_mailbox(window, body.presenter.wake_mailbox());
            body.presenter.bind_media(ctx, window);
        }
    }

    pub(crate) fn sync_after_layout(&self, app: &mut aqua::UiApp) -> bool {
        let mut changed = false;
        for body in self.bodies.values() {
            {
                let id = &body.owner;
                for (block, key, row) in body.presenter.heatmap_reveals(id) {
                    if app.reveal_collection_item(key.as_str(), row) {
                        body.presenter.heatmap_revealed(&block, &key, row);
                        changed = true;
                    }
                }
                for (block, key, row) in body.presenter.funnel_reveals(id) {
                    if app.reveal_collection_item(key.as_str(), row) {
                        body.presenter.funnel_revealed(&block, &key, row);
                        changed = true;
                    }
                }
                for (block, key, row) in body.presenter.distribution_reveals(id) {
                    if app.reveal_collection_item(key.as_str(), row) {
                        body.presenter.distribution_revealed(&block, &key, row);
                        changed = true;
                    }
                }
                for (block, key, row) in body.presenter.spans_reveals(id) {
                    if app.reveal_collection_item(key.as_str(), row) {
                        body.presenter.spans_revealed(&block, &key, row);
                        changed = true;
                    }
                }
                for (block, collection, row, index) in body.presenter.file_tree_focus_requests(id) {
                    changed |= app.reveal_collection_item(collection.as_str(), index);
                    if app.focus_key(row.as_str()) {
                        body.presenter
                            .file_tree_focused(&block, &collection, &row, index);
                        changed = true;
                    }
                }
            }
        }
        changed
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one stored leaf render: lease reuse, media, preparation, code services and the 128-leaf eviction"
    )]
    pub(crate) fn render_leaf(
        &mut self,
        target: StoredLeafTarget<'_>,
        context: BodyContext<'_>,
        place: SelectionPlace<'_>,
        allocate: &dyn Fn() -> SelectionSourceId,
        services: StoredLeafServices<'_>,
    ) -> Result<(StoredProjectedMarkdown, Rc<StoredLeafLease>), StoredBodyError> {
        let StoredLeafTarget {
            key,
            source,
            ordinal,
            leaf,
        } = target;
        let StoredLeafServices {
            invalidate,
            reveal,
            media,
            code_emit,
            instance,
            reduced_motion,
        } = services;
        let theme = context.theme;
        if !self.owner.source_is_current(source) || !self.owner.leaf_is_current(leaf.proof()) {
            return Err(StoredBodyError::StaleLeaf {
                key: key.clone(),
                ordinal,
            });
        }
        let failed = |source| StoredBodyError::Present {
            key: key.clone(),
            ordinal,
            source,
        };
        let body = self
            .bodies
            .get_mut(key)
            .ok_or_else(|| StoredBodyError::Unprepared { key: key.clone() })?;
        if body
            .leases
            .get(&ordinal)
            .is_some_and(|old| !Rc::ptr_eq(&old.leaf, &leaf))
        {
            body.presenter.release_stored_leaf(&body.owner, ordinal);
            body.leases.remove(&ordinal);
            body.batches.remove(&ordinal);
            body.ids.remove(&ordinal);
        }
        let leaf_owner = &body.owner;
        if let Some(media) = media {
            body.presenter
                .set_media_services(leaf_owner, media)
                .map_err(failed)?;
        }
        let ids = body.ids.entry(ordinal).or_default();
        body.presenter
            .set_projected_reveal(leaf_owner, ordinal, reveal);
        body.presenter
            .prepare_stored_leaf(
                &self.owner,
                leaf_owner,
                ordinal,
                Rc::clone(&leaf),
                theme,
                place.document,
                place.order,
                |index| {
                    while ids.len() <= index {
                        ids.push(allocate());
                    }
                    ids[index]
                },
            )
            .map_err(failed)?;
        let charts = HeatmapServices::new(*theme);
        let copy = Rc::clone(&code_emit);
        let reference = code_emit;
        let code = body.presenter.code_services(
            leaf_owner,
            theme,
            reduced_motion,
            Rc::new(move |effect| copy(super::presentation::CodeAction::Copy(effect))),
            true,
            Some(editor_ui::markdown::presenter::ReferenceServices {
                instance: Some(instance),
                open: Rc::new(move |action, captured| {
                    if captured == Some(instance) {
                        reference(super::presentation::CodeAction::Reference(action));
                    }
                }),
            }),
        );
        let rendered = body
            .presenter
            .render_stored_leaf(
                &self.owner,
                leaf_owner,
                ordinal,
                RevealSlot {
                    start: 0,
                    last: ordinal + 1 == source.block_count(),
                },
                context,
                BodyServices {
                    glyph: &editor_ui::markdown::glyph_painter(),
                    invalidate,
                    charts: Some(&charts),
                    code: Some(&code),
                },
            )
            .map_err(failed)?;
        let lease = match body.leases.get(&ordinal) {
            Some(existing) if Arc::ptr_eq(&existing.batch, &rendered.selection_batch) => {
                Rc::clone(existing)
            }
            _ => {
                let lease = Rc::new(StoredLeafLease {
                    leaf,
                    batch: Arc::clone(&rendered.selection_batch),
                    cleanup: Rc::clone(&self.cleanup),
                    key: key.clone(),
                    ordinal,
                });
                self.live.push(Rc::downgrade(&lease));
                body.leases.insert(ordinal, Rc::clone(&lease));
                lease
            }
        };
        body.batches
            .insert(ordinal, Arc::clone(&rendered.selection_batch));
        self.order
            .retain(|(other, row)| other != key || *row != ordinal);
        self.order.push_back((key.clone(), ordinal));
        while self.order.len() > 128 {
            if let Some((old_key, old_ordinal)) = self.order.pop_front()
                && let Some(old) = self.bodies.get_mut(&old_key)
            {
                if old
                    .leases
                    .get(&old_ordinal)
                    .is_none_or(|lease| Rc::strong_count(lease) == 1)
                {
                    old.presenter.release_stored_leaf(&old.owner, old_ordinal);
                    old.presenter
                        .set_projected_reveal(&old.owner, old_ordinal, None);
                }
                old.ids.remove(&old_ordinal);
                old.batches.remove(&old_ordinal);
                old.leases.remove(&old_ordinal);
            }
        }
        Ok((rendered, lease))
    }

    pub fn leaf(
        &mut self,
        key: &BodyKey,
        source: &StoredSource,
        ordinal: usize,
    ) -> LeafPreparation {
        let leaf_key = format!("{}:{}:leaf:{ordinal}", key.item, key.slot);
        self.owner.leaf(&leaf_key, source, ordinal)
    }

    pub(crate) fn literal_leaf(
        &mut self,
        key: &BodyKey,
        source: &StoredSource,
        ordinal: usize,
    ) -> LiteralPreparation {
        let leaf_key = format!("{}:{}:leaf:{ordinal}", key.item, key.slot);
        self.literal
            .prepare(&self.owner, &leaf_key, source, ordinal)
    }

    pub fn block_count(&self, key: &BodyKey) -> Option<usize> {
        let body = self.bodies.get(key)?;
        match self.owner.current(&body.owner) {
            StoredPreparation::Ready(source) if self.owner.source_is_current(&source) => {
                Some(source.block_count())
            }
            StoredPreparation::Pending
            | StoredPreparation::Ready(_)
            | StoredPreparation::Failed(_) => None,
        }
    }

    pub(crate) fn page(
        &mut self,
        key: &BodyKey,
        source: &StoredSource,
        ordinal: usize,
    ) -> (PagePreparation, Option<std::ops::Range<usize>>) {
        let moved = self.bodies.get_mut(key).and_then(|body| {
            let window = row_window(body.window.as_ref(), source.block_count(), Some(ordinal));
            (body.window.as_ref() != Some(&window)).then(|| {
                body.window = Some(window.clone());
                window
            })
        });
        (self.owner.page(source, ordinal), moved)
    }

    pub(crate) fn retry_page(&mut self, key: &BodyKey, offset: u64) -> bool {
        self.bodies
            .get(key)
            .is_some_and(|body| self.owner.retry_page(&body.owner, offset))
    }

    pub(crate) fn proof(
        &self,
        scope: &PresentationScope,
        key: &BodyKey,
        source: &StoredSource,
        ordinal: usize,
        seq: artisan_api::Seq,
        leaf: &editor_ui::markdown::stored::LeafProof,
    ) -> Option<StoredProof> {
        (self.scope.as_ref() == Some(scope)
            && self.owner.source_is_current(source)
            && self.owner.leaf_is_current(leaf))
        .then(|| StoredProof {
            scope: scope.clone(),
            key: key.clone(),
            source: source.clone(),
            leaf: leaf.clone(),
            ordinal,
            seq,
        })
    }

    pub fn drain_publish(&mut self, conversation: &mut Conversation, core: &Core) -> bool {
        let Some(scope) = self.scope.clone() else {
            return false;
        };
        if !scope.current(conversation, core) {
            return false;
        }
        let updates = self.owner.drain();
        let literal_changed = self.literal.drain(&self.owner);
        let mut media_changed = false;
        for body in self.bodies.values_mut() {
            media_changed |= body.presenter.drain_media();
        }
        let mut projections = Vec::new();
        let mut windows = Vec::new();
        self.bodies.retain(|key, body| {
            let valid = conversation
                .view
                .as_ref()
                .and_then(|view| view.item(&key.item))
                .and_then(|item| key.slot.text(item))
                .is_some_and(
                    |text| matches!(text, TextContent::Stored(source) if source == &body.source),
                );
            if !valid {
                self.owner.release(&body.owner);
                body.release();
                return false;
            }
            if let StoredPreparation::Ready(source) = self.owner.current(&body.owner)
                && self.owner.source_is_current(&source)
                && conversation.body_version(key) == Some(body.version)
            {
                let window = row_window(body.window.as_ref(), source.block_count(), None);
                body.window = Some(window.clone());
                windows.push((key.clone(), window.clone()));
                projections.push((
                    key.clone(),
                    BodyProjection {
                        version: body.version,
                        revision: source.revision(),
                        leaves: window,
                    },
                ));
            }
            true
        });
        for (key, window) in windows {
            self.retire_outside(&key, &window);
        }
        self.order.retain(|(key, _)| self.bodies.contains_key(key));
        let mut changed = !updates.is_empty() || media_changed || literal_changed;
        changed |= conversation.publish_projections(core, &scope, projections);
        changed
    }
}
