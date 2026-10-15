//! Text bodies: what each text row shows. The conversation prepares them
//! (parsing, stored-source pages and leaves, literal chunks) and hands each
//! row its [`Body`]; rows only draw what they are given. When a body's
//! preparation completes, only that body's rows are expanded again.

use crate::Conversation;
use crate::content::SourceField;
use crate::fold::{PlainSection, Row, RowKey, Slot};
use crate::motion::LiveBlockArrivals;
use crate::source_actions::CopySource;
use crate::thread::ItemUi;
use artisan_api::{ContentRef, Item, ItemId, ItemKind, LiveTarget, Seq, Status, TextContent};
use editor_ui::markdown_renderer::{
    LiteralLeaf, LiteralPreparation, MarkdownBodyTone, MarkdownPreparation, MarkdownRenderer,
    PreparedSource, StoredPreparation, StoredSource,
};
use editor_ui::selectable_text::{SelectionLeaf, SelectionSource};
use gpui::{Context, SharedString, px};
use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hash as _, Hasher as _};
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

/// One text body of one item.
pub(crate) type BodyKey = (ItemId, Slot);
/// How many times the reader had scrolled, and the row on top.
pub(crate) type ReaderPlace = (u64, Option<RowKey>);
/// The source a stored body's page moved for, where the reader was then,
/// and whether it moved toward the end: a page change must not undo itself
/// until the reader scrolls on to another row.
pub(crate) type SourceFrontier = (ContentRef, ReaderPlace, bool);

/// Visual rows of one long literal leaf registered with the list at once.
pub(crate) const NATIVE_BODY_WINDOW: usize = 64;

/// The visual rows of one literal leaf too long for one list row.
pub(crate) struct NativeBodyWindow {
    pub reference: ContentRef,
    pub range: Range<usize>,
    pub width: gpui::Pixels,
    pub font_size: gpui::Pixels,
    pub runs: Arc<[gpui::TextRun]>,
    pub count: usize,
    pub window: Range<usize>,
    /// When the window last moved (see [`SourceFrontier`]).
    pub frontier: Option<(ReaderPlace, bool)>,
    pub painted: HashSet<usize>,
    pub error: Option<String>,
}

/// One fetched literal leaf and the selection source copying it.
pub(crate) struct LiteralSelection {
    pub leaf: Arc<LiteralLeaf>,
    pub source: Rc<SelectionSource>,
    pub visible: SharedString,
}

/// An inline literal body split into bounded chunks, one row each.
pub(crate) struct PlainBody {
    pub seq: Seq,
    pub chunks: Vec<SharedString>,
    pub source: Rc<SelectionSource>,
}

/// What one text row shows.
#[derive(Clone)]
pub(crate) enum Body {
    /// Nothing (the item or its text is gone).
    Empty,
    /// Preparation is under way.
    Loading(&'static str),
    /// The Forge that holds the text is gone.
    Disconnected,
    /// Preparation failed: the error and a way to the whole text.
    Failed {
        error: SharedString,
        full: Full,
    },
    /// A long literal leaf's glyph window failed to shape.
    NativeFailed {
        error: String,
        key: SharedString,
        row: Row,
    },
    Markdown(Markdown),
    Plain(Plain),
}

/// The whole text behind a failed body.
#[derive(Clone)]
pub(crate) enum Full {
    /// Copied from what the item holds.
    Copy(SharedString, CopySource),
    /// Read from Forge.
    Stored(ContentRef, SourceField),
}

/// Prepared Markdown blocks.
#[derive(Clone)]
pub(crate) struct Markdown {
    pub prepared: Rc<PreparedSource>,
    pub range: Range<usize>,
    pub selector: SharedString,
    pub order: Vec<u64>,
    pub tone: MarkdownBodyTone,
    /// Inline bodies sit in a full-width block of their own.
    pub wrapped: bool,
    /// Offered when only a preview of the text was prepared.
    pub copy: Option<(SharedString, CopySource)>,
    /// Said under the last block while live text is incomplete.
    pub notice: Option<SharedString>,
}

/// Literal text, selectable as authored.
#[derive(Clone)]
pub(crate) struct Plain {
    pub text: SharedString,
    pub source: Rc<SelectionSource>,
    pub key: SharedString,
    pub order: Vec<u64>,
    pub separator: &'static str,
    /// Stored leaves select within their own bytes.
    pub slice: bool,
    /// A leaf too long for one row shows as a window of visual rows.
    pub native: Option<Native>,
}

/// One visual row of a long literal leaf.
#[derive(Clone)]
pub(crate) struct Native {
    pub key: SharedString,
    pub visual: usize,
    pub window: Range<usize>,
    pub base: Row,
    pub leaf: Arc<LiteralLeaf>,
}

/// A text row's input: its body and where it sits in it.
#[derive(Clone)]
pub(crate) struct TextBody {
    pub body: Body,
    pub first: bool,
    pub last: bool,
    pub stored: bool,
}

impl TextBody {
    fn whole(body: Body) -> Self {
        Self {
            body,
            first: true,
            last: true,
            stored: false,
        }
    }
}

/// Every text body's preparation state. The conversation owns it; rows
/// never see it.
pub(crate) struct Bodies {
    pub markdown: Rc<MarkdownRenderer>,
    /// The selection document of the open thread or agent.
    pub scope: SharedString,
    pub agent_scope: bool,
    /// Immutable inline revisions, by item and body.
    inline: HashMap<BodyKey, (Seq, Arc<str>)>,
    pub stored: HashMap<BodyKey, StoredSource>,
    /// A reply's prepared block count and revision, once known.
    shapes: HashMap<ItemId, (usize, u64)>,
    pub plain: HashMap<BodyKey, PlainBody>,
    pub literal: HashMap<(ItemId, Slot, usize), LiteralSelection>,
    pub native: HashMap<RowKey, NativeBodyWindow>,
    pub frontiers: HashMap<BodyKey, SourceFrontier>,
    /// The page each stored body last asked for; an older request's reply
    /// does not ask again.
    pub pages: HashMap<BodyKey, usize>,
    pub arrivals: HashMap<ItemId, LiveBlockArrivals>,
}

impl Bodies {
    pub(crate) fn new() -> Self {
        Self {
            markdown: Rc::new(
                MarkdownRenderer::new().with_prose_measure(px(crate::row::PROSE_MEASURE)),
            ),
            scope: SharedString::default(),
            agent_scope: false,
            inline: HashMap::new(),
            stored: HashMap::new(),
            shapes: HashMap::new(),
            plain: HashMap::new(),
            literal: HashMap::new(),
            native: HashMap::new(),
            frontiers: HashMap::new(),
            pages: HashMap::new(),
            arrivals: HashMap::new(),
        }
    }

