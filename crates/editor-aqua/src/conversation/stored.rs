use super::controller::Conversation;
use super::presentation::{BodyKey, PresentationScope};
use super::projection::BodyProjection;
use crate::core::{Core, host::ScopedSourceProvider};
use crate::ui::markdown::literal::{LiteralOwner, LiteralPreparation};
use crate::ui::markdown::presenter::{HeatmapServices, MarkdownPresenter, StoredProjectedMarkdown};
use crate::ui::markdown::stored::{LeafPreparation, StoredOwner, StoredPreparation, StoredSource};
use crate::ui::markdown::visual::{BodyTone, LinkAction};
use crate::ui::theme::ArtisanTheme;

pub fn edge_offset(
    loaded: std::ops::Range<usize>,
    count: usize,
    ordinal: usize,
    top: bool,
    following_tail: bool,
    following_viewport: bool,
) -> Option<usize> {
    if !loaded.contains(&ordinal) {
        return None;
    }
    if loaded.start > 0
        && !following_tail
        && !following_viewport
        && ordinal.saturating_sub(loaded.start) < 4
        && top
    {
        Some(loaded.start.saturating_sub(32))
    } else if loaded.end < count && loaded.end.saturating_sub(ordinal) <= 4 {
        Some(loaded.start.saturating_add(32))
    } else {
        None
    }
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

pub fn reverses_in_place(
    previous_moves: u64,
    previous_top: Option<&super::fold::RowKey>,
    previous_newer: bool,
    moves: u64,
    top: Option<&super::fold::RowKey>,
    toward_end: Option<bool>,
    newer: bool,
) -> bool {
    previous_newer != newer
        && (previous_moves == moves || previous_top == top || toward_end != Some(newer))
}
use aqua_text::document::{SelectionDocumentId, SelectionSourceId};
use aqua_ui::collection::SelectionSourceBatch;
use artisan_api::{ContentRef, SourceIndexFormat, TextContent};
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::{Rc, Weak};
use std::sync::Arc;

#[derive(Clone)]
pub struct StoredProof {
    scope: PresentationScope,
    key: BodyKey,
    source: StoredSource,
    leaf: crate::ui::markdown::stored::LeafProof,
    ordinal: usize,
    seq: artisan_api::Seq,
}

impl StoredProof {
    pub fn accepts(
        &self,
        owner: &StoredConversation,
        scope: &PresentationScope,
        row: &super::fold::Row,
        item: &artisan_api::Item,
    ) -> bool {
        self.ordinal + 1 == self.source.block_count() && self.accepts_row(owner, scope, row, item)
    }

    pub fn accepts_row(
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
            && self.source.descriptor(self.ordinal).is_some()
            && owner.owner.source_is_current(&self.source)
            && owner.owner.leaf_is_current(&self.leaf)
    }
}

struct Body {
    owner: String,
    source: ContentRef,
    version: u64,
    presenter: MarkdownPresenter,
    ids: HashMap<usize, Vec<SelectionSourceId>>,
    batches: HashMap<usize, Arc<SelectionSourceBatch>>,
    leases: HashMap<usize, Rc<StoredLeafLease>>,
}

struct PendingRelease {
    batch: Arc<SelectionSourceBatch>,
    _leaf: Arc<crate::ui::markdown::stored::StoredLeaf>,
    key: BodyKey,
    ordinal: usize,
}

pub(crate) struct StoredLeafLease {
    leaf: Arc<crate::ui::markdown::stored::StoredLeaf>,
    batch: Arc<SelectionSourceBatch>,
    cleanup: Rc<RefCell<Vec<PendingRelease>>>,
    key: BodyKey,
    ordinal: usize,
}

impl Drop for StoredLeafLease {
    fn drop(&mut self) {
        self.cleanup.borrow_mut().push(PendingRelease {
            batch: self.batch.clone(),
            _leaf: self.leaf.clone(),
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
    bodies: HashMap<BodyKey, Body>,
    cleanup: Rc<RefCell<Vec<PendingRelease>>>,
    live: Vec<Weak<StoredLeafLease>>,
    order: VecDeque<(BodyKey, usize)>,
    frontiers: HashMap<BodyKey, (ContentRef, u64, Option<super::fold::RowKey>, bool)>,
}

impl StoredConversation {
    pub fn rebind(&mut self, scope: Option<PresentationScope>) {
        if self.scope != scope {
            self.owner.clear();
            self.literal.clear();
            self.frontiers.clear();
            self.order.clear();
            for (_, body) in self.bodies.drain() {
                body.release();
            }
            self.scope = scope;
        }
    }

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
    ) -> anyhow::Result<StoredPreparation> {
        anyhow::ensure!(
            self.scope.as_ref() == Some(scope)
                && provider.is_available()
                && provider.host() == &scope.host
                && provider.instance() == scope.instance,
            "stale stored conversation scope"
        );
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
        for release in std::mem::take(&mut *self.cleanup.borrow_mut()) {
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

    pub fn sync_after_layout(&self, app: &mut aqua::UiApp) -> bool {
        let mut changed = false;
        for body in self.bodies.values() {
            {
                let id = &body.owner;
                for (block, key, row) in body.presenter.heatmap_reveals(&id) {
                    if app.reveal_collection_item(key.as_str(), row) {
                        body.presenter.heatmap_revealed(&block, &key, row);
                        changed = true;
                    }
                }
                for (block, key, row) in body.presenter.funnel_reveals(&id) {
                    if app.reveal_collection_item(key.as_str(), row) {
                        body.presenter.funnel_revealed(&block, &key, row);
                        changed = true;
                    }
                }
                for (block, key, row) in body.presenter.distribution_reveals(&id) {
                    if app.reveal_collection_item(key.as_str(), row) {
                        body.presenter.distribution_revealed(&block, &key, row);
                        changed = true;
                    }
                }
                for (block, key, row) in body.presenter.spans_reveals(&id) {
                    if app.reveal_collection_item(key.as_str(), row) {
                        body.presenter.spans_revealed(&block, &key, row);
                        changed = true;
                    }
                }
                for (block, collection, row, index) in body.presenter.file_tree_focus_requests(&id)
                {
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

    pub(crate) fn render_leaf(
        &mut self,
        key: &BodyKey,
        source: &StoredSource,
        ordinal: usize,
        leaf: Arc<crate::ui::markdown::stored::StoredLeaf>,
        document: SelectionDocumentId,
        order: &[u64],
        theme: &ArtisanTheme,
        tone: BodyTone,
        allocate: &dyn Fn() -> SelectionSourceId,
        invalidate: Rc<dyn Fn()>,
        link: Rc<dyn Fn(LinkAction)>,
        color: Rc<dyn Fn(crate::ui::rich::actions::ShowCopyMenu)>,
        resource_scope: crate::ui::rich::actions::ResourceScope,
        reveal: Option<Rc<crate::ui::markdown::reveal::PreparedReveal>>,
        media: Option<crate::ui::markdown::presenter::MediaServices>,
        code_emit: Rc<dyn Fn(super::presentation::CodeAction)>,
        instance: crate::core::HostInstance,
        reduced_motion: bool,
    ) -> anyhow::Result<(StoredProjectedMarkdown, Rc<StoredLeafLease>)> {
        anyhow::ensure!(
            self.owner.source_is_current(source) && self.owner.leaf_is_current(leaf.proof()),
            "stale stored Markdown leaf"
        );
        let body = self
            .bodies
            .get_mut(key)
            .ok_or_else(|| anyhow::anyhow!("stored Markdown body is unavailable"))?;
        if body
            .leases
            .get(&ordinal)
            .is_some_and(|old| !Arc::ptr_eq(&old.leaf, &leaf))
        {
            body.presenter.release_stored_leaf(&body.owner, ordinal);
            body.leases.remove(&ordinal);
            body.batches.remove(&ordinal);
            body.ids.remove(&ordinal);
        }
        let leaf_owner = &body.owner;
        if let Some(media) = media {
            body.presenter.set_media_services(&leaf_owner, media)?;
        }
        let ids = body.ids.entry(ordinal).or_default();
        body.presenter
            .set_projected_reveal(&leaf_owner, ordinal, reveal);
        body.presenter.prepare_stored_leaf(
            &self.owner,
            &leaf_owner,
            ordinal,
            leaf.clone(),
            theme,
            document,
            order,
            |index| {
                while ids.len() <= index {
                    ids.push(allocate());
                }
                ids[index]
            },
        )?;
        let charts = HeatmapServices::new(*theme);
        let copy = code_emit.clone();
        let reference = code_emit;
        let code = body.presenter.code_services(
            &leaf_owner,
            *theme,
            reduced_motion,
            Rc::new(move |effect| copy(super::presentation::CodeAction::Copy(effect))),
            true,
            Some(crate::ui::markdown::presenter::ReferenceServices {
                instance: Some(instance),
                open: Rc::new(move |action, captured| {
                    if captured == Some(instance) {
                        reference(super::presentation::CodeAction::Reference(action));
                    }
                }),
            }),
        );
        let rendered = body.presenter.render_stored_leaf(
            &self.owner,
            &leaf_owner,
            ordinal,
            0,
            ordinal + 1 == source.block_count(),
            theme,
            tone,
            Rc::new(crate::ui::asset::source_asset_glyph),
            invalidate,
            Some(link),
            Some(color),
            Some(resource_scope),
            Some(&charts),
            Some(&code),
        )?;
        let lease = match body.leases.get(&ordinal) {
            Some(existing) if Arc::ptr_eq(&existing.batch, &rendered.selection_batch) => {
                existing.clone()
            }
            _ => {
                let lease = Rc::new(StoredLeafLease {
                    leaf,
                    batch: rendered.selection_batch.clone(),
                    cleanup: self.cleanup.clone(),
                    key: key.clone(),
                    ordinal,
                });
                self.live.push(Rc::downgrade(&lease));
                body.leases.insert(ordinal, lease.clone());
                lease
            }
        };
        body.batches
            .insert(ordinal, rendered.selection_batch.clone());
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

    pub fn literal_leaf(
        &mut self,
        key: &BodyKey,
        source: &StoredSource,
        ordinal: usize,
    ) -> LiteralPreparation {
        let leaf_key = format!("{}:{}:leaf:{ordinal}", key.item, key.slot);
        self.literal
            .prepare(&self.owner, &leaf_key, source, ordinal)
    }

    pub fn owner(&self) -> &StoredOwner {
        &self.owner
    }

    pub(crate) fn prefetch(
        &mut self,
        proof: &StoredProof,
        scope: &PresentationScope,
        row: &super::fold::Row,
        item: &artisan_api::Item,
        top: Option<super::fold::RowKey>,
        reader_moves: u64,
        toward_end: Option<bool>,
        following_tail: bool,
        following_viewport: bool,
    ) -> bool {
        if !proof.accepts_row(self, scope, row, item) {
            return false;
        }
        let loaded = proof.source.loaded_range();
        let Some(offset) = edge_offset(
            loaded.clone(),
            proof.source.block_count(),
            proof.ordinal,
            top.as_ref() == Some(&row.key()),
            following_tail,
            following_viewport,
        ) else {
            return false;
        };
        let newer = offset > loaded.start;
        if self.frontiers.get(&proof.key).is_some_and(
            |(source, moves, previous_top, previous_newer)| {
                source == proof.source.source_ref()
                    && reverses_in_place(
                        *moves,
                        previous_top.as_ref(),
                        *previous_newer,
                        reader_moves,
                        top.as_ref(),
                        toward_end,
                        newer,
                    )
            },
        ) {
            return false;
        }
        let Some(body) = self.bodies.get(&proof.key) else {
            return false;
        };
        self.frontiers.insert(
            proof.key.clone(),
            (proof.source.source_ref().clone(), reader_moves, top, newer),
        );
        matches!(
            self.owner
                .move_window(&body.owner, &proof.source, offset as u64),
            StoredPreparation::Ready(_)
        )
    }

    pub fn current_range(&self, key: &BodyKey) -> Option<std::ops::Range<usize>> {
        let body = self.bodies.get(key)?;
        match self.owner.current(&body.owner) {
            StoredPreparation::Ready(source) if self.owner.source_is_current(&source) => {
                Some(source.loaded_range())
            }
            _ => None,
        }
    }

    pub fn proof(
        &self,
        scope: &PresentationScope,
        key: &BodyKey,
        source: &StoredSource,
        ordinal: usize,
        seq: artisan_api::Seq,
        leaf: &crate::ui::markdown::stored::LeafProof,
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
                windows.push((key.clone(), source.loaded_range()));
                projections.push((
                    key.clone(),
                    BodyProjection {
                        version: body.version,
                        revision: source.revision(),
                        leaves: source.loaded_range(),
                    },
                ));
            }
            true
        });
        for (key, window) in windows {
            self.retire_outside(&key, &window);
        }
        self.frontiers
            .retain(|key, _| self.bodies.contains_key(key));
        self.order.retain(|(key, _)| self.bodies.contains_key(key));
        let mut changed = !updates.is_empty() || media_changed || literal_changed;
        changed |= conversation.publish_projections(core, &scope, projections);
        changed
    }
}
