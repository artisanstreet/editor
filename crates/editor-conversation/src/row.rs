//! One native transcript row and its immutable Item model.
//! Prepared prose and literal bodies publish independent native leaves.
//! A row builds only its own visible content; the native list owns measurement.

mod presenters;
use presenters::{Action, Actions};

use crate::Conversation;
use crate::activity;
use crate::content::{SourceField, SourceOwners, StoredText};
use crate::fold::{PlainSection, Row, RunSummary, Slot, TextSection};
use crate::motion::{
    CompactionPhase, WorkMotion, eased, header_entering, phrase_or_text, tabular_figures,
    with_entrance,
};
use crate::phase::{self, Phase};
use crate::source_actions::{CopySource, copy_action};
use crate::thread::ItemUi;
use artisan_api::{
    BlobHash, Item, ItemId, ItemKind, LiveTarget, RunId, Status, TextContent, Timestamp,
};
use editor_ui::AssetId;
use editor_ui::button::{Button, ButtonVariant};
use editor_ui::chat_bubble::{ChatBubbleSegment, SegmentPosition};
use editor_ui::inline_code_text::inline_runs;
use editor_ui::markdown_renderer::{MarkdownBodyTone, MarkdownRenderer, PreparedReveal};
use editor_ui::motion::{MotionCurve, MotionPolicy};
use editor_ui::selectable_text::{
    SelectableText, SelectionLeaf, SelectionSource, register_selection_source,
};
use editor_ui::shimmer_text::{ShimmerText, ShimmerTiming, ShimmerTone};
use editor_ui::theme::{ArtisanTheme, ProseTypography, RadiusStep, RadiusTokens};
use gpui::{
    AnyElement, App, Context, Div, FontWeight, Hsla, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, RenderOnce, SharedString, StatefulInteractiveElement as _,
    Styled as _, WeakEntity, Window, div, prelude::FluentBuilder as _, px, svg,
};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

/// The turn column, gutter included (`TRANSCRIPT_PROSE_WIDTH_PX`).
pub(crate) const COLUMN: f32 = 768.0;
/// The gutter either side of the column (`TRANSCRIPT_GUTTER_PX`).
pub(crate) const GUTTER: f32 = 24.0;
/// The prose measure inside the column (`TRANSCRIPT_PROSE_BODY_WIDTH_PX`).
pub(crate) const PROSE_MEASURE: f32 = 672.0;
/// One body line inside work sections and chains (`steps(6)`).
const LINE: f32 = 24.0;
/// Shown live output keeps this many trailing bytes.
const LIVE_OUTPUT_TAIL: usize = 8 * 1024;

pub(crate) use editor_core::BlobImage as Blob;

/// What every row shares: renderer, theme, the conversation it sends
/// actions to, and per-body source state keyed by item and [`Slot`]. The
/// conversation owns it and prunes it as items and rows go; rows read it
/// and fill the preparation caches as they first paint a body.
pub(crate) struct Shared {
    pub markdown: Rc<MarkdownRenderer>,
    pub reveal_queue: RefCell<crate::motion::RevealQueue>,
    pub prose_arrivals: RefCell<HashMap<ItemId, crate::motion::LiveBlockArrivals>>,
    pub row_heights: RefCell<HashMap<crate::fold::RowKey, gpui::Pixels>>,
    pub disclosure_clips: RefCell<HashMap<crate::fold::RowKey, crate::disclosure::RowClip>>,
    pub disclosure_hidden: RefCell<std::collections::HashSet<ItemId>>,
    pub disclosure_fractions: RefCell<HashMap<crate::fold::RowKey, f32>>,
    pub theme: ArtisanTheme,
    pub conversation: WeakEntity<Conversation>,
    /// Immutable body revisions shared by every block row of an Item.
    pub sources: RefCell<HashMap<BodyKey, (artisan_api::Seq, Arc<str>)>>,
    pub stored_sources: RefCell<HashMap<BodyKey, editor_ui::markdown_renderer::StoredSource>>,
    pub prose_shapes: RefCell<HashMap<ItemId, (usize, u64)>>,
    pub selection_scope: RefCell<SharedString>,
    pub agent_scope: Cell<bool>,
    pub plain_sources: RefCell<HashMap<BodyKey, PlainBody>>,
    pub native_windows: RefCell<HashMap<crate::fold::RowKey, NativeBodyWindow>>,
    pub source_frontiers: RefCell<HashMap<BodyKey, SourceFrontier>>,
    pub literal_sources: RefCell<HashMap<(ItemId, Slot, usize), LiteralSelection>>,
}

/// One text body of one item.
pub(crate) type BodyKey = (ItemId, Slot);
/// The source a stored body's page moved for, the row at the top of the
/// view then, and whether it moved toward the end: a page change must not
/// undo itself while that row stays on top.
pub(crate) type SourceFrontier = (artisan_api::ContentRef, crate::fold::RowKey, bool);

pub(crate) const NATIVE_BODY_WINDOW: usize = 64;
pub(crate) struct NativeBodyWindow {
    pub reference: artisan_api::ContentRef,
    pub range: std::ops::Range<usize>,
    pub width: gpui::Pixels,
    pub font_size: gpui::Pixels,
    pub runs: Arc<[gpui::TextRun]>,
    pub count: usize,
    pub window: std::ops::Range<usize>,
    pub frontier: Option<(crate::fold::RowKey, bool)>,
    pub painted: std::collections::HashSet<usize>,
    pub error: Option<String>,
}
pub(crate) struct LiteralSelection {
    pub leaf: Arc<editor_ui::markdown_renderer::LiteralLeaf>,
    pub source: Rc<SelectionSource>,
    pub visible: SharedString,
}

pub(crate) struct PlainBody {
    pub seq: artisan_api::Seq,
    pub chunks: Vec<SharedString>,
    pub source: Rc<SelectionSource>,
}
impl Shared {
    pub(crate) fn prepare_plain(&self, item: &Item, section: PlainSection) -> usize {
        let Some(text) = section.inline_text(item) else {
            return 1;
        };
        let mut sources = self.plain_sources.borrow_mut();
        let key = (item.id.clone(), section.slot());
        if let Some(body) = sources.get_mut(&key) {
            if body.seq == item.seq
                || body
                    .chunks
                    .iter()
                    .flat_map(|chunk| chunk.as_bytes().iter().copied())
                    .eq(text.bytes())
            {
                body.seq = item.seq;
                return body.chunks.len();
            }
        }
        {
            let text: SharedString = text.to_owned().into();
            let chunks: Vec<SharedString> = crate::fold::plain_ranges(&text)
                .into_iter()
                .map(|range| text[range].to_owned().into())
                .collect();
            let mut owner =
                crate::fold::selection_order(self.agent_scope.get(), item.position, section.slot());
            owner.push(section.order());
            let source = SelectionSource::new(
                self.selection_scope.borrow().clone(),
                owner,
                chunks
                    .iter()
                    .enumerate()
                    .map(|(block, text)| SelectionLeaf {
                        order: {
                            let mut order = crate::fold::selection_order(
                                self.agent_scope.get(),
                                item.position,
                                section.slot(),
                            );
                            order.extend([section.order(), block as u64]);
                            order
                        },
                        text: text.clone(),
                        separator: if block == 0 { "\n\n" } else { "" },
                        copies: Rc::from([]),
                    })
                    .collect(),
            );
            sources.insert(
                key.clone(),
                PlainBody {
                    seq: item.seq,
                    chunks,
                    source,
                },
            );
        }
        sources[&key].chunks.len()
    }
}

pub(crate) struct RowModel {
    pub row: Row,
    /// The row's items and their subagent output.
    pub items: HashMap<ItemId, Arc<Item>>,
    pub live: HashMap<LiveTarget, Arc<ropey::Rope>>,
    pub ui: HashMap<ItemId, ItemUi>,
    pub run: Option<RunSummary>,
    /// The thread's agents' names by their item.
    pub agents: Arc<HashMap<ItemId, String>>,
    pub images: HashMap<BlobHash, Blob>,
    pub now: Timestamp,
    /// Space above the row: the transcript rhythm (see `fold::gaps`).
    pub gap: f32,
    /// Space below the row: the end space under the last row, else zero.
    pub end: f32,
}