    /// Another transcript: nothing prepared carries over.
    pub(crate) fn clear(&mut self, cx: &mut gpui::App) {
        self.markdown.clear(cx);
        self.inline.clear();
        self.stored.clear();
        self.shapes.clear();
        self.plain.clear();
        self.literal.clear();
        self.native.clear();
        self.frontiers.clear();
        self.pages.clear();
        self.arrivals.clear();
    }

    /// Bodies of items no longer loaded go.
    pub(crate) fn retain(&mut self, loaded: impl Fn(&ItemId) -> bool) {
        self.inline.retain(|(id, _), _| loaded(id));
        self.stored.retain(|(id, _), _| loaded(id));
        self.shapes.retain(|id, _| loaded(id));
        self.plain.retain(|(id, _), _| loaded(id));
        self.arrivals.retain(|id, _| loaded(id));
    }

    pub(crate) fn order(&self, position: u64, slot: Slot) -> Vec<u64> {
        crate::fold::selection_order(self.agent_scope, position, slot)
    }

    /// The prepared block count of a reply, once known.
    pub(crate) fn prose_blocks(&self, id: &ItemId) -> usize {
        self.shapes.get(id).map_or(0, |(blocks, _)| *blocks)
    }

    /// The inline revision of a body, shared until the item changes it.
    fn inline_source(&mut self, item: &Item, slot: Slot, text: &str) -> Arc<str> {
        let cached = self
            .inline
            .entry((item.id.clone(), slot))
            .or_insert_with(|| (item.seq, Arc::from(text)));
        if cached.0 != item.seq && cached.1.as_ref() != text {
            cached.1 = Arc::from(text);
        }
        cached.0 = item.seq;
        cached.1.clone()
    }

    /// Whether `source` is the current, completely prepared inline revision.
    pub(crate) fn inline_prepared(&self, id: &ItemId, slot: Slot, seq: Seq) -> bool {
        let key: SharedString = match slot {
            Slot::Prose => id.to_string().into(),
            slot => format!("{id}:{slot}").into(),
        };
        self.inline
            .get(&(id.clone(), slot))
            .is_some_and(|(at, source)| {
                *at == seq && self.markdown.is_source_prepared(&key, source)
            })
    }

