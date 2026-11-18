use super::{row_frame, status, work};
use crate::conversation::adapter::RowModel;
use crate::conversation::disclosure::RowDisclosure;
use crate::conversation::fold::{self, Row, RowKey, Slot};
use crate::conversation::presentation::{
    BodyKey, MarkdownOwners, MarkdownRender, PresentationScope, ScopedCodeAction,
};
use aqua_text::document::SelectionSourceId;
use aqua_ui::collection::BatchedCollectionRow;
use artisan_api::{ItemKind, Status, TextContent};
use editor_ui::kit::{Space, Spaced};
use editor_ui::markdown::presenter::HeatmapServices;
use editor_ui::markdown::visual::{BodyTone, LinkAction};
use editor_ui::rich::actions::{ResourceScope, ShowCopyMenu};
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
mod requests;
mod source_fields;
mod stored_rows;
pub(crate) use self::blobs::image_caption;
pub(crate) use self::bubbles::message_author;
use self::bubbles::{authored, bubble_sender, media_width, truncate, user_bubble};
use self::stored_rows::plain_block;

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
        snapshots: crate::conversation::adapter::RowSnapshots,
        rows: Vec<SnapshotRow>,
    },
}

#[derive(Clone, Copy)]
struct SnapshotRow {
    disclosure: RowDisclosure,
    held: bool,
}

#[derive(Clone)]
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