#[derive(IntoElement)]
struct PlainText {
    source: Rc<SelectionSource>,
    text: SharedString,
    key: SharedString,
    scope: SharedString,
    order: Vec<u64>,
    theme: ArtisanTheme,
}
impl RenderOnce for PlainText {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        register_selection_source(&self.source, window, cx);
        SelectableText::retained(self.key, self.text, self.theme, Vec::new())
            .selection_scope(self.scope)
            .selection_order(self.order, "\n\n")
    }
}

/// Lightweight deferred element; parsing and normalization stay on the renderer worker.
#[derive(IntoElement)]
struct PreparedText {
    shared: Rc<Shared>,
    source: Arc<str>,
    key: SharedString,
    order: Vec<u64>,
    block: usize,
    tone: MarkdownBodyTone,
}
impl RenderOnce for PreparedText {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let source = self.source.clone();
        let conversation = self.shared.conversation.clone();
        let preparation =
            self.shared
                .markdown
                .prepare_source(self.key.clone(), self.source, cx, move |cx| {
                    let _ = conversation.update(cx, Conversation::reexpand);
                });
        let prepared = match preparation {
            editor_ui::markdown_renderer::MarkdownPreparation::Ready(prepared) => Some(prepared),
            editor_ui::markdown_renderer::MarkdownPreparation::Pending => {
                self.shared.markdown.prepared_source(&self.key)
            }
            editor_ui::markdown_renderer::MarkdownPreparation::Failed(error) => {
                return div()
                    .child(error)
                    .child(copy_action(
                        format!("copy-full-{}", self.key),
                        self.shared.theme,
                        CopySource::Text(source),
                    ))
                    .into_any_element();
            }
        };
        let Some(prepared) = prepared else {
            return div().child("Loading content…").into_any_element();
        };
        self.shared.markdown.set_selection_prefix(self.order);
        let preview = prepared.is_preview() && self.block + 1 >= prepared.block_count();
        let copy_key = format!("copy-full-{}", self.key);
        let content = self.shared.markdown.render_prepared_range(
            &prepared,
            self.block..(self.block + 1).min(prepared.block_count()),
            self.shared.theme,
            self.key,
            self.tone,
        );
        div()
            .w_full()
            .child(content)
            .when(preview, |root| {
                root.child(copy_action(
                    copy_key,
                    self.shared.theme,
                    CopySource::Text(source),
                ))
            })
            .into_any_element()
    }
}