    /// Splits an inline literal body into chunks, once per revision.
    pub(crate) fn prepare_plain(&mut self, item: &Item, section: PlainSection) -> usize {
        let Some(text) = section.inline_text(item) else {
            return 1;
        };
        let key = (item.id.clone(), section.slot());
        if let Some(body) = self.plain.get_mut(&key)
            && (body.seq == item.seq
                || body
                    .chunks
                    .iter()
                    .flat_map(|chunk| chunk.as_bytes().iter().copied())
                    .eq(text.bytes()))
        {
            body.seq = item.seq;
            return body.chunks.len();
        }
        let text: SharedString = text.to_owned().into();
        let chunks: Vec<SharedString> = crate::fold::plain_ranges(&text)
            .into_iter()
            .map(|range| text[range].to_owned().into())
            .collect();
        let mut owner = self.order(item.position, section.slot());
        owner.push(section.order());
        let source = SelectionSource::new(
            self.scope.clone(),
            owner.clone(),
            chunks
                .iter()
                .enumerate()
                .map(|(block, text)| SelectionLeaf {
                    order: {
                        let mut order = owner.clone();
                        order.push(block as u64);
                        order
                    },
                    text: text.clone(),
                    separator: if block == 0 { "\n\n" } else { "" },
                    copies: Rc::from([]),
                })
                .collect(),
        );
        let count = chunks.len();
        self.plain.insert(
            key,
            PlainBody {
                seq: item.seq,
                chunks,
                source,
            },
        );
        count
    }
}

/// The text of a body in `item`.
fn body_text(item: &Item, slot: Slot) -> Option<&TextContent> {
    match slot {
        Slot::Prose => match &item.kind {
            ItemKind::AssistantMessage { text, .. } => Some(text),
            _ => None,
        },
        slot => slot.text(item),
    }
}

impl Conversation {
    /// What the row shows of its body, preparing what it needs. Rows past
    /// the list's overdraw never ask, so nothing off screen prepares.
    pub(crate) fn text_body(&mut self, row: &Row, cx: &mut Context<Self>) -> Option<TextBody> {
        let visual = row.visual();
        match row.semantic() {
            Row::Text {
                id, slot, block, ..
            } => {
                let item = self.view.as_ref()?.shared_item(id)?;
                Some(match slot {
                    Slot::Plain(section) => self.plain_body(&item, *section, *block, visual, cx),
                    slot => TextBody::whole(self.markdown_body(&item, *slot, *block, cx)),
                })
            }
            Row::WorkEntry { id, .. } => {
                let view = self.view.as_ref()?;
                let item = view.shared_item(id)?;
                let open = view.ui.get(id).and_then(|ui| ui.expanded).unwrap_or(false);
                if !open {
                    return None;
                }
                let source: Arc<str> = {
                    let key = (item.id.clone(), Slot::Preview);
                    match self.bodies.inline.get(&key) {
                        Some((seq, source)) if *seq == item.seq => source.clone(),
                        _ => {
                            let source: Arc<str> = crate::preview::markdown(&item)?.into();
                            self.bodies.inline.insert(key, (item.seq, source.clone()));
                            source
                        }
                    }
                };
                Some(TextBody::whole(self.inline_markdown(
                    &item,
                    Slot::Preview,
                    source,
                    0..usize::MAX,
                    MarkdownBodyTone::Foreground,
                    cx,
                )))
            }
            _ => None,
        }
    }

    /// A reply or a formatted section: inline, live or stored.
    fn markdown_body(
        &mut self,
        item: &Item,
        slot: Slot,
        block: usize,
        cx: &mut Context<Self>,
    ) -> Body {
        let Some(text) = body_text(item, slot) else {
            return Body::Empty;
        };
        let tone = if slot == Slot::Text(crate::fold::TextSection::Message) {
            MarkdownBodyTone::Muted
        } else {
            MarkdownBodyTone::Foreground
        };
        if slot != Slot::Prose {
            return match text {
                TextContent::Stored(reference) => {
                    self.stored_markdown(item, slot, reference, block, tone, cx)
                }
                TextContent::Inline(text) => {
                    let source = self.bodies.inline_source(item, slot, text);
                    self.inline_markdown(item, slot, source, block..block + 1, tone, cx)
                }
            };
        }
        let target = LiveTarget::Text {
            item_id: item.id.clone(),
        };
        let live = self
            .view
            .as_ref()
            .and_then(|view| view.live.get(&target))
            .filter(|_| item.status == Status::Running)
            .cloned();
        let source = match (text, &live) {
            (TextContent::Stored(reference), None) => {
                return self.stored_markdown(item, slot, reference, block, tone, cx);
            }
            (TextContent::Inline(text), None) => Some(self.bodies.inline_source(item, slot, text)),
            _ => None,
        };
        let coverage = live.as_ref().and_then(|rope| {
            let snapshot = self
                .host(cx)?
                .read(cx)
                .live_snapshot(&item.thread_id, &target)?;
            (Arc::ptr_eq(&snapshot.text, rope) && !snapshot.coverage.is_complete(rope.len_bytes()))
                .then_some((rope.len_bytes(), snapshot.coverage.total))
        });
        let key: SharedString = item.id.to_string().into();
        let owner = cx.entity().downgrade();
        let id = item.id.clone();
        let ready = move |cx: &mut gpui::App| {
            let _ = owner.update(cx, |this, cx| this.prose_ready(&id, cx));
        };
        let preparation = match (&live, &source) {
            (Some(live), _) => {
                self.bodies
                    .markdown
                    .prepare_rope_source(key.clone(), live.clone(), cx, ready)
            }
            (None, Some(source)) => {
                self.bodies
                    .markdown
                    .prepare_source(key.clone(), source.clone(), cx, ready)
            }
            (None, None) => return Body::Empty,
        };
        let prepared = match preparation {
            MarkdownPreparation::Ready(prepared) => prepared,
            MarkdownPreparation::Pending => match self.bodies.markdown.prepared_source(&key) {
                Some(prepared) => prepared,
                None => return Body::Loading("Loading reply…"),
            },
            MarkdownPreparation::Failed(error) => {
                return Body::Failed {
                    error: error.into(),
                    full: Full::Copy(
                        format!("copy-full-{}", item.id).into(),
                        CopySource::from_parts(live, source),
                    ),
                };
            }
        };
        let range = if self.bodies.shapes.contains_key(&item.id) {
            block..block + 1
        } else {
            0..prepared.block_count()
        };
        let last = range.end >= prepared.block_count();
        Body::Markdown(Markdown {
            copy: (last && prepared.is_preview()).then(|| {
                (
                    format!("copy-full-{}", item.id).into(),
                    CopySource::from_parts(live, source),
                )
            }),
            notice: last.then_some(coverage).flatten().map(|(shown, total)| {
                format!(
                    "Live preview shows {shown} of {total} bytes. Complete content is available after the reply is saved."
                )
                .into()
            }),
            prepared,
            range,
            selector: format!("prose-{}", item.id).into(),
            order: self.bodies.order(item.position, Slot::Prose),
            tone,
            wrapped: true,
        })
    }