pub struct RealizedRow {
    pub model: RowModel,
    pub disclosure: RowDisclosure,
    pub held: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AfterLayout {
    pub rebuild: bool,
}

pub struct RowFactory {
    rows: RowSource,
    pub scope: PresentationScope,
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
    media:
        Option<Rc<dyn Fn(&BodyKey, f32) -> Option<editor_ui::markdown::presenter::MediaServices>>>,
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
    live_texts: RefCell<HashMap<usize, (std::sync::Arc<ropey::Rope>, Rc<str>)>>,
    inline_texts: RefCell<HashMap<(artisan_api::ItemId, artisan_api::Seq), Rc<str>>>,
    unsettled: Cell<bool>,
    content_keyed: Cell<bool>,
}

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
    fn realize_inner(
        &self,
        index: usize,
        key: &RowKey,
        content_width: Option<f32>,
    ) -> Result<BatchedCollectionRow, RealizeRowError> {
        let _realize_span = editor_core::diagnostics::rebuild_stats::span("f_realize");
        if !self.live.get() {
            return Err(RealizeRowError::Stale(Staleness::Factory));
        }
        if !self.row_cache.borrow().scope_is(&self.scope) {
            return Err(RealizeRowError::Stale(Staleness::RowCache));
        }
        if !self.work.motion.borrow().scope_is(&self.scope) {
            return Err(RealizeRowError::Stale(Staleness::MotionScope));
        }
        let generated;
        let row = match &self.rows {
            RowSource::Snapshots { snapshots, rows } => {
                let state = rows
                    .get(index)
                    .ok_or(RealizeRowError::Unavailable(Part::Row))?;
                generated = RealizedRow {
                    model: snapshots
                        .model(index)
                        .ok_or(RealizeRowError::Unavailable(Part::Row))?,
                    disclosure: state.disclosure,
                    held: state.held,
                };
                &generated
            }
            RowSource::Realized(rows) => rows
                .get(index)
                .ok_or(RealizeRowError::Unavailable(Part::Row))?,
        };
        if &row.model.row.key() != key {
            return Err(RealizeRowError::Stale(Staleness::RowIdentity));
        }
        editor_core::diagnostics::rebuild_stats::record_event(
            if self.row_cache.borrow().get(key).is_some() {
                "row_rerealized"
            } else {
                "row_realized"
            },
        );
        self.unsettled.set(false);
        self.content_keyed.set(false);
        self.settled.borrow_mut().remove(key);
        let row_owner = self.row_cache.borrow().candidate(key);
        *row_owner.stored_leaf.borrow_mut() = None;
        row_owner.motion.borrow_mut().begin_frame();
        row_owner
            .motion
            .borrow_mut()
            .on_refit(self.invalidate.clone());
        let reveal = match (&self.reveals, row.model.row.semantic()) {
            (
                Some(resources),
                Row::Text {
                    id,
                    slot: Slot::Prose,
                    block,
                    ..
                },
            ) => {
                let initial = row.model.ui.get(id).is_some_and(|ui| ui.fresh(self.now));
                Some(resources.owners.borrow_mut().acquire(&row_owner, || {
                    resources
                        .arrivals
                        .borrow_mut()
                        .get_mut(id)
                        .map_or(initial, |arrival| arrival.claim(*block))
                }))
            }
            _ => None,
        };
        let queue_held = self.admission.as_ref().is_some_and(|queue| {
            let semantic = row.model.row.semantic();
            let position = row
                .model
                .row
                .anchor()
                .and_then(|id| row.model.items.get(id))
                .map_or(u64::MAX, |item| item.position);
            let (part, ordinal) = match semantic {
                Row::Chain { .. } => (0, 0),
                Row::Text {
                    slot: Slot::Prose | Slot::Text(_),
                    block,
                    ..
                } => (2, *block),
                _ => (1, 0),
            };
            let mut queue = queue.borrow_mut();
            let enabled = !crate::conversation::admission::immediate(semantic)
                && !queue.rushed(position)
                && !self.reduced_motion;
            queue.begin(key.clone(), (position, part, ordinal), enabled, self.now)
        });
        let deferred = row.disclosure.clip.is_none()
            && row.disclosure.fraction.is_none()
            && row.disclosure.hidden;
        if row.held || queue_held || deferred {
            if let Some((reveal, _, ready)) = &reveal {
                reveal.set_paused(true);
                ready.set(false);
            }
            if let Some(queue) = &self.admission {
                queue.borrow_mut().finish(key, row.held || queue_held);
            }
            return Ok(BatchedCollectionRow {
                element: row_frame(
                    &row.model,
                    row.disclosure,
                    row.held || queue_held,
                    None,
                    self.theme,
                ),
                batches: Vec::new(),
            });
        }
        let mut batches = Vec::new();
        let mut stored_proof = None;
        let content = match &row.model.row {
            Row::Gap | Row::EarlierWork { .. } => {
                let dispatch = &self.dispatch;
                super::load_row(&row.model.row, self.theme, Rc::clone(dispatch))
            }
            header @ (Row::Work { .. } | Row::WorkHeader(_)) => {
                let (run, segment, has_members) = match header {
                    Row::Work {
                        run,
                        segment,
                        members,
                    } => (
                        run.as_ref(),
                        *segment,
                        !members.entries().is_empty() || !members.spans().is_empty(),
                    ),
                    Row::WorkHeader(run) => (Some(run), 0, false),
                    _ => unreachable!("invariant: the arm matched only work rows"),
                };
                let work = &self.work;
                let dispatch = &self.dispatch;
                if segment != 0 || row.model.run.is_none() {
                    None
                } else {
                    let summary = row
                        .model
                        .run
                        .as_ref()
                        .ok_or(RealizeRowError::Unavailable(Part::Run))?;
                    let key = format!("work-{}", run.map_or_else(String::new, ToString::to_string));
                    let label = row_owner.motion.borrow_mut().phrase(
                        &format!("{key}-label"),
                        work::section_title(summary, work.timestamp),
                        self.theme.colors.text_muted.to_reference_text_color(),
                        self.theme,
                        work.seconds,
                        self.reduced_motion,
                    );
                    work::header(
                        &self.theme,
                        run,
                        segment,
                        has_members,
                        Some(summary),
                        label,
                        |open| {
                            row_owner.motion.borrow_mut().chevron(
                                &key,
                                open,
                                row.disclosure.fraction,
                                editor_ui::kit::IconSize::Md.px(),
                                self.theme.colors.text_muted.to_reference_paint(),
                                self.now,
                                self.reduced_motion,
                            )
                        },
                        Rc::clone(dispatch),
                    )
                }
            }
            Row::Chain { ids, .. } => {
                let work = &self.work;
                let dispatch = &self.dispatch;
                let open = row
                    .model
                    .ui
                    .get(ids.first())
                    .and_then(|ui| ui.chain_open)
                    .unwrap_or(false);
                let key = format!("chain-{}", ids.first());
                work::chain(
                    ids.as_slice(),
                    &row.model.items,
                    open,
                    self.theme,
                    |text, color| {
                        row_owner.motion.borrow_mut().phrase(
                            &format!("{key}-clause"),
                            text,
                            color,
                            self.theme,
                            work.seconds,
                            self.reduced_motion,
                        )
                    },
                    |open, color| {
                        row_owner.motion.borrow_mut().chevron(
                            &key,
                            open,
                            row.disclosure.fraction,
                            editor_ui::kit::IconSize::Md.px(),
                            color,
                            self.now,
                            self.reduced_motion,
                        )
                    },
                    Rc::clone(dispatch),
                )
            }
            Row::WorkEntry { id, .. } => {
                let work = &self.work;
                let dispatch = &self.dispatch;
                let item = row
                    .model
                    .items
                    .get(id)
                    .ok_or(RealizeRowError::Unavailable(Part::Item))?;
                let expanded = row
                    .model
                    .ui
                    .get(id)
                    .and_then(|ui| ui.expanded)
                    .unwrap_or(false);
                match &item.kind {
                    ItemKind::Agent { .. } => work::agent(
                        item,
                        self.theme,
                        |text, color| truncate(text, color, false, self.theme),
                        Rc::clone(dispatch),
                    ),
                    ItemKind::Tool { .. }
                    | ItemKind::UserMessage {
                        from_agent: Some(_),
                        ..
                    } => self.activity_row(
                        &row_owner,
                        item,
                        expanded,
                        &work.names,
                        Rc::clone(dispatch),
                        &mut batches,
                    )?,
                    ItemKind::Plan { .. } => work::plan_entry(item, self.theme),
                    ItemKind::Notice { .. } => self.notice_row(&row_owner, item, &mut batches)?,
                    ItemKind::Question { .. } => {
                        self.question_card(&row_owner, item, &mut batches)?
                    }
                    _ => return Err(RealizeRowError::Unrenderable { key: key.clone() }),
                }
            }
            Row::Text {
                id,
                slot: slot @ Slot::Plain(section),
                block,
                ..
            } if row
                .model
                .items
                .get(id)
                .is_some_and(|item| matches!(slot.text(item), Some(TextContent::Stored(_)))) =>
            {
                let item = row
                    .model
                    .items
                    .get(id)
                    .ok_or(RealizeRowError::Unavailable(Part::Item))?;
                let Some(TextContent::Stored(reference)) = slot.text(item) else {
                    return Err(RealizeRowError::Changed(Part::StoredSource));
                };
                Some(self.literal_row(
                    &row_owner,
                    item,
                    *slot,
                    *section,
                    *block,
                    reference,
                    row.model.version,
                    &mut batches,
                )?)
            }
            Row::Text {
                id, slot, block, ..
            } if matches!(slot, Slot::Prose | Slot::Text(_))
                && row.model.items.get(id).is_some_and(|item| {
                    (*slot != Slot::Prose || item.status != Status::Running)
                        && matches!(slot.text(item), Some(TextContent::Stored(_)))
                }) =>
            {
                let item = row
                    .model
                    .items
                    .get(id)
                    .ok_or(RealizeRowError::Unavailable(Part::Item))?;
                let Some(TextContent::Stored(reference)) = slot.text(item) else {
                    return Err(RealizeRowError::Changed(Part::StoredSource));
                };
                let (element, proof) = self.stored_row(
                    row,
                    &row_owner,
                    item,
                    *slot,
                    *block,
                    reference,
                    &reveal,
                    content_width,
                    &mut batches,
                )?;
                stored_proof = proof;
                Some(if slot.is_bubble() {
                    let count = self
                        .stored
                        .as_ref()
                        .and_then(|services| {
                            services.owner.borrow().block_count(&BodyKey {
                                item: id.clone(),
                                slot: *slot,
                            })
                        })
                        .unwrap_or(1);
                    let sticker = self.bubble_sticker(
                        &row_owner,
                        key,
                        row.model.ui.get(id).is_some_and(|ui| ui.fresh(self.now)),
                    );
                    user_bubble(
                        element,
                        *block,
                        count,
                        false,
                        if *block == 0 {
                            self.user_attachments(&row_owner, item)?
                        } else {
                            None
                        },
                        bubble_sender(item, row.model.author),
                        sticker,
                        self.theme,
                    )
                } else {
                    element
                })
            }
            Row::Text {
                id, slot, block: 0, ..
            } if matches!(slot, Slot::Text(_))
                && row
                    .model
                    .items
                    .get(id)
                    .is_some_and(|item| slot.inline_text(item).is_some_and(str::is_empty)) =>
            {
                use aqua_ui::element::Box as UiBox;
                use aqua_ui::style::{Length, Style};
                let item = row
                    .model
                    .items
                    .get(id)
                    .ok_or(RealizeRowError::Unavailable(Part::Item))?;
                let mut root = UiBox::new()
                    .layout("column")
                    .width(Length::Pct(100.0))
                    .spaced(Space::S8)
                    .style(Style::new().align(aqua_ui::AlignItems::END));
                if let Some(attachments) = self.user_attachments(&row_owner, item)? {
                    root = root.child(attachments);
                }
                Some(root.build())
            }
            Row::Text {
                id,
                slot: slot @ (Slot::Prose | Slot::Text(_)),
                block,
                ..
            } => 'prose: {
                let item = row
                    .model
                    .items
                    .get(id)
                    .ok_or(RealizeRowError::Unavailable(Part::Item))?;
                let running = item.status == Status::Running && *slot == Slot::Prose;
                let shape_count = self
                    .shape_counts
                    .get(&BodyKey {
                        item: id.clone(),
                        slot: *slot,
                    })
                    .copied();
                let sealed = shape_count.is_some_and(|count| *block + 1 < count);
                let fresh = *slot == Slot::Prose
                    && row.model.ui.get(id).is_some_and(|ui| ui.fresh(self.now));
                if !(reveal.is_some() || (!running && !fresh)) {
                    return Err(RealizeRowError::Unavailable(Part::Reveal));
                }
                let saved = match slot.inline_text(item) {
                    Some(source) => Some(source),
                    None if running => None,
                    None => return Err(RealizeRowError::Unavailable(Part::StoredText)),
                };
                let live = if running {
                    let bound = self
                        .reveals
                        .as_ref()
                        .and_then(|resources| resources.live.get(id));
                    match (bound, saved) {
                        (Some(binding), _) => {
                            let target = artisan_api::LiveTarget::Text {
                                item_id: id.clone(),
                            };
                            if !row
                                .model
                                .live
                                .get(&target)
                                .is_some_and(|source| std::sync::Arc::ptr_eq(source, &binding.text))
                            {
                                return Err(RealizeRowError::Stale(Staleness::LiveSnapshot));
                            }
                            Some(binding.clone())
                        }
                        (None, Some(_)) => None,
                        (None, None) => return Err(RealizeRowError::Unavailable(Part::LiveSource)),
                    }
                } else {
                    None
                };
                self.content_keyed
                    .set(shape_count.is_some() && live.is_none());
                let live_source = live.as_ref().map(|binding| self.live_text(&binding.text));
                let source = live_source
                    .as_deref()
                    .or(saved)
                    .ok_or(RealizeRowError::Unavailable(Part::LiveSource))?;
                if let (Some((owner, is_fresh, ready)), Some(resources)) = (&reveal, &self.reveals)
                {
                    ready.set(false);
                    owner.begin_frame(
                        self.now,
                        running && !sealed,
                        *is_fresh,
                        self.reduced_motion,
                        resources.swift,
                    );
                    owner.set_paused(false);
                    owner.set_rushed(
                        self.admission
                            .as_ref()
                            .is_some_and(|queue| queue.borrow().rushed(item.position)),
                    );
                }
                let body_key = BodyKey {
                    item: id.clone(),
                    slot: *slot,
                };
                let mut owners = self.markdown.borrow_mut();
                let mut engine = self.engine.borrow_mut();
                let engine = match &mut *engine {
                    Some(engine) => engine,
                    absent @ None => absent.insert(editor_ui::markdown::MarkdownEngine::new()?),
                };
                let prepare_span = editor_core::diagnostics::rebuild_stats::span("f_md_prepare");
                let (count, revision) = match owners.prepare_on_realization(
                    &self.scope,
                    body_key.clone(),
                    &id.to_string(),
                    engine,
                    source,
                    item.seq,
                ) {
                    Ok(prepared) => prepared,
                    Err(error) => match self.copy.clone() {
                        None => return Err(error.into()),
                        Some(copy) => {
                            if let Some((owner, _, _)) = &reveal {
                                owner.mark_presented();
                            }
                            let copy_source = super::source_actions::CopySource::from_parts(
                                live.as_ref().map(|binding| binding.text.clone()),
                                std::sync::Arc::from(source),
                            );
                            break 'prose Some(super::source_actions::failed_body(
                                self.theme,
                                error.to_string(),
                                format!("copy-full-{id}"),
                                copy_source,
                                copy,
                            ));
                        }
                    },
                };
                drop(prepare_span);
                let preview_notice = (*block + 1 == count)
                    .then(|| live.as_ref().and_then(|binding| binding.preview_notice()))
                    .flatten();
                let shaped = self.shape_counts.contains_key(&body_key);
                if (if shaped {
                    *block + 1 == count
                } else {
                    *block == 0
                }) && let Some(binding) = &live
                {
                    self.pending_live
                        .borrow_mut()
                        .push((binding.clone(), count));
                }
                if let (Some(width), Some(media)) = (content_width, &self.media) {
                    let info = owners
                        .projected_leaf_info(&self.scope, &body_key, *block)
                        .ok_or(RealizeRowError::Unavailable(Part::LeafGeometry))?;
                    if !owners.projected_leaf_is_current(&self.scope, &body_key, *block, &info) {
                        return Err(RealizeRowError::Stale(Staleness::LeafGeometry));
                    }
                    let width = media_width(super::reading_width(width), *slot, info.list_depth);
                    if width.is_nan() || width <= 0.0 {
                        return Err(RealizeRowError::Unmeasured);
                    }
                    if let Some(services) = media(&body_key, width) {
                        if services.width != width {
                            return Err(RealizeRowError::MediaWidth);
                        }
                        owners.set_realization_media(
                            &self.scope,
                            &body_key,
                            &id.to_string(),
                            services,
                        )?;
                    }
                }
                let code = owners.code_services(
                    &self.scope,
                    &body_key,
                    self.theme,
                    self.reduced_motion,
                    true,
                    self.on_code.clone(),
                )?;
                let order = fold::selection_order(self.scope.agent.is_some(), item.position, *slot);
                let id = id.to_string();
                let services = HeatmapServices::new(self.theme);
                let projected_span = editor_core::diagnostics::rebuild_stats::span("f_md_project");
                let prepared = owners.prepare_projected_row_with_geometry(
                    &self.scope,
                    MarkdownRender {
                        key: body_key,
                        id: &id,
                        source,
                        theme: &self.theme,
                        tone: BodyTone::Foreground,
                        order: &order,
                        glyph: editor_ui::markdown::glyph_painter(),
                        invalidate: self.invalidate.clone(),
                        on_link: self.on_link.clone(),
                        on_color: self.on_color.clone(),
                        reference_scope: Some(ResourceScope {
                            host: self.scope.host.to_string(),
                            thread: self.scope.thread.clone(),
                        }),
                        heatmap: &services,
                    },
                    *block,
                    Some(&code),
                    reveal.as_ref().map(|(owner, _, _)| owner.clone()),
                    || (self.allocate)(),
                )?;
                drop(projected_span);
                let entrance = if prepared.geometry.block_entrance {
                    reveal.as_ref().and_then(|(owner, _, _)| {
                        owner
                            .block_entrance(*block)
                            .map(|(alpha, rise)| (alpha, rise, owner.block_panel_height(*block)))
                    })
                } else {
                    None
                };
                if *block == 0 && !shaped {
                    let version = self
                        .projection_versions
                        .get(&BodyKey {
                            item: item.id.clone(),
                            slot: *slot,
                        })
                        .copied()
                        .unwrap_or(row.model.version);
                    self.pending_projections.borrow_mut().push((
                        BodyKey {
                            item: item.id.clone(),
                            slot: *slot,
                        },
                        crate::conversation::projection::BodyProjection {
                            version,
                            revision,
                            leaves: 0..count,
                        },
                    ));
                }
                let prepared = if let Some(resources) = &self.reveals
                    && reveal.is_some()
                {
                    resources.owners.borrow_mut().record(
                        &row_owner,
                        row.model.version,
                        *block,
                        prepared,
                        live.filter(|_| !sealed),
                    )
                } else {
                    prepared.row
                };
                batches = prepared.batches;
                let element = if let Some((alpha, rise, height)) = entrance {
                    use aqua_ui::element::Box as UiBox;
                    use aqua_ui::style::{Length, Style};
                    let inner = UiBox::new()
                        .width(Length::Pct(100.0))
                        .style(
                            Style::new()
                                .flex_shrink(0.0)
                                .opacity(alpha)
                                .transform(aqua_core::Transform2D::translate(0.0, rise)),
                        )
                        .child(prepared.element)
                        .build();
                    UiBox::new()
                        .layout("column")
                        .width(Length::Pct(100.0))
                        .style(Style::new().height(Length::Px(height)).clip(true))
                        .child(inner)
                        .build()
                } else {
                    prepared.element
                };
                let element = if let Some((owner, _, _)) = &reveal {
                    owner.mark_presented();
                    if !owner.fully_visible() && !self.reduced_motion {
                        use aqua_ui::element::Box as UiBox;
                        use aqua_ui::style::{Length, Style};
                        UiBox::new()
                            .layout("column")
                            .width(Length::Pct(100.0))
                            .style(
                                Style::new()
                                    .height(Length::Px(owner.visible_height()))
                                    .clip(true),
                            )
                            .child(
                                UiBox::new()
                                    .width(Length::Pct(100.0))
                                    .style(Style::new().flex_shrink(0.0))
                                    .child(element)
                                    .build(),
                            )
                            .build()
                    } else {
                        element
                    }
                } else {
                    element
                };
                Some(if slot.is_bubble() {
                    let fresh = row
                        .model
                        .ui
                        .get(&item.id)
                        .is_some_and(|ui| ui.fresh(self.now));
                    user_bubble(
                        element,
                        *block,
                        count,
                        source.is_empty(),
                        if *block == 0 {
                            self.user_attachments(&row_owner, item)?
                        } else {
                            None
                        },
                        bubble_sender(item, row.model.author),
                        self.bubble_sticker(&row_owner, key, fresh),
                        self.theme,
                    )
                } else if let Some(notice) = preview_notice {
                    aqua_ui::element::Box::new()
                        .width(aqua_ui::style::Length::Pct(100.0))
                        .layout("column")
                        .child(element)
                        .child(editor_ui::page::notice(&self.theme, notice).build())
                        .build()
                } else {
                    element
                })
            }
            Row::Prompt(id) => {
                let item = row
                    .model
                    .items
                    .get(id)
                    .ok_or(RealizeRowError::Unavailable(Part::Item))?;
                if matches!(&item.kind, ItemKind::Question { .. }) {
                    self.question_card(&row_owner, item, &mut batches)?
                } else {
                    let dispatch = &self.dispatch;
                    let approval = self.approval_card(
                        &row_owner,
                        item,
                        row.model.answers.get(id),
                        Rc::clone(dispatch),
                        &mut batches,
                    )?;
                    if approval.is_some() {
                        approval
                    } else {
                        let work = &self.work;
                        match &item.kind {
                            ItemKind::Tool { .. }
                            | ItemKind::UserMessage {
                                from_agent: Some(_),
                                ..
                            } => self.activity_row(
                                &row_owner,
                                item,
                                row.model
                                    .ui
                                    .get(id)
                                    .and_then(|ui| ui.expanded)
                                    .unwrap_or(false),
                                &work.names,
                                Rc::clone(dispatch),
                                &mut batches,
                            )?,
                            ItemKind::Agent { .. } => work::agent(
                                item,
                                self.theme,
                                |text, color| truncate(text, color, false, self.theme),
                                Rc::clone(dispatch),
                            ),
                            ItemKind::Plan { .. } => work::plan_entry(item, self.theme),
                            ItemKind::Notice { .. } => {
                                self.notice_row(&row_owner, item, &mut batches)?
                            }
                            _ => None,
                        }
                    }
                }
            }
            Row::Image(id) => {
                let item = row
                    .model
                    .items
                    .get(id)
                    .ok_or(RealizeRowError::Unavailable(Part::Item))?;
                let ItemKind::Image { blob, .. } = &item.kind else {
                    return Err(RealizeRowError::Changed(Part::Item));
                };
                let width = super::reading_width(content_width.ok_or(RealizeRowError::Unmeasured)?);
                if width.is_nan() || width <= 0.0 {
                    return Err(RealizeRowError::Unmeasured);
                }
                let image = self.blob_frame(&row_owner, item, blob, None, width)?;
                Some(
                    aqua_ui::element::Box::new()
                        .layout("column")
                        .spaced(Space::S6)
                        .child(image)
                        .build(),
                )
            }
            Row::Compaction(id) => {
                use crate::conversation::compaction::CompactionPhase;
                let item = row
                    .model
                    .items
                    .get(id)
                    .ok_or(RealizeRowError::Unavailable(Part::Item))?;
                let ItemKind::Compaction { .. } = &item.kind else {
                    return Err(RealizeRowError::Changed(Part::Item));
                };
                let phase = match item.status {
                    Status::Running => CompactionPhase::Running,
                    Status::Done => CompactionPhase::Done,
                    Status::AwaitingUser
                    | Status::Failed
                    | Status::Declined
                    | Status::Cancelled => CompactionPhase::Stopped,
                };
                let fresh = row.model.ui.get(id).is_some_and(|ui| ui.fresh(self.now));
                let frame = row_owner.motion.borrow_mut().compaction(
                    id,
                    phase,
                    fresh,
                    self.now,
                    self.reduced_motion,
                );
                let shimmer_key = (!self.reduced_motion)
                    .then(|| aqua_ui::tree::hash_key(("compaction-shimmer", format!("{key:?}"))));
                super::compaction::row(item, frame, self.theme, |phase, switch, frame, theme| {
                    super::compaction::label(phase, switch, frame, theme, shimmer_key)
                })
            }
            Row::AgentResult(_) => None,
            Row::Status(run) => status::run(&row.model, run, self.theme, |text, live, theme| {
                let label = match &text {
                    status::StatusText::Status(label) => Some(label.clone()),
                    status::StatusText::Reasoning(_) => None,
                };
                let element = self.statuses.borrow_mut().render(
                    key.clone(),
                    text,
                    live,
                    self.reduced_motion,
                    theme,
                );
                let now = self.now;
                let swap = row_owner.motion.borrow_mut().swap(
                    format!("status-{key:?}"),
                    label,
                    self.reduced_motion,
                    now,
                );
                match swap {
                    Some(progress) => super::motion::swapped(element, progress),
                    None => element,
                }
            }),
            Row::AgentStatus(_, doing) => {
                status::agent(doing.as_deref(), self.theme, |text, theme| {
                    self.statuses
                        .borrow_mut()
                        .agent(key.clone(), text, self.reduced_motion, theme)
                })
            }
            Row::Text {
                id,
                slot: Slot::Plain(section),
                block,
                ..
            } => {
                let item = row
                    .model
                    .items
                    .get(id)
                    .ok_or(RealizeRowError::Unavailable(Part::Item))?;
                let Some(text) = section
                    .inline_text(item)
                    .and_then(|text| plain_block(text, *block))
                else {
                    return Err(RealizeRowError::Changed(Part::PlainText));
                };
                Some(match section {
                    fold::PlainSection::ImageCaption => image_caption(text, self.theme),
                    _ => aqua_ui::element::Text::new(text.to_owned()).build(),
                })
            }
            _ => return Err(RealizeRowError::Unrenderable { key: key.clone() }),
        };
        let semantic = row.model.row.semantic();
        let author = message_author(
            semantic,
            semantic
                .anchor()
                .and_then(|id| row.model.items.get(id))
                .map(Arc::as_ref),
            row.model.author,
        );
        let content = match (content, author) {
            (Some(element), Some(author)) => Some(authored(element, author)),
            (content, _) => content,
        };
        let mut work_settling = false;
        let content = match content {
            Some(element) => {
                let (id, standalone) = match row.model.row.semantic() {
                    Row::WorkEntry { id, .. } => (Some(id), false),
                    Row::Chain { ids, .. } => (Some(ids.first()), false),
                    Row::Prompt(id) | Row::Image(id) | Row::AgentResult(id) => (Some(id), true),
                    Row::Text {
                        id, slot, block: 0, ..
                    } if slot.is_card() => (Some(id), true),
                    _ => (None, false),
                };
                if let Some(id) = id {
                    let fresh = row.disclosure.clip.is_none()
                        && row.model.ui.get(id).is_some_and(|ui| ui.fresh(self.now));
                    let live = standalone || row.model.run.as_ref().is_some_and(|run| run.live());
                    let entrance = row_owner.motion.borrow_mut().entrance(
                        key.clone(),
                        fresh,
                        live,
                        self.reduced_motion,
                        self.now,
                    );
                    if row.disclosure.clip.is_some() {
                        Some(element)
                    } else if let Some(entrance) = entrance {
                        work_settling = entrance.settling;
                        Some(entrance.wrap(
                            element,
                            row.disclosure.gap.unwrap_or(row.model.gap),
                            self.invalidate.clone(),
                        ))
                    } else {
                        Some(element)
                    }
                } else {
                    Some(element)
                }
            }
            None => None,
        };
        if let Some(queue) = &self.admission {
            let position = row
                .model
                .row
                .anchor()
                .and_then(|id| row.model.items.get(id))
                .map_or(u64::MAX, |item| item.position);
            let settling = !queue.borrow().rushed(position)
                && (work_settling
                    || reveal
                        .as_ref()
                        .is_some_and(|(owner, _, _)| owner.is_settling()));
            queue.borrow_mut().finish(key, settling);
        }
        let reveal_done = reveal.as_ref().is_none_or(|(owner, _, _)| {
            owner.fully_visible() && !owner.is_settling() && !owner.is_animating()
        });
        let quiescent = self.content_keyed.get() || row_settles_statically(&row.model, self.now);
        let motionless = {
            let motion = row_owner.motion.borrow();
            let now = self.now;
            !motion.animating() && !motion.sticking(now)
        };
        let disclosure_rest = row.disclosure == RowDisclosure::default();
        let settled = !self.unsettled.get()
            && !work_settling
            && !row.held
            && disclosure_rest
            && quiescent
            && reveal_done
            && motionless;
        editor_core::diagnostics::rebuild_stats::record_event(match row.model.row.semantic() {
            Row::Text { .. } => "rz_kind_text",
            Row::Chain { .. } | Row::WorkEntry { .. } => "rz_kind_chain",
            Row::Work { .. } | Row::WorkHeader(_) => "rz_kind_work",
            Row::Status(_) => "rz_kind_status",
            _ => "rz_kind_other",
        });
        editor_core::diagnostics::rebuild_stats::record_event(match () {
            () if settled => "rz_settled",
            () if self.unsettled.get() => "rz_why_pending",
            () if work_settling => "rz_why_entrance",
            () if row.held => "rz_why_held",
            () if !disclosure_rest => "rz_why_disclosure",
            () if !quiescent => "rz_why_live",
            () if !reveal_done => "rz_why_reveal",
            () => "rz_why_motion",
        });
        if settled && let Some(fingerprint) = self.fingerprint(index, key) {
            self.settled.borrow_mut().insert(key.clone(), fingerprint);
        }
        let element = if let Some(observations) = &self.observations {
            let item = row
                .model
                .row
                .anchor()
                .and_then(|id| row.model.items.get(id));
            let snapshot_live = row
                .model
                .row
                .anchor()
                .and_then(|id| self.reveals.as_ref()?.live.get(id).cloned());
            let snapshot = super::transcript::RowSnapshot {
                row: row.model.row.clone(),
                version: row.model.version,
                item_seq: item.map(|item| item.seq),
                source: snapshot_live
                    .as_ref()
                    .map(|live| self.live_text(&live.text))
                    .or_else(|| {
                        item.and_then(|item| match &item.kind {
                            ItemKind::AssistantMessage {
                                text: TextContent::Inline(source),
                                ..
                            } => Some(self.inline_text(item, source)),
                            _ => None,
                        })
                    }),
                active: self.active.clone(),
                revealed: !row.held && !row.disclosure.hidden,
                reveal_ready: reveal.as_ref().map(|(_, _, ready)| ready.clone()),
                stored: stored_proof.map(|proof| {
                    (
                        Rc::downgrade(
                            &self
                                .stored
                                .as_ref()
                                .expect("stored proof has an owner")
                                .owner,
                        ),
                        proof,
                    )
                }),
                live: snapshot_live,
            };
            let (height, painted) = observations.callbacks(snapshot, self.markdown.clone());
            let height_active = self.active.clone();
            let paint_active = self.active.clone();
            let height = Rc::new(move |bounds| {
                if height_active.get() {
                    height(bounds);
                }
            });
            let painted = Rc::new(move |bounds| {
                if paint_active.get() {
                    painted(bounds);
                }
            });
            super::measured_row_frame(
                &row.model,
                row.disclosure,
                row.held,
                content,
                self.theme,
                height,
                painted,
            )
        } else {
            row_frame(&row.model, row.disclosure, row.held, content, self.theme)
        };
        if let Some(resources) = &self.reveals {
            resources
                .owners
                .borrow_mut()
                .activate(key.clone(), &row_owner);
        }
        self.row_cache
            .borrow_mut()
            .commit(key.clone(), row_owner.clone());
        self.realized_owners
            .borrow_mut()
            .insert(key.clone(), Rc::downgrade(&row_owner));
        let stored_lease = row_owner.stored_leaf.borrow().clone();
        let lease = aqua_ui::element::Canvas::new()
            .style(aqua_ui::style::Style::new().absolute_at(
                Some(0.0),
                Some(0.0),
                Some(0.0),
                Some(0.0),
            ))
            .on_prepaint(move |_, _| {
                let _ = &row_owner;
                let _ = &stored_lease;
            })
            .build()
            .exclude_from_hit_testing();
        let element = aqua_ui::element::Box::new()
            .width(aqua_ui::style::Length::Pct(100.0))
            .child(element)
            .child(lease)
            .build();
        Ok(BatchedCollectionRow { element, batches })
    }
}

#[cfg(test)]
#[path = "../../../tests/conversation/view/factory.rs"]
mod tests;