/// A native semantic row fetches only its indexed immutable source leaf.
#[derive(IntoElement)]
struct StoredPreparedText {
    shared: Rc<Shared>,
    source: artisan_api::ContentRef,
    host: Option<WeakEntity<editor_core::HostData>>,
    owners: SourceOwners,
    id: ItemId,
    slot: Slot,
    order: Vec<u64>,
    block: usize,
    tone: MarkdownBodyTone,
    literal: Option<PlainSection>,
    visual: Option<usize>,
    reveal: Option<Rc<PreparedReveal>>,
    queue_key: Option<crate::fold::RowKey>,
}
impl RenderOnce for StoredPreparedText {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let Some(host) = self.host.clone().and_then(|host| host.upgrade()) else {
            if let Some(reveal) = &self.reveal {
                reveal.mark_presented();
            }
            return div()
                .child("This Forge is no longer connected.")
                .into_any_element();
        };
        let index_client = host.read(cx).client.clone();
        let index_fetcher: editor_ui::markdown_renderer::MarkdownSourceIndexFetch =
            Arc::new(move |reference, format, offset, limit| {
                let client = index_client.clone();
                Box::pin(async move {
                    client
                        .source_index_range(&reference, format, offset, limit)
                        .await
                        .map_err(|error| error.to_string())
                })
            });
        let client = host.read(cx).client.clone();
        let fetcher: editor_ui::markdown_renderer::MarkdownContentFetch =
            Arc::new(move |reference, offset, limit| {
                let client = client.clone();
                Box::pin(async move {
                    client
                        .content_range(&reference, offset, limit)
                        .await
                        .map_err(|error| error.to_string())
                })
            });
        let key: SharedString = format!("{}:{}:index", self.id, self.slot).into();
        let owner = self.shared.conversation.clone();
        // Keep the requested page as the source owner; bootstrap only once.
        // Calling prepare(None) on every row would reset an older page to the tail.
        let format = if self.literal.is_some() {
            artisan_api::SourceIndexFormat::Literal
        } else {
            artisan_api::SourceIndexFormat::Markdown
        };
        let current = self
            .shared
            .stored_sources
            .borrow()
            .get(&(self.id.clone(), self.slot))
            .cloned()
            .filter(|source| source.source_ref() == &self.source && source.format() == format);
        let indexed = current.map_or_else(
            || {
                self.shared.markdown.prepare_stored_source(
                    key,
                    self.source.clone(),
                    format,
                    fetcher,
                    index_fetcher,
                    cx,
                    move |cx| {
                        let _ = owner.update(cx, Conversation::reexpand);
                    },
                )
            },
            editor_ui::markdown_renderer::StoredPreparation::Ready,
        );
        let index = match indexed {
            editor_ui::markdown_renderer::StoredPreparation::Ready(source) => source,
            editor_ui::markdown_renderer::StoredPreparation::Pending => {
                return div().child("Loading formatted content…").into_any_element();
            }
            editor_ui::markdown_renderer::StoredPreparation::Failed(error) => {
                if let Some(reveal) = &self.reveal {
                    reveal.mark_presented();
                }
                return div()
                    .child(error)
                    .child(StoredText {
                        host: Some(host.downgrade()),
                        source: self.source.clone(),
                        theme: self.shared.theme,
                        key: (self.id.clone(), SourceField::of_slot(self.slot)),
                        owners: self.owners.clone(),
                    })
                    .into_any_element();
            }
        };
        let changed = self
            .shared
            .stored_sources
            .borrow_mut()
            .insert((self.id.clone(), self.slot), index.clone())
            .is_none_or(|old| {
                old.source_ref() != index.source_ref() || old.revision() != index.revision()
            });
        if changed {
            let owner = self.shared.conversation.clone();
            cx.defer(move |cx| {
                let _ = owner.update(cx, Conversation::reexpand);
            });
        }
        if self.block >= index.block_count() {
            return div()
                .child("Content layout is updating…")
                .into_any_element();
        }
        let key: SharedString = format!("{}:{}:leaf:{}", self.id, self.slot, self.block).into();
        let owner = self.shared.conversation.clone();
        let id = self.id.clone();
        let slot = self.slot;
        let block = self.block;
        if let Some(section) = self.literal {
            let prepared = self.shared.markdown.prepare_stored_literal_leaf(
                key.clone(),
                &index,
                self.block,
                cx,
                move |cx| {
                    let _ =
                        owner.update(cx, |this, cx| this.source_leaf_ready(&id, slot, block, cx));
                },
            );
            let leaf = match prepared {
                editor_ui::markdown_renderer::LiteralPreparation::Ready(leaf) => leaf,
                editor_ui::markdown_renderer::LiteralPreparation::Pending => {
                    return div().child("Loading content…").into_any_element();
                }
                editor_ui::markdown_renderer::LiteralPreparation::Failed(error) => {
                    return div()
                        .child(error)
                        .child(StoredText {
                            host: Some(host.downgrade()),
                            source: self.source.clone(),
                            theme: self.shared.theme,
                            key: (self.id.clone(), SourceField::Plain(section)),
                            owners: self.owners.clone(),
                        })
                        .into_any_element();
                }
            };
            let mut order = self.order;
            order.extend([section.order(), self.block as u64]);
            let scope = self.shared.selection_scope.borrow().clone();
            let separator = if self.block == 0 { "\n\n" } else { "" };
            let (source, visible) = {
                let mut sources = self.shared.literal_sources.borrow_mut();
                let entry = sources
                    .entry((self.id.clone(), self.slot, self.block))
                    .or_insert_with(|| LiteralSelection {
                        leaf: leaf.clone(),
                        visible: leaf
                            .text
                            .strip_suffix('\n')
                            .map_or_else(|| leaf.text.clone(), |text| text.to_owned().into()),
                        source: SelectionSource::new(
                            scope.clone(),
                            order.clone(),
                            vec![SelectionLeaf {
                                order: order.clone(),
                                text: leaf.text.clone(),
                                separator,
                                copies: Rc::from([]),
                            }],
                        ),
                    });
                if entry.leaf.source_ref != leaf.source_ref
                    || entry.leaf.source_range != leaf.source_range
                {
                    *entry = LiteralSelection {
                        leaf: leaf.clone(),
                        visible: leaf
                            .text
                            .strip_suffix('\n')
                            .map_or_else(|| leaf.text.clone(), |text| text.to_owned().into()),
                        source: SelectionSource::new(
                            scope.clone(),
                            order.clone(),
                            vec![SelectionLeaf {
                                order: order.clone(),
                                text: leaf.text.clone(),
                                separator,
                                copies: Rc::from([]),
                            }],
                        ),
                    };
                }
                (entry.source.clone(), entry.visible.clone())
            };
            register_selection_source(&source, window, cx);
            // The source registry retains the exact raw LF; it separates this
            // native row from the next. Painting that LF as an extra empty
            // physical line would double the transcript's line spacing.
            let visible_range = 0..visible.len();
            let selectable = SelectableText::retained(
                format!("{key}:native:{}", self.visual.unwrap_or(0)),
                visible.clone(),
                self.shared.theme,
                Vec::new(),
            )
            .source_slice(visible_range)
            .selection_scope(scope)
            .selection_order(order, separator);
            if visible.len() <= editor_ui::selectable_text::MAX_NATIVE_SOURCE {
                return selectable.into_any_element();
            }
            let base = Row::PlainBlock {
                id: self.id.clone(),
                section,
                block: self.block,
            };
            let base_key = base.key();
            let window = self
                .shared
                .native_windows
                .borrow()
                .get(&base_key)
                .map_or(0..NATIVE_BODY_WINDOW, |state| state.window.clone());
            let visual = self.visual.unwrap_or(0);
            let notify_owner = self.shared.conversation.clone();
            let layout_owner = self.shared.conversation.clone();
            let paint_owner = self.shared.conversation.clone();
            let failure_owner = self.shared.conversation.clone();
            let layout_base = base.clone();
            let paint_base = base.clone();
            let layout_leaf = leaf.clone();
            let paint_leaf = leaf.clone();
            let failed_leaf = leaf.clone();
            let renderer = self.shared.markdown.clone();
            return selectable
                .native_window(
                    renderer,
                    key,
                    visual,
                    window,
                    Rc::new(move |cx| {
                        let _ = notify_owner.update(cx, |this, cx| {
                            this.source_leaf_ready(&self.id, self.slot, self.block, cx)
                        });
                    }),
                    Rc::new(move |ready, cx| {
                        let _ = layout_owner.update(cx, |this, cx| {
                            this.native_body_ready(&layout_base, &layout_leaf, ready, false, cx)
                        });
                    }),
                    Rc::new(move |ready, cx| {
                        let _ = paint_owner.update(cx, |this, cx| {
                            this.native_body_ready(&paint_base, &paint_leaf, ready, true, cx)
                        });
                    }),
                )
                .native_failure_handler(Rc::new(move |error, cx| {
                    let _ = failure_owner.update(cx, |this, cx| {
                        this.native_body_failed(&base, &failed_leaf, error, cx)
                    });
                }))
                .into_any_element();
        }
        let prepared = self.shared.markdown.prepare_stored_leaf(
            key.clone(),
            &index,
            self.block,
            cx,
            move |cx| {
                let _ = owner.update(cx, |this, cx| this.source_leaf_ready(&id, slot, block, cx));
            },
        );
        let prepared = match prepared {
            editor_ui::markdown_renderer::MarkdownPreparation::Ready(prepared) => prepared,
            editor_ui::markdown_renderer::MarkdownPreparation::Pending => {
                return div().child("Loading content…").into_any_element();
            }
            editor_ui::markdown_renderer::MarkdownPreparation::Failed(error) => {
                if let Some(reveal) = &self.reveal {
                    reveal.mark_presented();
                }
                return div()
                    .child(error)
                    .child(StoredText {
                        host: Some(host.downgrade()),
                        source: self.source.clone(),
                        theme: self.shared.theme,
                        key: (self.id.clone(), SourceField::of_slot(self.slot)),
                        owners: self.owners.clone(),
                    })
                    .into_any_element();
            }
        };
        let mut order = self.order;
        order.push(self.block as u64);
        self.shared.markdown.set_selection_prefix(order);
        if let Some(reveal) = &self.reveal {
            let content = self.shared.markdown.render_prepared_revealing_range(
                &prepared,
                0..prepared.block_count(),
                self.shared.theme,
                key,
                self.tone,
                reveal,
            );
            if reveal.is_animating() {
                window.request_animation_frame();
            }
            if let Some(key) = &self.queue_key {
                self.shared
                    .reveal_queue
                    .borrow_mut()
                    .finish(key, reveal.is_settling());
            }
            content
        } else {
            self.shared.markdown.render_prepared_range(
                &prepared,
                0..prepared.block_count(),
                self.shared.theme,
                key,
                self.tone,
            )
        }
    }
}

pub(crate) struct RowView {
    model: RowModel,
    version: u64,
    shared: Rc<Shared>,
    /// A local paint effect is running.
    pub animating: bool,
    /// Local opacity entrance and the finite compaction separator.
    motion: WorkMotion,
    prose_reveal: Rc<PreparedReveal>,
    queue_held: bool,
    prose_fresh: bool,
    reported_height: Rc<Cell<Option<gpui::Pixels>>>,
    stored: SourceOwners,
    collections: crate::collections::CollectionOwners,
    source_host: Option<WeakEntity<editor_core::HostData>>,
}

impl Drop for RowView {
    fn drop(&mut self) {
        if !self
            .shared
            .disclosure_clips
            .borrow()
            .contains_key(&self.model.row.key())
        {
            self.shared
                .row_heights
                .borrow_mut()
                .remove(&self.model.row.key());
        }
        self.shared
            .reveal_queue
            .borrow_mut()
            .remove(&self.model.row.key());
    }
}

impl RowView {
    pub(crate) fn awaiting_reveal_admission(&self) -> bool {
        match self.model.row.semantic() {
            Row::ProseBlock { .. } => self.prose_fresh && !self.prose_reveal.has_presented(),
            Row::WorkEntry { .. } | Row::Chain { .. } | Row::Prompt(_) | Row::Image(_) => {
                !self.motion.entrance_started()
                    && self
                        .model
                        .row
                        .anchor()
                        .is_some_and(|id| self.ui(id).is_some_and(ItemUi::fresh))
            }
            _ => false,
        }
    }