    /// Inline Markdown other than a reply, parsed off the main thread.
    fn inline_markdown(
        &mut self,
        item: &Item,
        slot: Slot,
        source: Arc<str>,
        blocks: Range<usize>,
        tone: MarkdownBodyTone,
        cx: &mut Context<Self>,
    ) -> Body {
        let key: SharedString = format!("{}:{slot}", item.id).into();
        let owner = cx.entity().downgrade();
        let id = item.id.clone();
        let preparation =
            self.bodies
                .markdown
                .prepare_source(key.clone(), source.clone(), cx, move |cx| {
                    let _ = owner.update(cx, |this, cx| this.body_ready(&id, slot, cx));
                });
        let prepared = match preparation {
            MarkdownPreparation::Ready(prepared) => prepared,
            MarkdownPreparation::Pending => match self.bodies.markdown.prepared_source(&key) {
                Some(prepared) => prepared,
                None => return Body::Loading("Loading content…"),
            },
            MarkdownPreparation::Failed(error) => {
                return Body::Failed {
                    error: error.into(),
                    full: Full::Copy(format!("copy-full-{key}").into(), CopySource::Text(source)),
                };
            }
        };
        let end = blocks.end.min(prepared.block_count());
        let preview = prepared.is_preview() && end >= prepared.block_count();
        Body::Markdown(Markdown {
            range: blocks.start.min(end)..end,
            copy: preview.then(|| (format!("copy-full-{key}").into(), CopySource::Text(source))),
            notice: None,
            prepared,
            selector: key,
            order: self.bodies.order(item.position, slot),
            tone,
            wrapped: true,
        })
    }

    /// The fetchers a stored body reads its index and leaves with.
    fn stored_index(
        &mut self,
        item: &Item,
        slot: Slot,
        reference: &ContentRef,
        cx: &mut Context<Self>,
    ) -> Result<StoredSource, Box<Body>> {
        let format = if slot.literal() {
            artisan_api::SourceIndexFormat::Literal
        } else {
            artisan_api::SourceIndexFormat::Markdown
        };
        let key = (item.id.clone(), slot);
        if let Some(source) = self
            .bodies
            .stored
            .get(&key)
            .filter(|source| source.source_ref() == reference && source.format() == format)
        {
            return Ok(source.clone());
        }
        let Some(host) = self.host(cx) else {
            return Err(Box::new(Body::Disconnected));
        };
        let client = host.read(cx).client.clone();
        let index_client = client.clone();
        let index: editor_ui::markdown_renderer::MarkdownSourceIndexFetch =
            Arc::new(move |reference, format, offset, limit| {
                let client = index_client.clone();
                Box::pin(async move {
                    client
                        .source_index_range(&reference, format, offset, limit)
                        .await
                        .map_err(|error| error.to_string())
                })
            });
        let content: editor_ui::markdown_renderer::MarkdownContentFetch =
            Arc::new(move |reference, offset, limit| {
                let client = client.clone();
                Box::pin(async move {
                    client
                        .content_range(&reference, offset, limit)
                        .await
                        .map_err(|error| error.to_string())
                })
            });
        let owner = cx.entity().downgrade();
        let id = item.id.clone();
        let preparation = self.bodies.markdown.prepare_stored_source(
            format!("{}:{slot}:index", item.id).into(),
            reference.clone(),
            format,
            content,
            index,
            cx,
            move |cx| {
                let _ = owner.update(cx, |this, cx| this.body_ready(&id, slot, cx));
            },
        );
        match preparation {
            StoredPreparation::Ready(source) => {
                self.bodies.stored.insert(key, source.clone());
                // Its rows split on the index; that waits for the list to
                // finish laying out.
                let owner = cx.entity().downgrade();
                let id = item.id.clone();
                cx.defer(move |cx| {
                    let _ = owner.update(cx, |this, cx| this.respace(&id, slot, cx));
                });
                Ok(source)
            }
            StoredPreparation::Pending => {
                Err(Box::new(Body::Loading("Loading formatted content…")))
            }
            StoredPreparation::Failed(error) => Err(Box::new(Body::Failed {
                error: error.into(),
                full: Full::Stored(reference.clone(), SourceField::of_slot(slot)),
            })),
        }
    }

    /// One indexed Markdown leaf of a stored body.
    fn stored_markdown(
        &mut self,
        item: &Item,
        slot: Slot,
        reference: &ContentRef,
        block: usize,
        tone: MarkdownBodyTone,
        cx: &mut Context<Self>,
    ) -> Body {
        let index = match self.stored_index(item, slot, reference, cx) {
            Ok(index) => index,
            Err(body) => return *body,
        };
        if block >= index.block_count() {
            return Body::Loading("Content layout is updating…");
        }
        let key: SharedString = format!("{}:{slot}:leaf:{block}", item.id).into();
        let owner = cx.entity().downgrade();
        let id = item.id.clone();
        let preparation =
            self.bodies
                .markdown
                .prepare_stored_leaf(key.clone(), &index, block, cx, move |cx| {
                    let _ = owner.update(cx, |this, cx| this.leaf_ready(&id, slot, block, cx));
                });
        let prepared = match preparation {
            MarkdownPreparation::Ready(prepared) => prepared,
            MarkdownPreparation::Pending => return Body::Loading("Loading content…"),
            MarkdownPreparation::Failed(error) => {
                return Body::Failed {
                    error: error.into(),
                    full: Full::Stored(reference.clone(), SourceField::of_slot(slot)),
                };
            }
        };
        let mut order = self.bodies.order(item.position, slot);
        order.push(block as u64);
        Body::Markdown(Markdown {
            range: 0..prepared.block_count(),
            prepared,
            selector: key,
            order,
            tone,
            wrapped: false,
            copy: None,
            notice: None,
        })
    }