    fn actions(&self) -> Actions {
        Actions::new(&self.shared)
    }
    fn text(&self) -> presenters::text::TextPresenter<'_> {
        presenters::text::TextPresenter {
            shared: &self.shared,
            owners: &self.stored,
            source_host: self.source_host.clone(),
        }
    }
    fn media(&self) -> presenters::media::MediaPresenter<'_> {
        presenters::media::MediaPresenter {
            theme: self.theme(),
            images: &self.model.images,
            actions: self.actions(),
        }
    }
    fn agent(&self) -> presenters::agent::AgentPresenter {
        presenters::agent::AgentPresenter {
            theme: self.theme(),
            actions: self.actions(),
        }
    }
    fn tools(&self) -> presenters::tool::ToolPresenter<'_> {
        presenters::tool::ToolPresenter {
            text: self.text(),
            collections: &self.collections,
            actions: self.actions(),
        }
    }
    fn requests(&self) -> presenters::request::RequestPresenter<'_> {
        presenters::request::RequestPresenter {
            text: self.text(),
            collections: &self.collections,
            actions: self.actions(),
        }
    }
    fn answer_state(&self, id: &ItemId) -> presenters::request::AnswerState {
        let ui = self.ui(id);
        presenters::request::AnswerState {
            sending: ui.is_some_and(|ui| ui.sending),
            error: ui.and_then(|ui| ui.error.clone()),
        }
    }
    pub(crate) fn new(model: RowModel, version: u64, shared: Rc<Shared>) -> Self {
        let prose_fresh = match model.row.semantic() {
            Row::ProseBlock { id, block } => {
                let block = *block;
                shared.prose_arrivals.borrow_mut().get_mut(id).map_or_else(
                    || model.ui.get(id).is_some_and(ItemUi::fresh),
                    |arrival| arrival.claim(block),
                )
            }
            _ => false,
        };
        Self {
            model,
            version,
            shared,
            animating: false,
            motion: WorkMotion::default(),
            prose_reveal: Rc::new(PreparedReveal::default()),
            queue_held: false,
            prose_fresh,
            reported_height: Rc::default(),
            stored: Rc::new(RefCell::new(HashMap::new())),
            collections: crate::collections::CollectionOwners::default(),
            source_host: None,
        }
    }

    pub(crate) fn disclosure_finished(&mut self, cx: &mut Context<Self>) {
        self.animating =
            self.motion.end_frame() || self.prose_reveal.is_animating() || self.queue_held;
        self.refresh(cx);
    }

    /// What it shows changed outside its model (a source finished
    /// preparing): draw it fresh, and measure it again.
    pub(crate) fn refresh(&mut self, cx: &mut Context<Self>) {
        self.reported_height.set(None);
        cx.notify();
    }

    /// The height to draw this row from cache at: known, and no motion
    /// running.
    pub(crate) fn cached_height(&self) -> Option<gpui::Pixels> {
        self.reported_height.get().filter(|_| !self.animating)
    }

    pub(crate) fn ready_to_read(&self) -> bool {
        !self.queue_held
            && !self.prose_reveal.is_animating()
            && !self
                .model
                .row
                .anchor()
                .is_some_and(|id| self.shared.disclosure_hidden.borrow().contains(id))
    }

    pub(crate) fn version(&self) -> u64 {
        self.version
    }

    /// New content: re-render (and re-measure) this row only.
    pub(crate) fn set(&mut self, model: RowModel, version: u64, cx: &mut Context<Self>) {
        self.stored.borrow_mut().retain(|(id, field), (_, owner)| {
            let source = model.items.get(id).and_then(|item| field.text(item));
            let Some(TextContent::Stored(source)) = source else {
                return false;
            };
            owner.update(cx, |owner, cx| {
                if owner.source(cx) != source {
                    owner.set_source(source.clone(), cx);
                }
            });
            true
        });
        self.collections.reconcile(&model.items, cx);
        self.model = model;
        self.version = version;
        self.refresh(cx);
    }

    pub(crate) fn tick_header(&mut self, now: Timestamp, cx: &mut Context<Self>) {
        if self.model.run.as_ref().is_some_and(RunSummary::live)
            && matches!(
                self.model.row,
                Row::Work { segment: 0, .. } | Row::Status(_)
            )
        {
            self.model.now = now;
            self.refresh(cx);
        }
    }

    pub(crate) fn set_live(
        &mut self,
        target: &LiveTarget,
        text: Option<&Arc<ropey::Rope>>,
        cx: &mut Context<Self>,
    ) {
        match (self.model.live.get(target), text) {
            (Some(old), Some(next)) if Arc::ptr_eq(old, next) => return,
            (None, None) => return,
            _ => {}
        }
        if let Some(text) = text {
            self.model.live.insert(target.clone(), text.clone());
        } else {
            self.model.live.remove(target);
        }
        self.refresh(cx);
    }

    fn item(&self, id: &ItemId) -> Option<&Item> {
        self.model.items.get(id).map(Arc::as_ref)
    }

    fn live_text(&self, target: &LiveTarget) -> Option<String> {
        self.model.live.get(target).map(|text| {
            if matches!(target, LiveTarget::Shell { .. }) {
                let start = text.byte_to_char(text.len_bytes().saturating_sub(LIVE_OUTPUT_TAIL));
                let tail = text.slice(start..).to_string();
                if start > 0 {
                    format!(
                        "Showing the latest output, {} earlier bytes hidden.\n{tail}",
                        text.char_to_byte(start)
                    )
                } else {
                    tail
                }
            } else {
                text.chars().take(2048).collect()
            }
        })
    }

    fn ui(&self, id: &ItemId) -> Option<&ItemUi> {
        self.model.ui.get(id)
    }

    fn theme(&self) -> ArtisanTheme {
        self.shared.theme
    }

    /// Newly observed compact content fades in without changing its measured height.
    fn entering(&mut self, id: &ItemId, element: AnyElement) -> AnyElement {
        if self.queue_held
            || self
                .shared
                .disclosure_clips
                .borrow()
                .contains_key(&self.model.row.key())
        {
            return element;
        }
        let fresh = self.ui(id).is_some_and(ItemUi::fresh);
        let entrance = self.motion.entrance(id.as_str(), fresh);
        with_entrance(element, entrance, px(self.model.gap))
    }

    fn plain_block(
        &self,
        id: &ItemId,
        section: PlainSection,
        block: usize,
        visual: Option<usize>,
    ) -> AnyElement {
        let Some(item) = self.item(id) else {
            return div().into_any_element();
        };
        if let Some(TextContent::Stored(reference)) = section.text(item) {
            let native_key = Row::PlainBlock {
                id: id.clone(),
                section,
                block,
            }
            .key();
            if let Some(state) = self.shared.native_windows.borrow().get(&native_key) {
                if let Some(error) = &state.error {
                    if visual.unwrap_or(0) != state.window.start {
                        return div().into_any_element();
                    }
                    let renderer = self.shared.markdown.clone();
                    let owner = self.shared.conversation.clone();
                    let retry_row = Row::PlainBlock {
                        id: id.clone(),
                        section,
                        block,
                    };
                    let source_key: SharedString =
                        format!("{id}:{}:leaf:{block}", section.slot()).into();
                    return div()
                        .child(error.clone())
                        .child(
                            Button::text(
                                format!("retry-native-{id}-{}-{block}", section.slot()),
                                self.theme(),
                                "Retry",
                            )
                            .on_activate(move |_, _, cx| {
                                renderer.retry_native_window(&source_key);
                                let _ = owner
                                    .update(cx, |this, cx| this.retry_native_body(&retry_row, cx));
                            }),
                        )
                        .into_any_element();
                }
            }
            let count = self
                .shared
                .stored_sources
                .borrow()
                .get(&(id.clone(), section.slot()))
                .filter(|source| source.source_ref() == reference)
                .map_or(1, |source| source.block_count());
            let first = block == 0 && visual.unwrap_or(0) == 0;
            let visual_last = visual.is_none_or(|visual| {
                self.shared
                    .native_windows
                    .borrow()
                    .get(&native_key)
                    .is_some_and(|state| visual + 1 == state.count)
            });
            let last = block + 1 == count && visual_last;
            let content = StoredPreparedText {
                shared: self.shared.clone(),
                source: reference.clone(),
                host: self.source_host.clone(),
                owners: self.stored.clone(),
                id: id.clone(),
                slot: section.slot(),
                block,
                tone: MarkdownBodyTone::Foreground,
                literal: Some(section),
                visual,
                reveal: None,
                queue_key: None,
                order: crate::fold::selection_order(
                    self.shared.agent_scope.get(),
                    item.position,
                    section.slot(),
                ),
            }
            .into_any_element();
            return if section == PlainSection::Error {
                self.error_body(content, first, last)
            } else if section == PlainSection::ImageCaption {
                div()
                    .w_full()
                    .text_size(self.theme().typography.control_text)
                    .text_color(self.theme().colors.muted_foreground.to_paint())
                    .child(content)
                    .into_any_element()
            } else {
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .items_end()
                    .gap(px(8.0))
                    .children(
                        (first && section == PlainSection::User)
                            .then(|| self.media().user_attachments(item))
                            .flatten(),
                    )
                    .child(ChatBubbleSegment::new(
                        self.theme(),
                        match (first, last) {
                            (true, true) => SegmentPosition::Single,
                            (true, false) => SegmentPosition::Head,
                            (false, true) => SegmentPosition::Tail,
                            (false, false) => SegmentPosition::Body,
                        },
                        content,
                    ))
                    .into_any_element()
            };
        }
        self.shared.prepare_plain(item, section);
        let sources = self.shared.plain_sources.borrow();
        let Some(body) = sources.get(&(id.clone(), section.slot())) else {
            return div().into_any_element();
        };
        let Some(text) = body.chunks.get(block) else {
            return div().into_any_element();
        };
        let last = block + 1 == body.chunks.len();
        let content = PlainText {
            source: body.source.clone(),
            text: text.clone(),
            key: format!("{}-{id}-{block}", section.slot()).into(),
            scope: self.shared.selection_scope.borrow().clone(),
            order: {
                let mut order = crate::fold::selection_order(
                    self.shared.agent_scope.get(),
                    item.position,
                    section.slot(),
                );
                order.extend([section.order(), block as u64]);
                order
            },
            theme: self.theme(),
        }
        .into_any_element();
        let theme = self.theme();
        if section == PlainSection::Error {
            return self.error_body(content, block == 0, last);
        }
        if section == PlainSection::ImageCaption {
            return div()
                .w_full()
                .text_size(theme.typography.control_text)
                .text_color(theme.colors.muted_foreground.to_paint())
                .child(content)
                .into_any_element();
        }
        let mut root = div().w_full().flex().flex_col().items_end().gap(px(8.0));
        if block == 0 && section == PlainSection::User {
            root = root.children(self.media().user_attachments(item));
        }
        if section == PlainSection::User && text.is_empty() {
            return root.into_any_element();
        }
        root.child(ChatBubbleSegment::new(
            theme,
            SegmentPosition::for_index(block, body.chunks.len()),
            content,
        ))
        .into_any_element()
    }

    /// One prepared block of a reply, or all of it while its blocks are
    /// not known yet. The renderer owns parsing; the native list owns
    /// visibility.
    fn prose(&mut self, id: &ItemId, block: usize, cx: &mut Context<Self>) -> AnyElement {
        let Some(item) = self.model.items.get(id).cloned() else {
            return div().into_any_element();
        };
        let ItemKind::AssistantMessage { text, .. } = &item.kind else {
            return div().into_any_element();
        };
        self.prose_reveal.begin_frame(
            item.status == Status::Running,
            self.prose_fresh,
            cx.reduce_motion(),
        );
        let key: SharedString = id.to_string().into();
        let owner = self.shared.conversation.clone();
        let prepared_id = id.clone();
        let live = self
            .model
            .live
            .get(&LiveTarget::Text {
                item_id: id.clone(),
            })
            .filter(|_| item.status == Status::Running)
            .cloned();
        let live_coverage = live.as_ref().and_then(|rope| {
            let conversation = self.shared.conversation.upgrade()?;
            let conversation = conversation.read(cx);
            let host = conversation.host(cx)?;
            let snapshot = host.read(cx).live_snapshot(
                &item.thread_id,
                &LiveTarget::Text {
                    item_id: id.clone(),
                },
            )?;
            (Arc::ptr_eq(&snapshot.text, rope) && !snapshot.coverage.is_complete(rope.len_bytes()))
                .then_some((rope.len_bytes(), snapshot.coverage.total))
        });
        if live.is_none() {
            if let TextContent::Stored(_) = text {
                return self
                    .text()
                    .stored_reply(&item, block, self.prose_reveal.clone());
            }
        }
        let source = if live.is_none() {
            let TextContent::Inline(text) = text else {
                unreachable!("stored source handled above")
            };
            let mut sources = self.shared.sources.borrow_mut();
            let cached = sources
                .entry((id.clone(), Slot::Prose))
                .or_insert_with(|| (item.seq, Arc::from(text.as_str())));
            if cached.0 != item.seq && cached.1.as_ref() != text.as_str() {
                cached.1 = Arc::from(text.as_str());
            }
            cached.0 = item.seq;
            Some(cached.1.clone())
        } else {
            None
        };
        let preparation = if let Some(live) = &live {
            self.shared
                .markdown
                .prepare_rope_source(key.clone(), live.clone(), cx, move |cx| {
                    let _ = owner.update(cx, |this, cx| this.prose_ready(&prepared_id, cx));
                })
        } else {
            self.shared.markdown.prepare_source(
                key.clone(),
                source.as_ref().expect("static prose source").clone(),
                cx,
                move |cx| {
                    let _ = owner.update(cx, |this, cx| this.prose_ready(&prepared_id, cx));
                },
            )
        };
        let prepared = match preparation {
            editor_ui::markdown_renderer::MarkdownPreparation::Ready(prepared) => Some(prepared),
            editor_ui::markdown_renderer::MarkdownPreparation::Pending => {
                self.shared.markdown.prepared_source(&key)
            }
            editor_ui::markdown_renderer::MarkdownPreparation::Failed(error) => {
                self.prose_reveal.mark_presented();
                return div()
                    .child(error)
                    .child(copy_action(
                        format!("copy-full-{id}"),
                        self.theme(),
                        CopySource::from_parts(live, source),
                    ))
                    .into_any_element();
            }
        };
        let Some(prepared) = prepared else {
            return div().child("Loading reply…").into_any_element();
        };
        self.shared
            .markdown
            .set_selection_prefix(crate::fold::selection_order(
                self.shared.agent_scope.get(),
                item.position,
                Slot::Prose,
            ));
        let split = self.shared.prose_shapes.borrow().contains_key(id);
        let range = if split {
            block..block + 1
        } else {
            0..prepared.block_count()
        };
        let last = range.end >= prepared.block_count();
        let theme = self.theme();
        let content = self.shared.markdown.render_prepared_revealing_range(
            &prepared,
            range,
            theme,
            format!("prose-{id}"),
            MarkdownBodyTone::Foreground,
            &self.prose_reveal,
        );
        self.animating |= self.prose_reveal.is_animating();
        let mut root = div().w_full().child(content);
        if last {
            if let Some((shown, total)) = live_coverage {
                root = root.child(editor_ui::page::notice(&theme, format!(
                    "Live preview shows {shown} of {total} bytes. Complete content is available after the reply is saved."
                )));
            }
        }
        if last && prepared.is_preview() {
            root = root.child(copy_action(
                format!("copy-full-{id}"),
                theme,
                CopySource::from_parts(live, source),
            ));
        }
        root.into_any_element()
    }

    /// Only the header lives here. Expanded descendants are independent native rows.
    fn work_row(
        &mut self,
        run: Option<&RunId>,
        segment: usize,
        entries: &[ItemId],
        compaction: Option<&ItemId>,
        _cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if segment != 0 {
            return None;
        }
        let summary = self.model.run.as_ref()?;
        let label = self.section_title(summary);
        let open = summary.open;
        let controlled = !summary.live() && run.is_some() && !entries.is_empty();
        let theme = self.theme();
        let muted = theme.colors.muted_foreground.to_paint();
        let key = format!("work-{}", run.map_or_else(String::new, ToString::to_string));
        let words = self.motion.phrase(&format!("{key}-label"), &label);
        let label = phrase_or_text(words, label, px(LINE));
        let local_fraction = self.motion.chevron(&key, open);
        let fraction = self
            .shared
            .disclosure_fractions
            .borrow()
            .get(&self.model.row.key())
            .copied()
            .unwrap_or(local_fraction);
        let checkpoint = compaction
            .and_then(|id| self.compaction_status(id))
            .map(|(_, status)| {
                div().debug_selector(|| "work-compaction".to_owned()).child(
                    div()
                        .debug_selector(|| "compaction".to_owned())
                        .child(status),
                )
            });
        let near = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(theme.spacing.steps(1.0))
            .min_w_0()
            .text_color(muted)
            .child(div().min_w_0().truncate().child(label))
            .when(controlled, |near| {
                near.child(chevron(fraction, 16.0, muted))
            });
        let mut header = div()
            .relative()
            .w_full()
            .min_w_0()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap(theme.spacing.steps(3.0))
            .text_size(px(ProseTypography::BODY_SIZE_PX))
            .line_height(px(LINE))
            .font_weight(ProseTypography::BODY_WEIGHT)
            .tracking(ProseTypography::TRACKING_EM)
            .text_color(muted)
            .font_features(tabular_figures())
            .pb(theme.spacing.steps(2.0))
            .debug_selector(|| "work-header".to_owned())
            .child(near)
            .children(checkpoint)
            .child(
                div()
                    .absolute()
                    .bottom_0()
                    .left_0()
                    .w_full()
                    .h(px(1.0))
                    .bg(theme.colors.border.to_paint()),
            );
        if let Some(progress) = self.motion.header_entrance() {
            header = header_entering(header, progress);
        }
        let shell = div().min_w_0();
        Some(if let Some(run) = run.filter(|_| controlled).cloned() {
            let activate = self
                .actions()
                .handler(Action::ToggleWork { run, open: !open });
            shell
                .id(SharedString::from(key))
                .cursor_pointer()
                .child(header)
                .on_click(move |_, _, cx| activate(cx))
                .into_any_element()
        } else {
            shell.child(header).into_any_element()
        })
    }

    /// A section is always titled with its run's own state, live or
    /// settled; never a generic title.
    fn section_title(&self, summary: &RunSummary) -> String {
        let end = summary.ended_at.unwrap_or(self.model.now);
        let elapsed = summary
            .started_at
            .and_then(|start| u64::try_from(end.saturating_sub(start)).ok())
            .unwrap_or(0);
        phase::section_title(summary.phase, summary.worked, elapsed)
    }

    /// A chain's shared disclosure control. Its activities are native rows below it.
    fn trace_chain(&mut self, ids: &[ItemId]) -> AnyElement {
        let Some(first) = ids.first().cloned() else {
            return div().into_any_element();
        };
        let open = self
            .ui(&first)
            .and_then(|ui| ui.chain_open)
            .unwrap_or(false);
        let categories = activity::chain_categories(ids.iter().filter_map(|id| self.item(id)));
        let clause = activity::chain_clause(categories.iter().copied());
        let icon = activity::chain_icon(categories.iter().copied());
        let theme = self.theme();
        let color = if open {
            theme.colors.foreground.to_paint()
        } else {
            theme.colors.muted_foreground.to_paint()
        };
        let key = format!("chain-{first}");
        let words = self.motion.phrase(&format!("{key}-clause"), &clause);
        let clause = phrase_or_text(words, clause, px(LINE));
        let local_fraction = self.motion.chevron(&key, open);
        let fraction = self
            .shared
            .disclosure_fractions
            .borrow()
            .get(&self.model.row.key())
            .copied()
            .unwrap_or(local_fraction);
        let activate = self.actions().handler(Action::ToggleChain {
            id: first,
            open: !open,
        });
        div()
            .id(SharedString::from(key))
            .flex()
            .flex_row()
            .items_center()
            .gap(theme.spacing.steps(1.0))
            .w_full()
            .min_w_0()
            .py(theme.spacing.steps(0.5))
            .cursor_pointer()
            .debug_selector(|| "work-chain".to_owned())
            .on_click(move |_, _, cx| activate(cx))
            .child(
                svg()
                    .path(icon.as_str())
                    .size(px(16.0))
                    .flex_shrink_0()
                    .text_color(color),
            )
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_size(px(ProseTypography::BODY_SIZE_PX))
                    .font_weight(ProseTypography::BODY_WEIGHT)
                    .tracking(ProseTypography::TRACKING_EM)
                    .line_height(px(LINE))
                    .text_color(color)
                    .font_features(tabular_figures())
                    .child(clause),
            )
            .child(chevron(fraction, 14.0, color))
            .into_any_element()
    }

    /// A non-activity row inside the work: plan, notice, nested messages.
    fn entry(&self, item: &Item, _depth: usize) -> AnyElement {
        match &item.kind {
            ItemKind::AssistantMessage { .. } => self.text().assistant(item, 0),
            ItemKind::Tool { .. } => {
                let live = self.live_text(&LiveTarget::Shell {
                    item_id: item.id.clone(),
                });
                let expanded = self
                    .ui(&item.id)
                    .and_then(|ui| ui.expanded)
                    .unwrap_or(false);
                self.tools()
                    .activity_row(item, expanded, live.as_deref(), &self.model.agents)
            }
            ItemKind::Agent { .. } => self.agent().agent_row(item),
            ItemKind::UserMessage { .. } => self.plain_block(&item.id, PlainSection::User, 0, None),
            ItemKind::Plan { .. } => self.tools().plan_entry(item),
            ItemKind::Notice { .. } => self.text().notice(item),
            ItemKind::Question { .. } => self.requests().question_card(item),
            ItemKind::Error { .. } => self.error_alert(item),
            ItemKind::Compaction { .. } | ItemKind::Image { .. } => div().into_any_element(),
        }
    }

    /// The compaction separator (`render_compaction_separator`): a
    /// full-width rule with a centred label, "Compacting" under the shimmer
    /// while the engine compacts. Appearing live, it opens, draws its rules
    /// outward and lifts its label in; settling, the word swaps and one
    /// highlight runs from the label to both ends of the rule.
    fn compaction_status(
        &mut self,
        id: &ItemId,
    ) -> Option<(crate::motion::CompactionFrame, AnyElement)> {
        let theme = self.theme();
        let item = self.item(id)?;
        let phase = match item.status {
            Status::Running => CompactionPhase::Running,
            Status::Done => CompactionPhase::Done,
            _ => CompactionPhase::Stopped,
        };
        let fresh = self.ui(id).is_some_and(ItemUi::fresh);
        let frame = self.motion.compaction(id.as_str(), phase, fresh);
        // The slot keeps the widest word it can show in flow, invisibly; the
        // visible words sit centred over it, so the rules keep their length.
        let widest = match phase {
            CompactionPhase::Stopped => CompactionPhase::Stopped.label(),
            CompactionPhase::Running | CompactionPhase::Done => CompactionPhase::Running.label(),
        };
        let word = |phase: CompactionPhase| {
            div()
                .absolute()
                .top(px(0.0))
                .left(px(0.0))
                .right(px(0.0))
                .flex()
                .justify_center()
                .child(
                    ShimmerText::new("compaction-status", phase.label(), theme)
                        .active(phase == CompactionPhase::Running)
                        .timing(ShimmerTiming::Progress)
                        .tone(ShimmerTone::Muted),
                )
        };
        let mut slot = div()
            .relative()
            .flex_shrink_0()
            .child(div().opacity(0.0).child(widest));
        slot = match frame.swap {
            Some((from, swap)) if swap < 0.5 => {
                let out = eased(MotionCurve::EaseInOut, swap * 2.0);
                slot.child(
                    word(from)
                        .opacity(1.0 - out)
                        .top(px(-4.0 * out))
                        .filter(vec![gpui::Filter::Blur(px(2.0 * out))]),
                )
            }
            Some((_, swap)) => {
                let enter = eased(MotionCurve::EaseInOut, swap * 2.0 - 1.0);
                slot.child(
                    word(phase)
                        .opacity(enter)
                        .top(px(4.0 * (1.0 - enter)))
                        .filter(vec![gpui::Filter::Blur(px(2.0 * (1.0 - enter)))]),
                )
            }
            None if frame.label < 1.0 => slot.child(
                word(phase)
                    .opacity(frame.label)
                    .top(px(4.0 * (1.0 - frame.label)))
                    .filter(vec![gpui::Filter::Blur(px(3.0 * (1.0 - frame.label)))]),
            ),
            None => slot.child(word(phase)),
        };
        Some((frame, slot.into_any_element()))
    }

    fn compaction_row(&mut self, id: &ItemId) -> AnyElement {
        const PULSE_LENGTH_PX: f32 = 56.0;
        const PULSE_PEAK_ALPHA: f32 = 0.55;
        let theme = self.theme();
        let Some((frame, slot)) = self.compaction_status(id) else {
            return div().into_any_element();
        };
        let muted = theme.colors.muted_foreground.to_paint();
        let rule_color = theme.colors.border.to_paint();
        // Each rule grows from the label outward; the highlight runs the
        // same way and fades as it goes.
        let rule = |outward_left: bool| {
            let mut track = div()
                .flex_1()
                .min_w_0()
                .h(px(1.0))
                .relative()
                .flex()
                .when(outward_left, gpui::Styled::justify_end)
                .child(div().h_full().w(gpui::relative(frame.draw)).bg(rule_color));
            if let Some(pulse) = frame.pulse {
                let travel = eased(MotionCurve::SmoothOut, pulse);
                let alpha = PULSE_PEAK_ALPHA * (std::f32::consts::PI * pulse).sin();
                let highlight = div()
                    .absolute()
                    .top(px(0.0))
                    .h_full()
                    .w(px(PULSE_LENGTH_PX))
                    .bg(fade(theme.colors.foreground.to_paint(), alpha));
                // A lane one highlight shorter than the rule: it starts at
                // the label and ends flush with the end.
                let lane = div().absolute().top(px(0.0)).h_full();
                track = track.child(if outward_left {
                    lane.left(px(PULSE_LENGTH_PX))
                        .right(px(0.0))
                        .child(highlight.right(gpui::relative(travel)))
                } else {
                    lane.left(px(0.0))
                        .right(px(PULSE_LENGTH_PX))
                        .child(highlight.left(gpui::relative(travel)))
                });
            }
            track
        };
        div()
            .w_full()
            .min_w_0()
            .h(px(LINE * frame.open))
            .overflow_hidden()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(12.0))
            .text_size(px(ProseTypography::BODY_SIZE_PX))
            .line_height(px(LINE))
            .font_weight(ProseTypography::BODY_WEIGHT)
            .tracking(ProseTypography::TRACKING_EM)
            .text_color(muted)
            .debug_selector(|| "compaction".to_owned())
            .child(rule(true))
            .child(slot)
            .child(rule(false))
            .into_any_element()
    }

    /// Error body styling is shared across its bounded native leaves.
    fn error_alert(&self, item: &Item) -> AnyElement {
        self.plain_block(&item.id, PlainSection::Error, 0, None)
    }
    fn error_body(&self, content: AnyElement, first: bool, last: bool) -> AnyElement {
        let theme = self.theme();
        let mut root = div()
            .w_full()
            .min_w_0()
            .px(theme.spacing.steps(3.5))
            .pt(px(if first { 12.0 } else { 0.0 }))
            .pb(px(if last { 12.0 } else { 0.0 }))
            .border_x_1()
            .border_color(theme.colors.destructive.with_alpha(0.25).to_paint())
            .bg(theme.colors.destructive.with_alpha(0.05).to_paint())
            .text_color(theme.colors.muted_foreground.to_paint())
            .text_size(theme.typography.control_text)
            .when(first, |root| {
                root.border_t_1()
                    .rounded_tl(RadiusTokens::value(RadiusStep::Xl))
                    .rounded_tr(RadiusTokens::value(RadiusStep::Xl))
            })
            .when(last, |root| {
                root.border_b_1()
                    .rounded_bl(RadiusTokens::value(RadiusStep::Xl))
                    .rounded_br(RadiusTokens::value(RadiusStep::Xl))
            });
        if first {
            root = root.child(
                div()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.colors.destructive.to_paint())
                    .child("Error"),
            );
        }
        root.child(content).into_any_element()
    }

    /// Adapters supply canonical status and reasoning copy. The row shows a bounded snippet.
    fn status_row(&self, run: &RunId) -> Option<AnyElement> {
        let theme = self.theme();
        let muted = theme.colors.muted_foreground.to_paint();
        let summary = self.model.run.as_ref();
        let phase = summary.map_or(Phase::Thinking, |summary| summary.phase);
        let reduced = self
            .live_text(&LiveTarget::Reasoning {
                run_id: run.clone(),
            })
            .filter(|_| phase == Phase::Thinking)
            .and_then(|reasoning| summary_copy(&reasoning));
        let live = summary.is_none_or(RunSummary::live);
        let content: AnyElement = if let Some(copy) = reduced {
            let runs = inline_runs(&copy, theme);
            ShimmerText::new("run-summary", runs.text, theme)
                .text_runs(runs.highlights, runs.overrides)
                .active(live)
                .timing(ShimmerTiming::Progress)
                .tone(ShimmerTone::Muted)
                .truncate()
                .into_any_element()
        } else {
            let copy = self
                .live_text(&LiveTarget::Status {
                    run_id: run.clone(),
                })
                .filter(|text| !text.trim().is_empty())
                .or_else(|| phase::status_copy(phase).map(str::to_owned))?;
            ShimmerText::new("run-status", copy, theme)
                .active(live)
                .tone(ShimmerTone::Muted)
                .truncate()
                .into_any_element()
        };
        Some(
            div()
                .w_full()
                .min_w_0()
                .flex()
                .flex_row()
                .items_center()
                .text_size(px(ProseTypography::BODY_SIZE_PX))
                .line_height(px(ProseTypography::BODY_LINE_PX))
                .font_weight(ProseTypography::BODY_WEIGHT)
                .tracking(ProseTypography::TRACKING_EM)
                .text_color(muted)
                .child(content)
                .into_any_element(),
        )
    }
}

impl Render for RowView {
    #[expect(
        clippy::too_many_lines,
        reason = "the row's one render dispatches its kind, runs its motion frame and lays out the column"
    )]
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.animating = false;
        self.source_host = self
            .shared
            .conversation
            .upgrade()
            .and_then(|conversation| conversation.read(cx).host(cx))
            .map(|host| host.downgrade());
        let policy = if cx.reduce_motion() {
            MotionPolicy::Reduced
        } else {
            MotionPolicy::Full
        };
        let live = self.model.run.as_ref().is_some_and(RunSummary::live);
        self.motion.begin_frame(live, policy);
        let row = self.model.row.clone();
        let key = row.key();
        let bypass = matches!(
            row.semantic(),
            Row::Work { .. }
                | Row::PlainBlock {
                    section: PlainSection::User,
                    ..
                }
        );
        let position = row
            .anchor()
            .and_then(|id| self.item(id))
            .map_or(u64::MAX, |item| item.position);
        let (part, ordinal) = match row.semantic() {
            Row::Chain { .. } => (0, 0),
            Row::ProseBlock { block, .. } | Row::TextBlock { block, .. } => (2, *block),
            Row::PlanStep { step, .. } => (2, *step),
            _ => (1, 0),
        };
        self.queue_held = self.shared.reveal_queue.borrow_mut().begin(
            key.clone(),
            (position, part, ordinal),
            !bypass && policy == MotionPolicy::Full,
        );
        self.prose_reveal.set_paused(self.queue_held);
        let clip = self.shared.disclosure_clips.borrow().get(&key).copied();
        let deferred_by_group = clip.is_none()
            && !self.shared.disclosure_fractions.borrow().contains_key(&key)
            && row
                .anchor()
                .is_some_and(|id| self.shared.disclosure_hidden.borrow().contains(id));
        let content: Option<AnyElement> = if self.queue_held || deferred_by_group {
            Some(div().into_any_element())
        } else {
            match &row {
                Row::ProseBlock { id, block } => Some(self.prose(id, *block, cx)),
                Row::Work {
                    run,
                    segment,
                    entries,
                    compaction,
                } => self.work_row(run.as_ref(), *segment, entries, compaction.as_ref(), cx),
                Row::Projected { row, visual } => match row.as_ref() {
                    Row::PlainBlock { id, section, block } => {
                        Some(self.plain_block(id, *section, *block, Some(*visual)))
                    }
                    _ => None,
                },
                Row::PlainBlock { id, section, block } => {
                    Some(self.plain_block(id, *section, *block, None))
                }
                Row::WorkEntry { id, .. } => {
                    let content = self
                        .item(id)
                        .map_or_else(|| div().into_any_element(), |item| self.entry(item, 0));
                    Some(self.entering(id, content))
                }
                Row::Chain { ids, .. } => {
                    let content = self.trace_chain(ids);
                    Some(if let Some(id) = ids.first() {
                        self.entering(id, content)
                    } else {
                        content
                    })
                }
                Row::PlanStep { id, step, .. } => Some(self.item(id).map_or_else(
                    || div().into_any_element(),
                    |item| self.tools().plan_step(item, *step),
                )),
                Row::TextBlock {
                    id, section, block, ..
                } => Some(self.item(id).map_or_else(
                    || div().into_any_element(),
                    |item| self.text().section(item, *section, *block),
                )),
                Row::Prompt(id) => {
                    let card = match self.item(id) {
                        Some(item) if matches!(item.kind, ItemKind::Question { .. }) => {
                            self.requests().question_card(item)
                        }
                        Some(item) => self
                            .requests()
                            .approval_card(item, self.answer_state(&item.id))
                            .unwrap_or_else(|| self.entry(item, 0)),
                        None => div().into_any_element(),
                    };
                    Some(self.entering(id, card))
                }
                Row::Compaction(id) => Some(self.compaction_row(id)),
                Row::Image(id) => {
                    let image = self.item(id).map_or_else(
                        || div().into_any_element(),
                        |item| self.media().image_row(item),
                    );
                    Some(self.entering(id, image))
                }
                Row::Gap => Some(
                    div()
                        .w_full()
                        .flex()
                        .justify_center()
                        .child(self.actions().button(
                            self.theme(),
                            "load-newer",
                            "Load newer messages",
                            ButtonVariant::Secondary,
                            false,
                            Action::LoadNewer,
                        ))
                        .into_any_element(),
                ),
                Row::Status(run) => self.status_row(run),
                Row::AgentResult(id) => Some(self.item(id).map_or_else(
                    || div().into_any_element(),
                    |item| self.text().section(item, TextSection::AgentResult, 0),
                )),
                Row::AgentStatus(_, working) => self.agent().agent_status_row(*working),
            }
        };
        let theme = self.theme();
        // The status line keeps its own 8 px below (`mb(steps(2))`).
        let below = if matches!(row, Row::Status(_)) {
            theme.spacing.steps(2.0)
        } else {
            px(0.0)
        };
        let column = div()
            .w_full()
            .max_w(px(COLUMN))
            .mx_auto()
            .px(px(GUTTER))
            .min_w_0()
            .text_color(theme.colors.foreground.to_paint());
        self.animating |= self.motion.end_frame() || clip.is_some();
        if self.animating {
            window.request_animation_frame();
        }
        self.shared.reveal_queue.borrow_mut().finish(
            &key,
            !bypass
                && (self.queue_held
                    || self.motion.is_settling()
                    || self.prose_reveal.is_settling()),
        );
        self.animating |= self.queue_held;
        if self.queue_held {
            window.request_animation_frame();
        }
        let content = content.map(|content| {
            let content = if self.queue_held {
                div().opacity(0.0).child(content).into_any_element()
            } else {
                content
            };
            let chained = match &row {
                Row::WorkEntry { id, .. } => self
                    .item(id)
                    .is_some_and(|item| activity::category(item).is_some()),
                Row::TextBlock {
                    section: TextSection::Message,
                    ..
                } => true,
                _ => false,
            };
            if chained {
                div()
                    .relative()
                    .w_full()
                    .min_w_0()
                    .pl(theme.spacing.steps(6.0))
                    .child(
                        div()
                            .absolute()
                            .top(px(-self.model.gap))
                            .bottom_0()
                            .left(theme.spacing.steps(1.75))
                            .w(px(2.0))
                            .bg(theme.colors.border.with_alpha(0.6).to_paint()),
                    )
                    .child(content)
                    .into_any_element()
            } else {
                content
            }
        });
        let column = match content {
            Some(content) => column
                .pt(px(if self.queue_held || deferred_by_group {
                    0.0
                } else {
                    self.model.gap
                }))
                .pb(below)
                .child(content),
            // Nothing to show: the row keeps only the end space.
            None => column,
        };
        let measured = self.reported_height.clone();
        let shared = self.shared.clone();
        let body_key = key.clone();
        let owner = self.shared.conversation.clone();
        let end = px(self.model.end);
        let mut body = div()
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .child(column.flex_shrink_0())
            .on_children_prepainted(move |bounds, window, _| {
                if let Some(bounds) = bounds.first() {
                    let changed = shared
                        .row_heights
                        .borrow_mut()
                        .insert(body_key.clone(), bounds.size.height)
                        .is_none_or(|old| old != bounds.size.height);
                    if changed && shared.disclosure_clips.borrow().contains_key(&body_key) {
                        window.request_animation_frame();
                    }
                }
            });
        if let Some(clip) = clip {
            body = body.h(clip.height).overflow_hidden().opacity(clip.opacity);
        } else if deferred_by_group || self.queue_held {
            body = body.h(px(0.0)).overflow_hidden();
        }
        div()
            .relative()
            .w_full()
            .pb(end)
            .child(body)
            .on_children_prepainted(move |bounds, _, cx| {
                if let Some(bounds) = bounds.first() {
                    let height = bounds.size.height + end;
                    if measured
                        .replace(Some(height))
                        .is_some_and(|previous| previous != height)
                    {
                        let owner = owner.clone();
                        let key = key.clone();
                        cx.defer(move |cx| {
                            let _ = owner.update(cx, |this, cx| this.row_height_changed(&key, cx));
                        });
                    }
                }
            })
    }
}