    /// A literal body: inline chunks, or stored leaves and their visual
    /// rows.
    fn plain_body(
        &mut self,
        item: &Item,
        section: PlainSection,
        block: usize,
        visual: Option<usize>,
        cx: &mut Context<Self>,
    ) -> TextBody {
        let slot = section.slot();
        let mut order = self.bodies.order(item.position, slot);
        order.extend([section.order(), block as u64]);
        let Some(TextContent::Stored(reference)) = section.text(item) else {
            self.bodies.prepare_plain(item, section);
            let Some(body) = self.bodies.plain.get(&(item.id.clone(), slot)) else {
                return TextBody::whole(Body::Empty);
            };
            let Some(text) = body.chunks.get(block) else {
                return TextBody::whole(Body::Empty);
            };
            return TextBody {
                first: block == 0,
                last: block + 1 == body.chunks.len(),
                stored: false,
                body: Body::Plain(Plain {
                    text: text.clone(),
                    source: body.source.clone(),
                    key: format!("{slot}-{}-{block}", item.id).into(),
                    order,
                    separator: "\n\n",
                    slice: false,
                    native: None,
                }),
            };
        };
        let reference = reference.clone();
        let base = Row::Text {
            id: item.id.clone(),
            slot,
            block,
            run: None,
        };
        let native = self.bodies.native.get(&base.key());
        let key: SharedString = format!("{}:{slot}:leaf:{block}", item.id).into();
        if let Some(state) = native
            && let Some(error) = &state.error
        {
            return TextBody {
                first: true,
                last: true,
                stored: true,
                body: if visual.unwrap_or(0) == state.window.start {
                    Body::NativeFailed {
                        error: error.clone(),
                        key,
                        row: base,
                    }
                } else {
                    Body::Empty
                },
            };
        }
        let count = self
            .bodies
            .stored
            .get(&(item.id.clone(), slot))
            .filter(|source| source.source_ref() == &reference)
            .map_or(1, StoredSource::block_count);
        let visual_last =
            visual.is_none_or(|visual| native.is_some_and(|state| visual + 1 == state.count));
        let window = native.map(|state| state.window.clone());
        let first = block == 0 && visual.unwrap_or(0) == 0;
        let last = block + 1 == count && visual_last;
        let body = self.stored_literal(item, section, &reference, block, visual, window, order, cx);
        TextBody {
            body,
            first,
            last,
            stored: true,
        }
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "one leaf: its item, section, source, ordinal, visual row, window and order"
    )]
    fn stored_literal(
        &mut self,
        item: &Item,
        section: PlainSection,
        reference: &ContentRef,
        block: usize,
        visual: Option<usize>,
        window: Option<Range<usize>>,
        order: Vec<u64>,
        cx: &mut Context<Self>,
    ) -> Body {
        let slot = section.slot();
        let index = match self.stored_index(item, slot, reference, cx) {
            Ok(index) => index,
            Err(body) => return *body,
        };
        if block >= index.block_count() {
            return Body::Loading("Content layout is updating…");
        }
        let key: SharedString = format!("{}:{slot}:leaf:{block}", item.id).into();
        // A leaf this row already holds is never fetched again: the
        // renderer's bounded cache may have let it go while it shows.
        let held = self
            .bodies
            .literal
            .get(&(item.id.clone(), slot, block))
            .filter(|entry| same_leaf(&index, block, &entry.leaf))
            .map(|entry| entry.leaf.clone());
        let owner = cx.entity().downgrade();
        let id = item.id.clone();
        let preparation = match held {
            Some(leaf) => LiteralPreparation::Ready(leaf),
            None => self.bodies.markdown.prepare_stored_literal_leaf(
                key.clone(),
                &index,
                block,
                cx,
                move |cx| {
                    let _ = owner.update(cx, |this, cx| this.leaf_ready(&id, slot, block, cx));
                },
            ),
        };
        let leaf = match preparation {
            LiteralPreparation::Ready(leaf) => leaf,
            LiteralPreparation::Pending => return Body::Loading("Loading content…"),
            LiteralPreparation::Failed(error) => {
                return Body::Failed {
                    error: error.into(),
                    full: Full::Stored(reference.clone(), SourceField::Plain(section)),
                };
            }
        };
        let separator = if block == 0 { "\n\n" } else { "" };
        let scope = self.bodies.scope.clone();
        let entry = self
            .bodies
            .literal
            .entry((item.id.clone(), slot, block))
            .or_insert_with(|| literal_selection(&leaf, scope.clone(), &order, separator));
        if entry.leaf.source_ref != leaf.source_ref || entry.leaf.source_range != leaf.source_range
        {
            *entry = literal_selection(&leaf, scope, &order, separator);
        }
        let visible = entry.visible.clone();
        let native =
            (visible.len() > editor_ui::selectable_text::MAX_NATIVE_SOURCE).then(|| Native {
                key: key.clone(),
                visual: visual.unwrap_or(0),
                window: window.unwrap_or(0..NATIVE_BODY_WINDOW),
                base: Row::Text {
                    id: item.id.clone(),
                    slot,
                    block,
                    run: None,
                },
                leaf: leaf.clone(),
            });
        Body::Plain(Plain {
            text: visible,
            source: entry.source.clone(),
            key: format!("{key}:native:{}", visual.unwrap_or(0)).into(),
            order,
            separator,
            slice: true,
            native,
        })
    }

    /// A body finished preparing: its rows split again if its blocks
    /// changed, and show what is ready.
    pub(crate) fn body_ready(&mut self, id: &ItemId, slot: Slot, cx: &mut Context<Self>) {
        if self.view.as_ref().and_then(|view| view.item(id)).is_none() {
            return;
        }
        self.respace(id, slot, cx);
    }

    /// One leaf of a stored body arrived: only its own rows draw it.
    pub(crate) fn leaf_ready(
        &mut self,
        id: &ItemId,
        slot: Slot,
        block: usize,
        cx: &mut Context<Self>,
    ) {
        let indices = self
            .row_context
            .item_rows
            .get(id)
            .cloned()
            .unwrap_or_default();
        for index in indices {
            let row = self.rows[index].0.clone();
            if !matches!(row.semantic(), Row::Text { slot: row_slot, block: row_block, .. }
                if *row_slot == slot && *row_block == block)
            {
                continue;
            }
            let Some(view) = self.row_views.get(index).and_then(Clone::clone) else {
                continue;
            };
            let body = self.text_body(&row, cx);
            view.update(cx, |view, cx| view.set_body(body, cx));
            self.list.remeasure_items(index..index + 1);
        }
        cx.notify();
    }

    /// A reply's preparation completed: record its arrivals, then its rows.
    pub(crate) fn prose_ready(&mut self, id: &ItemId, cx: &mut Context<Self>) {
        let Some(view) = self.view.as_ref() else {
            return;
        };
        if view.item(id).is_none() {
            return;
        }
        let key: SharedString = id.to_string().into();
        let Some(prepared) = self.bodies.markdown.prepared_source(&key) else {
            return;
        };
        let blocks = prepared.block_count().max(1);
        let current = view.live.get(&LiveTarget::Text {
            item_id: id.clone(),
        });
        let caught_up =
            current.is_none_or(|rope| self.bodies.markdown.is_rope_source_prepared(&key, rope));
        let initially_fresh = view.ui.get(id).is_some_and(ItemUi::fresh);
        let arrivals = &mut self.bodies.arrivals;
        if arrivals.contains_key(id) || arrivals.len() < 128 {
            let arrival = arrivals
                .entry(id.clone())
                .or_insert_with(|| LiveBlockArrivals::baseline(0));
            arrival.prepared(blocks, caught_up, initially_fresh);
            // Rows already mounted keep their own reveal clock; consume
            // their token so evicting and remounting cannot replay it.
            for index in self.row_context.item_rows.get(id).into_iter().flatten() {
                if self.row_views.get(*index).is_some_and(Option::is_some)
                    && let Row::Text {
                        slot: Slot::Prose,
                        block,
                        ..
                    } = self.rows[*index].0.semantic()
                {
                    arrival.claim(*block);
                }
            }
        }
        self.respace(id, Slot::Prose, cx);
    }

    /// Re-expands one body and splices only its rows; every other row
    /// keeps its key, view and measured height.
    pub(crate) fn respace(&mut self, id: &ItemId, slot: Slot, cx: &mut Context<Self>) {
        let is_body = |row: &Row| {
            matches!(row.semantic(), Row::Text { id: row_id, slot: row_slot, .. }
                if row_id == id && *row_slot == slot)
        };
        let Some(start) = self.rows.iter().position(|(row, _)| is_body(row)) else {
            // Not shown (folded away), or shown in a tool row's preview.
            self.refresh_item(id, cx);
            return;
        };
        let end = start
            + self.rows[start..]
                .iter()
                .take_while(|(row, _)| is_body(row))
                .count();
        let base = self.folded.0.iter().find(|(row, _)| is_body(row)).cloned();
        if !self.disclosures.is_empty() || base.is_none() {
            // A disclosure in flight keeps the rows it measured.
            self.reexpand(cx);
            self.refresh_item(id, cx);
            return;
        }
        let (base, version) = base.expect("checked above");
        let rows = self.body_rows(base, version);
        let rows = self.project_native_windows(rows);
        let same = rows.len() == end - start
            && rows
                .iter()
                .zip(&self.rows[start..end])
                .all(|((new, a), (old, b))| new == old && a == b);
        if !same {
            self.splice_rows(start..end, rows, cx);
            self.prune_sources();
        }
        self.refresh_item(id, cx);
    }

    /// Hands every mounted row of `id` its body again.
    pub(crate) fn refresh_item(&mut self, id: &ItemId, cx: &mut Context<Self>) {
        let indices = self
            .row_context
            .item_rows
            .get(id)
            .cloned()
            .unwrap_or_default();
        for index in indices {
            let Some(view) = self.row_views.get(index).and_then(Clone::clone) else {
                continue;
            };
            let row = self.rows[index].0.clone();
            let body = self.text_body(&row, cx);
            view.update(cx, |view, cx| view.set_body(body, cx));
            self.list.remeasure_items(index..index + 1);
        }
        cx.notify();
    }

    /// The rows one text body shows: the blocks prepared or loaded so far,
    /// else the whole body in one row.
    pub(crate) fn body_rows(&mut self, row: Row, version: u64) -> Vec<(Row, u64)> {
        let Row::Text { id, slot, run, .. } = &row else {
            return vec![(row, version)];
        };
        let (id, slot, run) = (id.clone(), *slot, run.clone());
        let Some(item) = self.view.as_ref().and_then(|view| view.shared_item(&id)) else {
            return vec![(row, version)];
        };
        let blocks = |range: Range<usize>, revision: u64| {
            let mut hasher = DefaultHasher::new();
            version.hash(&mut hasher);
            revision.hash(&mut hasher);
            let version = hasher.finish();
            range
                .map(|block| {
                    (
                        Row::Text {
                            id: id.clone(),
                            slot,
                            block,
                            run: run.clone(),
                        },
                        version,
                    )
                })
                .collect::<Vec<_>>()
        };
        let live = slot == Slot::Prose
            && item.status == Status::Running
            && self.view.as_ref().is_some_and(|view| {
                view.live.contains_key(&LiveTarget::Text {
                    item_id: id.clone(),
                })
            });
        match body_text(&item, slot) {
            Some(TextContent::Stored(reference)) if !live => {
                let format = if slot.literal() {
                    artisan_api::SourceIndexFormat::Literal
                } else {
                    artisan_api::SourceIndexFormat::Markdown
                };
                match self
                    .bodies
                    .stored
                    .get(&(id.clone(), slot))
                    .filter(|source| source.source_ref() == reference && source.format() == format)
                {
                    // An empty document still keeps its item's row.
                    Some(source) if !source.loaded_range().is_empty() => {
                        blocks(source.loaded_range(), source.revision())
                    }
                    _ => vec![(row, version)],
                }
            }
            _ if slot == Slot::Prose => {
                if let Some(prepared) = self.bodies.markdown.prepared_source(&id.to_string().into())
                {
                    self.bodies.shapes.insert(
                        id.clone(),
                        (prepared.block_count().max(1), prepared.revision()),
                    );
                }
                match self.bodies.shapes.get(&id).copied() {
                    Some((count, revision)) => blocks(0..count, revision),
                    None => vec![(row, version)],
                }
            }
            _ => match slot {
                Slot::Plain(section) => {
                    let count = self.bodies.prepare_plain(&item, section);
                    blocks(0..count, 0)
                        .into_iter()
                        .map(|(row, _)| (row, version))
                        .collect()
                }
                _ => match self
                    .bodies
                    .markdown
                    .prepared_source(&format!("{id}:{slot}").into())
                {
                    Some(prepared) => blocks(0..prepared.block_count().max(1), prepared.revision()),
                    None => vec![(row, version)],
                },
            },
        }
    }