/// Adapter reasoning is already the canonical status copy.
pub(crate) fn summary_copy(summary: &str) -> Option<String> {
    let summary = summary.trim();
    (!summary.is_empty()).then(|| summary.to_owned())
}

/// The catalog chevron, rotated with its disclosure's open fraction.
fn chevron(fraction: f32, size: f32, color: Hsla) -> gpui::Svg {
    let angle = fraction * std::f32::consts::FRAC_PI_2;
    svg()
        .path(AssetId::TABLER_CHEVRON_RIGHT.as_str())
        .size(px(size))
        .flex_shrink_0()
        .text_color(color)
        .with_transformation(gpui::Transformation::rotate(gpui::radians(angle)))
}

/// Card body text: 14 px on a 24 px line.
fn body_text(text: &str, theme: &ArtisanTheme) -> Div {
    div()
        .w_full()
        .min_w_0()
        .text_size(theme.typography.editor_text_desktop)
        .line_height(theme.spacing.steps(6.0))
        .whitespace_normal()
        .child(text.to_owned())
}

/// The work section's body line: base size, 24 px line, prose weight and
/// tracking.
fn body_line(element: Div, theme: &ArtisanTheme) -> Div {
    element
        .min_w_0()
        .text_size(px(ProseTypography::BODY_SIZE_PX))
        .line_height(theme.spacing.steps(6.0))
        .font_weight(ProseTypography::BODY_WEIGHT)
        .tracking(ProseTypography::TRACKING_EM)
}

/// Icons in tinted text take that colour, slightly muted.
fn icon(asset: AssetId, color: Hsla) -> gpui::Svg {
    svg()
        .path(asset.as_str())
        .size(px(16.0))
        .flex_shrink_0()
        .text_color(fade(color, 0.8))
}

/// `color` with its alpha scaled.
fn fade(mut color: Hsla, amount: f32) -> Hsla {
    color.alpha *= amount;
    color
}