    /// A literal leaf too long for one list row shows as its window of
    /// visual rows.
    pub(crate) fn project_native_windows(&self, rows: Vec<(Row, u64)>) -> Vec<(Row, u64)> {
        rows.into_iter()
            .flat_map(|(row, version)| {
                let Some(state) = self
                    .bodies
                    .native
                    .get(&row.key())
                    .filter(|state| state.count > 1)
                else {
                    return vec![(row, version)];
                };
                state
                    .window
                    .clone()
                    .map(|visual| {
                        (
                            Row::Projected {
                                row: Box::new(row.clone()),
                                visual,
                            },
                            version,
                        )
                    })
                    .collect()
            })
            .collect()
    }

    /// Source state follows the published rows: literal selections and
    /// page frontiers of sources no longer shown, and native windows of
    /// leaves that changed, go.
    pub(crate) fn prune_sources(&mut self) {
        let Some(view) = self.view.as_ref() else {
            return;
        };
        let bodies = &mut self.bodies;
        let stored = &bodies.stored;
        let current = |id: &ItemId, slot: Slot, reference: &ContentRef| {
            view.item(id).is_some_and(|item| {
                matches!(slot.text(item), Some(TextContent::Stored(current)) if current == reference)
            })
        };
        bodies.literal.retain(|(id, slot, ordinal), entry| {
            stored.get(&(id.clone(), *slot)).is_some_and(|source| {
                source.source_ref() == &entry.leaf.source_ref
                    && source.loaded_range().contains(ordinal)
            }) && current(id, *slot, &entry.leaf.source_ref)
        });
        bodies.frontiers.retain(|key, (reference, _, _)| {
            stored
                .get(key)
                .is_some_and(|source| source.source_ref() == reference)
                && view.item(&key.0).is_some()
        });
        let shown: HashMap<RowKey, (ItemId, Slot, usize)> = self
            .rows
            .iter()
            .filter_map(|(row, _)| match row.semantic() {
                Row::Text {
                    id,
                    slot: slot @ Slot::Plain(_),
                    block,
                    ..
                } => Some((row.semantic().key(), (id.clone(), *slot, *block))),
                _ => None,
            })
            .collect();
        bodies.native.retain(|key, state| {
            let Some((id, slot, block)) = shown.get(key) else {
                return false;
            };
            stored.get(&(id.clone(), *slot)).is_some_and(|source| {
                source.source_ref() == &state.reference
                    && source.leaf_descriptor(*block).is_some_and(|descriptor| {
                        descriptor.source_start == state.range.start as u64
                            && descriptor.source_end == state.range.end as u64
                    })
            })
        });
    }
}

/// Whether `leaf` is ordinal `block` of the source `index` describes.
pub(crate) fn same_leaf(index: &StoredSource, block: usize, leaf: &LiteralLeaf) -> bool {
    index.source_ref() == &leaf.source_ref
        && index.leaf_descriptor(block).is_some_and(|descriptor| {
            descriptor.source_start == leaf.source_range.start as u64
                && descriptor.source_end == leaf.source_range.end as u64
        })
}

fn literal_selection(
    leaf: &Arc<LiteralLeaf>,
    scope: SharedString,
    order: &[u64],
    separator: &'static str,
) -> LiteralSelection {
    LiteralSelection {
        leaf: leaf.clone(),
        visible: leaf
            .text
            .strip_suffix('\n')
            .map_or_else(|| leaf.text.clone(), |text| text.to_owned().into()),
        source: SelectionSource::new(
            scope,
            order.to_vec(),
            vec![SelectionLeaf {
                order: order.to_vec(),
                text: leaf.text.clone(),
                separator,
                copies: Rc::from([]),
            }],
        ),
    }
}
