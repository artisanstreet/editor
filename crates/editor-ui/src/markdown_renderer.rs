//! Native Markdown presentation with bounded asynchronous preparation.
//!
//! The worker normalizes syntax, inline text, rich specs, and code presentation.
//! The transcript constructs only visible semantic rows; nested lists and
//! tables flatten into that same row stream. Ordinary code uses a bounded
//! native line viewport, shared CodeCard chrome, and source-owned selection.
//! Shared media components load through the scoped Forge provider.

use crate::theme::{ArtisanTheme, Oklch, ProseTypography, TextStep, TypographyTokens};
use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    AnyElement, AppContext as _, Div, FontStyle, FontWeight, HighlightStyle, IntoElement,
    ParentElement, Pixels, SharedString, Styled, div, prelude::InteractiveElement as _, px,
};

use crate::markdown::{Block, CodeToken, CodeTokenKind, Span};
use crate::selectable_text::SelectableText;

mod async_preparation;
mod budget;
pub mod native_projection;
mod projection_owner;
mod projection_worker;
pub use projection_owner::NativePreparation;
pub use projection_worker::NativeWindow;
mod literal;
mod stored;
pub use literal::{LiteralLeaf, LiteralPreparation};
pub use stored::{
    ContentFetch as MarkdownContentFetch, SourceIndexFetch as MarkdownSourceIndexFetch,
    StoredPreparation, StoredSource,
};
mod code_copy;
pub use async_preparation::{MarkdownPreparation, PreparedSource};
mod diagram;
mod inline;
mod prepared;
mod reveal;
pub use reveal::PreparedReveal;
mod rich;
mod table;

pub use code_copy::CodeCopyButton;
use inline::flatten_inline;
pub(crate) use prepared::PreparedMarkdown;
use prepared::{
    LeafPresentation, PreparedBlock, PreparedCode, PreparedKind, PreparedLeaf, ROOT_SLOT, Slot,
};
pub use rich::{
    CopyEntry, CopyKind, MediaFetch, MediaFetcher, MediaRequest, OpenReference, ResourceScope,
    ShowCopyMenu, decode_image_bytes, image_resident_bytes,
};

/// Renderer-owned bounded preparation, resource, and selection state.
#[derive(Debug)]
pub struct MarkdownRenderer {
    prose_measure: Option<Pixels>,
    media: RefCell<Option<MediaFetcher>>,
    selection_prefix: RefCell<Vec<u64>>,
    selection_scope: RefCell<SharedString>,
    selection_sources: RefCell<Vec<SelectionEntry>>,
    preparation: async_preparation::PreparationOwner,
    stored: stored::StoredOwner,
    literal: literal::LiteralOwner,
    projection: projection_owner::ProjectionOwner,
    pub(crate) native_runs:
        RefCell<std::collections::VecDeque<crate::selectable_text::NativeRunEntry>>,
}

struct SelectionEntry {
    scope: SharedString,
    prefix: Vec<u64>,
    prepared: std::rc::Weak<PreparedMarkdown>,
    source: Rc<crate::selectable_text::SelectionSource>,
    bytes: usize,
}
impl std::fmt::Debug for SelectionEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SelectionEntry")
            .field("scope", &self.scope)
            .field("prefix", &self.prefix)
            .finish_non_exhaustive()
    }
}

impl MarkdownRenderer {
    /// Constructs lazy, renderer-owned preparation and resource state.
    #[must_use]
    pub fn new() -> Self {
        Self {
            prose_measure: None,
            media: RefCell::new(None),
            selection_prefix: RefCell::new(Vec::new()),
            selection_scope: RefCell::new({
                static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
                SharedString::from(format!(
                    "markdown-{}",
                    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                ))
            }),
            selection_sources: RefCell::new(Vec::new()),
            preparation: async_preparation::PreparationOwner::default(),
            stored: stored::StoredOwner::default(),
            literal: literal::LiteralOwner::default(),
            projection: projection_owner::ProjectionOwner::default(),
            native_runs: RefCell::default(),
        }
    }

    /// Changes this renderer's immutable host/thread resource provider.
    pub fn set_media_fetcher(&self, fetcher: Option<MediaFetcher>) {
        *self.media.borrow_mut() = fetcher;
    }

    /// Releases resources for Items no longer resident in the transcript window.
    pub fn retain_resource_owners(&self, owners: &[Vec<u64>], cx: &mut gpui::App) {
        if let Some(media) = self.media.borrow().as_ref() {
            media.retain_resources(owners, cx);
        }
    }

    /// Semantic transcript Item order; prepared block ordinals follow it.
    pub fn set_selection_prefix(&self, prefix: Vec<u64>) {
        *self.selection_prefix.borrow_mut() = prefix;
    }

    /// Selection document identity. Conversation renderers share a host/thread scope;
    /// standalone previews retain their default isolated identity.
    pub fn set_selection_scope(&self, scope: impl Into<SharedString>) {
        *self.selection_scope.borrow_mut() = scope.into();
    }

    /// Parses/highlights one Item revision on the renderer's bounded worker.
    pub fn prepare_source(
        &self,
        key: SharedString,
        source: Arc<str>,
        cx: &mut gpui::App,
        notify: impl Fn(&mut gpui::App) + 'static,
    ) -> MarkdownPreparation {
        self.preparation.prepare(
            key,
            async_preparation::Source::Text(source),
            None,
            cx,
            notify,
        )
    }
    /// Like [`Self::prepare_source`] for a user's message: `command`, the
    /// special `/command` it starts with, paints as the composer paints it.
    pub fn prepare_message(
        &self,
        key: SharedString,
        source: Arc<str>,
        command: Option<Range<usize>>,
        cx: &mut gpui::App,
        notify: impl Fn(&mut gpui::App) + 'static,
    ) -> MarkdownPreparation {
        let command = command
            .and_then(|range| source.get(range))
            .map(str::to_owned);
        self.preparation.prepare(
            key,
            async_preparation::Source::Text(source),
            command,
            cx,
            notify,
        )
    }
    /// Publishes only admitted native hot glyph windows. The caller supplies
    /// its actual resolved layout width and stable immutable TextRuns identity.
    #[expect(
        clippy::too_many_arguments,
        reason = "one preparation request: its key, source, typography, width, runs, window and the callback"
    )]
    pub fn prepare_native_window(
        &self,
        key: SharedString,
        source: SharedString,
        font_size: Pixels,
        width: Pixels,
        runs: Arc<[gpui::TextRun]>,
        range: Range<usize>,
        cx: &mut gpui::App,
        notify: impl Fn(&mut gpui::App) + 'static,
    ) -> NativePreparation {
        let worker = match self.preparation.worker() {
            Ok(worker) => worker,
            Err(error) => return NativePreparation::Failed(error),
        };
        self.projection.prepare(
            key, source, font_size, width, runs, range, &worker, cx, notify,
        )
    }
    pub fn native_row_count(
        &self,
        key: &SharedString,
        source: &SharedString,
        font_size: Pixels,
        width: Pixels,
        runs: &Arc<[gpui::TextRun]>,
    ) -> Option<usize> {
        self.projection
            .row_count(key, source, font_size, width, runs)
    }
    pub fn retry_native_window(&self, key: &SharedString) {
        self.projection.retry(key);
    }
    #[expect(
        clippy::too_many_arguments,
        reason = "one preparation request: its key, reference, format, both fetchers and the callback"
    )]
    pub fn prepare_stored_source(
        &self,
        key: SharedString,
        reference: artisan_api::ContentRef,
        format: artisan_api::SourceIndexFormat,
        fetcher: MarkdownContentFetch,
        index_fetcher: MarkdownSourceIndexFetch,
        cx: &mut gpui::App,
        notify: impl Fn(&mut gpui::App) + 'static,
    ) -> StoredPreparation {
        self.stored.prepare(
            key,
            reference,
            format,
            fetcher,
            index_fetcher,
            None,
            cx,
            notify,
        )
    }
    pub fn prepare_stored_leaf(
        &self,
        key: SharedString,
        source: &StoredSource,
        leaf: usize,
        cx: &mut gpui::App,
        notify: impl Fn(&mut gpui::App) + 'static,
    ) -> MarkdownPreparation {
        if source.format() != artisan_api::SourceIndexFormat::Markdown {
            return MarkdownPreparation::Failed(
                "Literal sources require the literal projection renderer".into(),
            );
        }
        let index = &source.index;
        let Some(descriptor) = source.leaf_descriptor(leaf) else {
            return MarkdownPreparation::Pending;
        };
        self.preparation.prepare(
            key,
            async_preparation::Source::Remote {
                reference: source.reference.clone(),
                metadata: index.header.metadata.clone(),
                descriptor,
                fetcher: Arc::clone(&source.fetcher),
            },
            None,
            cx,
            notify,
        )
    }
    /// Fetches only one indexed raw literal leaf after bounded admission.
    pub fn prepare_stored_literal_leaf(
        &self,
        key: SharedString,
        source: &StoredSource,
        ordinal: usize,
        cx: &mut gpui::App,
        notify: impl Fn(&mut gpui::App) + 'static,
    ) -> LiteralPreparation {
        self.literal.prepare(key, source, ordinal, cx, notify)
    }
    /// Proves raw bytes belong to this exact source descriptor, not glyph paint.
    pub fn is_stored_literal_leaf_prepared(
        &self,
        key: &SharedString,
        source: &StoredSource,
        ordinal: usize,
    ) -> bool {
        self.literal.is_prepared(key, source, ordinal)
    }
    pub fn request_stored_window(
        &self,
        key: SharedString,
        source: &StoredSource,
        offset: u64,
        cx: &mut gpui::App,
        notify: impl Fn(&mut gpui::App) + 'static,
    ) -> StoredPreparation {
        self.stored.prepare(
            key,
            source.reference.clone(),
            source.format,
            Arc::clone(&source.fetcher),
            Arc::clone(&source.index_fetcher),
            Some(offset),
            cx,
            notify,
        )
    }
    pub fn is_stored_leaf_prepared(
        &self,
        key: &SharedString,
        source: &StoredSource,
        leaf: usize,
    ) -> bool {
        self.preparation
            .is_remote_prepared(key, &source.reference, leaf)
    }
    /// Receives a cheap live snapshot; bounded text flattening happens on the worker.
    pub fn prepare_rope_source(
        &self,
        key: SharedString,
        source: Arc<ropey::Rope>,
        cx: &mut gpui::App,
        notify: impl Fn(&mut gpui::App) + 'static,
    ) -> MarkdownPreparation {
        self.preparation.prepare(
            key,
            async_preparation::Source::Rope(source),
            None,
            cx,
            notify,
        )
    }
    pub fn is_rope_source_prepared(&self, key: &SharedString, source: &Arc<ropey::Rope>) -> bool {
        self.preparation.is_rope_prepared(key, source)
    }
    /// Most recent prepared revision, useful while its successor is pending.
    pub fn prepared_source(&self, key: &SharedString) -> Option<Rc<PreparedSource>> {
        self.preparation.prepared(key)
    }
    /// True only for the current completed revision, including source identity.
    pub fn is_source_prepared(&self, key: &SharedString, source: &Arc<str>) -> bool {
        self.preparation.is_prepared(key, source)
    }
    /// Cancels old Item jobs and releases all prepared/cache resources when
    /// the owning thread changes or closes.
    pub fn clear(&self, cx: &mut gpui::App) {
        self.stored.clear();
        self.literal.clear();
        self.projection.clear();
        self.native_runs.borrow_mut().clear();
        self.preparation.clear(cx);
        if let Some(media) = self.media.borrow_mut().take() {
            media.clear_resources(cx);
        }
        self.selection_prefix.borrow_mut().clear();
        self.selection_sources.borrow_mut().clear();
    }
    /// Draws only the requested semantic leaf-row range. The caller's native
    /// list owns row visibility and measurement; block identity remains its
    /// source ordinal across different windows.
    pub fn render_prepared_range(
        &self,
        prepared: &PreparedSource,
        range: Range<usize>,
        theme: ArtisanTheme,
        selector: impl Into<SharedString>,
        tone: MarkdownBodyTone,
    ) -> AnyElement {
        self.render_prepared(
            &prepared.presentation,
            prepared.revision(),
            &theme,
            &selector.into(),
            tone,
            range,
            None,
            true,
            None,
        )
    }

    /// Renders a user's message with catalog-backed inline mentions.
    pub fn render_message_range(
        &self,
        prepared: &PreparedSource,
        range: Range<usize>,
        theme: ArtisanTheme,
        selector: impl Into<SharedString>,
        tone: MarkdownBodyTone,
        mentions: &crate::text_editor::Tokenizer,
    ) -> AnyElement {
        self.render_prepared(
            &prepared.presentation,
            prepared.revision(),
            &theme,
            &selector.into(),
            tone,
            range,
            None,
            true,
            Some(mentions),
        )
    }

    /// Like [`Self::render_prepared_range`], without copy actions on code:
    /// a tool call's preview, which nobody copies from.
    pub fn render_preview_range(
        &self,
        prepared: &PreparedSource,
        range: Range<usize>,
        theme: ArtisanTheme,
        selector: impl Into<SharedString>,
        tone: MarkdownBodyTone,
    ) -> AnyElement {
        self.render_prepared(
            &prepared.presentation,
            prepared.revision(),
            &theme,
            &selector.into(),
            tone,
            range,
            None,
            false,
            None,
        )
    }

    /// Paints paced word fades over complete prepared native leaves. The range
    /// retains its original layout and selection ownership during the reveal.
    pub fn render_prepared_revealing_range(
        &self,
        prepared: &PreparedSource,
        range: Range<usize>,
        theme: ArtisanTheme,
        selector: impl Into<SharedString>,
        tone: MarkdownBodyTone,
        reveal: &PreparedReveal,
    ) -> AnyElement {
        self.render_prepared(
            &prepared.presentation,
            prepared.revision(),
            &theme,
            &selector.into(),
            tone,
            range,
            Some(reveal),
            true,
            None,
        )
    }

    fn selection_source(
        &self,
        prepared: &Rc<PreparedMarkdown>,
        scope: &SharedString,
        prefix: &[u64],
    ) -> Rc<crate::selectable_text::SelectionSource> {
        const SELECTION_SOURCE_BYTES: usize = 32 * 1024 * 1024;
        let mut sources = self.selection_sources.borrow_mut();
        sources.retain(|entry| entry.prepared.strong_count() > 0);
        let index = sources
            .iter()
            .position(|entry| &entry.scope == scope && entry.prefix == prefix);
        if let Some(index) = index {
            let entry = sources.remove(index);
            if entry.prepared.ptr_eq(&Rc::downgrade(prepared)) {
                let source = Rc::clone(&entry.source);
                sources.push(entry);
                return source;
            }
        }
        let source = prepared.selection_source(scope, prefix);
        let bytes = source
            .resident_bytes()
            .saturating_add(std::mem::size_of::<SelectionEntry>())
            .saturating_add(std::mem::size_of_val(prefix))
            .saturating_add(scope.len());
        while !sources.is_empty()
            && (sources.len() >= 48
                || sources
                    .iter()
                    .map(|entry| entry.bytes)
                    .sum::<usize>()
                    .saturating_add(bytes)
                    > SELECTION_SOURCE_BYTES)
        {
            sources.remove(0);
        }
        // The current render owns its source even when it cannot fit in the
        // retained registry. Never cache an oversized document indefinitely.
        if bytes > SELECTION_SOURCE_BYTES {
            return source;
        }
        sources.push(SelectionEntry {
            scope: scope.clone(),
            prefix: prefix.to_vec(),
            prepared: Rc::downgrade(prepared),
            source: Rc::clone(&source),
            bytes,
        });
        source
    }

    /// Bounds reading blocks to `measure` while code fences keep the full
    /// width of the container.
    ///
    /// Prose reads best on a fixed measure, but code wraps or clips where
    /// prose would reflow, so a fence takes every pixel its column offers.
    /// The caller therefore hands the renderer its whole column and names
    /// the prose measure here, instead of bounding the container itself.
    /// Without a measure every block fills the container.
    #[must_use]
    pub const fn with_prose_measure(mut self, measure: Pixels) -> Self {
        self.prose_measure = Some(measure);
        self
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "one draw of a prepared range with its theme, selector, tone and optional reveal"
    )]
    fn render_prepared(
        &self,
        prepared: &Rc<PreparedMarkdown>,
        revision: u64,
        theme: &ArtisanTheme,
        selector: &SharedString,
        tone: MarkdownBodyTone,
        range: Range<usize>,
        reveal: Option<&PreparedReveal>,
        copy: bool,
        mentions: Option<&crate::text_editor::Tokenizer>,
    ) -> AnyElement {
        let selection_prefix = self.selection_prefix.borrow();
        let media = self
            .media
            .borrow()
            .clone()
            .map(|media| media.owned_by(&selection_prefix, revision));
        let selection_scope = self.selection_scope.borrow();
        let selection_source = self.selection_source(prepared, &selection_scope, &selection_prefix);
        let render = RenderContext {
            theme,
            media: media.as_ref(),
            selection_prefix: &selection_prefix,
            selection_scope: &selection_scope,
            tone,
            selector,
            reveal,
            copy,
            mentions,
        };
        let mut root = markdown_root(render.id(ROOT_SLOT), theme, tone);
        let flow = prepared.flow();
        if let Some(reveal) = reveal {
            reveal.set_tail(
                flow.last()
                    .filter(|_| range.end >= flow.len())
                    .and_then(tail_slot),
            );
        }
        for row in flow
            .iter()
            .skip(range.start)
            .take(range.end.saturating_sub(range.start))
        {
            let (content, measure) = match &row.kind {
                prepared::FlowKind::Block(block) => (
                    render_block(block, &render),
                    if matches!(block.kind, PreparedKind::Code(_)) {
                        None
                    } else {
                        self.prose_measure
                    },
                ),
                prepared::FlowKind::TableRow { block, row } => {
                    let PreparedKind::Table(table) = &block.kind else {
                        unreachable!("prepared table row")
                    };
                    (table::render_row(table, *row, &render), self.prose_measure)
                }
                prepared::FlowKind::Empty { leaf, slot } => {
                    (render_leaf(leaf, *slot, "\n", &render), self.prose_measure)
                }
            };
            let mut frame = div()
                .relative()
                .w_full()
                .min_w_0()
                .pl(px(
                    crate::f32_of_count(row.indent) * ProseTypography::LIST_INDENT_PX
                ))
                .child(content);
            if let (Some(reveal), prepared::FlowKind::Block(block)) = (reveal, &row.kind)
                && matches!(
                    block.kind,
                    PreparedKind::Code(_) | PreparedKind::Html { .. }
                )
                && let Some((alpha, rise)) = reveal.block_entrance(block.selector)
            {
                frame = reveal.block_panel(block.selector, frame.opacity(alpha).top(px(rise)));
            }
            for (indent, marker, ordered) in &row.markers {
                let mut label = div()
                    .absolute()
                    .left(px(
                        crate::f32_of_count(*indent) * ProseTypography::LIST_INDENT_PX
                    ))
                    .top_0()
                    .text_color(theme.colors.text_muted.to_paint());
                if *ordered {
                    label = label.font_weight(FontWeight::NORMAL);
                }
                frame = frame.child(label.child(marker.clone()));
            }
            root = root.child(with_block_margins(
                frame.into_any_element(),
                row.gap,
                measure,
            ));
        }
        let content = root.into_any_element();
        let content = if let Some(reveal) = reveal {
            reveal.mask(content)
        } else {
            content
        };
        SelectionRegistration {
            source: selection_source,
            content,
        }
        .into_any_element()
    }
}

#[derive(gpui::IntoElement)]
struct SelectionRegistration {
    source: Rc<crate::selectable_text::SelectionSource>,
    content: AnyElement,
}
impl gpui::RenderOnce for SelectionRegistration {
    fn render(self, window: &mut gpui::Window, cx: &mut gpui::App) -> impl IntoElement {
        crate::selectable_text::register_selection_source(&self.source, window, cx);
        self.content
    }
}

/// Everything one render pass threads through the block tree.
struct RenderContext<'a> {
    mentions: Option<&'a crate::text_editor::Tokenizer>,
    selection_prefix: &'a [u64],
    selection_scope: &'a SharedString,
    media: Option<&'a MediaFetcher>,
    theme: &'a ArtisanTheme,
    tone: MarkdownBodyTone,
    /// Namespace for stable prepared ordinals.
    selector: &'a SharedString,
    reveal: Option<&'a PreparedReveal>,
    /// Code carries a copy action.
    copy: bool,
}

impl RenderContext<'_> {
    fn id(&self, slot: Slot) -> SharedString {
        SharedString::from(format!("{}-md-{slot}", self.selector))
    }

    fn element_id(&self, slot: Slot) -> gpui::ElementId {
        gpui::ElementId::NamedInteger(self.selector.clone(), slot as u64)
    }

    fn selection_order(&self, slot: Slot) -> Vec<u64> {
        let mut order = self.selection_prefix.to_vec();
        order.push(slot as u64);
        order
    }
}

impl Default for MarkdownRenderer {
    fn default() -> Self {
        Self::new()
    }
}

/// A byte offset or count from the wire as an index; saturates where
/// `usize` is narrower than 64 bits, so a bounds check rejects it.
fn wire_index(value: u64) -> usize {
    usize::try_from(value).unwrap_or(usize::MAX)
}

fn markdown_root(selector: SharedString, theme: &ArtisanTheme, tone: MarkdownBodyTone) -> Div {
    // No container gap: inter-block spacing lives in per-block margins so
    // collapsing behavior matches the reference.
    let mut root = body_container(theme, tone).flex().flex_col();
    root = root.debug_selector(move || selector.to_string());
    root
}

/// Which body text tone a rendered message carries.
///
/// The reference prose body reads muted; the assistant reply is explicitly
/// promoted to the foreground token per product direction, so the tone
/// rides the render entry instead of living in any shared default.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum MarkdownBodyTone {
    /// Reference muted prose body.
    Muted,
    /// Assistant reply body in the foreground token.
    Foreground,
    /// A preview in a card: foreground mono at code size, like a terminal.
    Code,
}

/// Returns the theme body color for one markdown body tone.
///
/// Span-level recipes (strong, code, links, headings) carry their own exact
/// tokens and never consult this; only untagged body text inherits it.
#[must_use]
pub const fn markdown_body_text_color(tone: MarkdownBodyTone, theme: ArtisanTheme) -> Oklch {
    match tone {
        MarkdownBodyTone::Muted => theme.colors.text_muted,
        MarkdownBodyTone::Foreground | MarkdownBodyTone::Code => theme.colors.text,
    }
}

fn body_container(theme: &ArtisanTheme, tone: MarkdownBodyTone) -> Div {
    let body = div()
        .w_full()
        .min_w_0()
        .text_color(markdown_body_text_color(tone, *theme).to_paint())
        .whitespace_normal();
    if tone == MarkdownBodyTone::Code {
        return body
            .font_family(theme.typography.mono.family)
            .text_size(px(ProseTypography::CODE_SIZE_PX))
            .line_height(px(ProseTypography::CODE_LINE_PX))
            .font_weight(FontWeight::NORMAL)
            .tracking(0.0);
    }
    body.text_size(px(ProseTypography::BODY_SIZE_PX))
        .line_height(px(ProseTypography::BODY_LINE_PX))
        .font_weight(ProseTypography::BODY_WEIGHT)
        .tracking(theme.typography.tracking_em)
}

/// Which margin recipe a block uses: root blocks take plugin margins,
/// while paragraphs and nested lists inside list items take the
/// item-scope recipe (`> ul > li p`, `ul ul`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockScope {
    /// Top-level transcript blocks.
    Root,
    /// Blocks inside a list item.
    Item,
}

/// Reference top/bottom margins for one block in px.
const fn block_margins(block: &Block, scope: BlockScope) -> (f32, f32) {
    match block {
        Block::Heading { level, .. } => {
            let heading = ProseTypography::heading(*level);
            (heading.margin_top_px, heading.margin_bottom_px)
        }
        Block::Paragraph { .. } => match scope {
            BlockScope::Root => (
                ProseTypography::PARAGRAPH_MARGIN_PX,
                ProseTypography::PARAGRAPH_MARGIN_PX,
            ),
            BlockScope::Item => (
                ProseTypography::ITEM_PARAGRAPH_MARGIN_PX,
                ProseTypography::ITEM_PARAGRAPH_MARGIN_PX,
            ),
        },
        Block::Code(_) => (
            ProseTypography::FENCE_MARGIN_PX,
            ProseTypography::FENCE_MARGIN_PX,
        ),
        // Raw HTML arrives as inert text, so it reads as a paragraph.
        Block::Html { .. } => (
            ProseTypography::PARAGRAPH_MARGIN_PX,
            ProseTypography::PARAGRAPH_MARGIN_PX,
        ),
        Block::List { .. } => match scope {
            BlockScope::Root => (
                ProseTypography::LIST_MARGIN_PX,
                ProseTypography::LIST_MARGIN_PX,
            ),
            BlockScope::Item => (
                ProseTypography::NESTED_LIST_MARGIN_PX,
                ProseTypography::NESTED_LIST_MARGIN_PX,
            ),
        },
        Block::Table(_) => (
            ProseTypography::TABLE_MARGIN_PX,
            ProseTypography::TABLE_MARGIN_PX,
        ),
    }
}

/// Wraps one rendered block with its collapsed top gap; bottom stays zero
/// because every gap renders exactly once (see [`block_gaps`]).
fn with_block_margins(child: AnyElement, top_px: f32, measure: Option<Pixels>) -> AnyElement {
    let mut block = div().mt(px(top_px));
    if let Some(measure) = measure {
        block = block.max_w(measure);
    }
    block.child(child).into_any_element()
}

/// Collapsed top gaps for one block sequence in source order.
///
/// CSS margins collapse: the space between two blocks is the larger of the
/// two facing margins, never their sum — and a flex column does not
/// collapse at all, so emitting both a bottom margin and a collapsed top
/// would double every gap (two paragraphs would read 20 + 20 = 40 instead
/// of 20). Each gap here is therefore rendered exactly once, as top margin
/// with zero bottom: the first block takes 0, and every other gap is the
/// max of the facing margins. Followers of h2–h4 keep the heading's own
/// bottom (`h2/h3/h4 + *` zeroes only the follower's top margin, so the
/// collapse still reads the full 24 px after an h2).
#[must_use]
pub fn block_gaps(blocks: &[Block], scope: BlockScope) -> Vec<f32> {
    let mut gaps = Vec::with_capacity(blocks.len());
    let mut previous_bottom: f32 = 0.0;
    let mut previous_zeroes_follower = false;
    for (index, block) in blocks.iter().enumerate() {
        let (margin_top, margin_bottom) = block_margins(block, scope);
        let gap = if index == 0 {
            0.0
        } else if previous_zeroes_follower {
            previous_bottom
        } else {
            previous_bottom.max(margin_top)
        };
        gaps.push(gap);
        previous_bottom = margin_bottom;
        previous_zeroes_follower = matches!(block, Block::Heading { level: 2..=4, .. });
    }
    gaps
}

/// Render the immutable prepared block using the current presentation context.
/// Parsing and inline-span preparation are owned by the source revision worker.
fn render_block(prepared: &PreparedBlock, render: &RenderContext<'_>) -> AnyElement {
    let theme = render.theme;
    let selector = render.id(prepared.selector);
    let mut element = body_container(theme, render.tone).flex().flex_col();

    match &prepared.kind {
        PreparedKind::Heading { level, leaf } => {
            let heading = ProseTypography::heading(*level);
            element = element
                .font_family(theme.typography.heading.family)
                .font_weight(ProseTypography::HEADING_WEIGHT)
                .text_size(px(heading.size_px))
                .line_height(px(heading.line_px))
                .letter_spacing(px(theme.typography.heading_tracking_px(&heading)))
                .text_color(theme.colors.text.to_paint())
                .child(render_leaf(leaf, prepared.selector, "\n\n", render));
        }
        PreparedKind::Paragraph { leaf } => {
            element = element.child(render_leaf(leaf, prepared.selector, "\n\n", render));
        }
        PreparedKind::Code(code) => {
            element = element.child(render_code(code, render));
        }
        PreparedKind::Html { id, source } => {
            element = element.child(
                SelectableText::retained(
                    render.element_id(*id),
                    source.clone(),
                    *theme,
                    Vec::new(),
                )
                .selection_order(render.selection_order(*id), "\n")
                .selection_scope(render.selection_scope.clone()),
            );
        }
        PreparedKind::List(list) => {
            let _ = list;
            unreachable!("list containers are flattened into prepared flow rows");
        }
        PreparedKind::Table(_) => {
            unreachable!("table containers are flattened into prepared flow rows")
        }
    }
    element = element.debug_selector(move || selector.to_string());
    element.into_any_element()
}

fn item_marker(ordered: bool, base: u64, position: usize, task: Option<bool>) -> String {
    if let Some(checked) = task {
        if checked {
            return String::from("☑");
        }
        return String::from("☐");
    }
    if ordered {
        format!("{}.", base.saturating_add(position as u64))
    } else {
        String::from("•")
    }
}

/// The slot of the last leaf `row` draws, where a streamed source ends.
fn tail_slot(row: &prepared::FlowRow) -> Option<Slot> {
    match &row.kind {
        prepared::FlowKind::Block(block) => match &block.kind {
            PreparedKind::Heading { .. } | PreparedKind::Paragraph { .. } => Some(block.selector),
            PreparedKind::Code(_)
            | PreparedKind::Html { .. }
            | PreparedKind::List(_)
            | PreparedKind::Table(_) => None,
        },
        prepared::FlowKind::TableRow { block, row } => {
            let PreparedKind::Table(table) = &block.kind else {
                return None;
            };
            table
                .rows
                .get(*row)?
                .iter()
                .rev()
                .find(|cell| cell.leaf.is_some())
                .map(|cell| cell.selector)
        }
        prepared::FlowKind::Empty { slot, .. } => Some(*slot),
    }
}

/// Renders one prepared inline leaf under `selector`.
fn render_leaf(
    leaf: &PreparedLeaf,
    slot: Slot,
    separator: &'static str,
    render: &RenderContext<'_>,
) -> AnyElement {
    let presentation = leaf.presentation();
    selectable_leaf(&presentation, slot, separator, render)
}

/// Renders one flattened leaf as retained selectable text.
///
/// The selection element owns link clicks and suppresses drag activation,
/// so no separate click handler lives beside it: one element, one behavior.
/// Text, resolved runs, overrides, and link data are shared with the cached
/// presentation, so nothing here copies the leaf's content. Only a leaf
/// with shared highlights avoids rebuilding style ranges.
#[expect(
    clippy::significant_drop_tightening,
    reason = "the flagged bindings are references or plain values, not guards"
)]
fn selectable_leaf(
    presentation: &LeafPresentation,
    slot: Slot,
    separator: &'static str,
    render: &RenderContext<'_>,
) -> AnyElement {
    let id = render.element_id(slot);
    let theme = render.theme;
    let order = render.selection_order(slot);
    let scope = render.selection_scope.clone();
    let reveal = render.reveal.map(|reveal| (reveal, slot));
    let reference_scope = render.media.and_then(MediaFetcher::resource_scope);
    let (highlights, overrides) = presentation.resolved(theme);
    let highlights = reveal.map_or_else(
        || Rc::clone(&highlights),
        |(reveal, slot)| reveal.highlights(slot, &presentation.text, Rc::clone(&highlights)),
    );
    let mut element = SelectableText::retained(id, presentation.text.clone(), *theme, Vec::new())
        .selection_order(order, separator)
        .selection_scope(scope)
        .with_shared_highlights(highlights)
        .with_shared_text_run_overrides(overrides)
        .with_inline_file_icons(
            presentation
                .file_icons
                .iter()
                .map(|(offset, (light, dark))| {
                    (
                        *offset,
                        if theme.colors.mode.is_dark() {
                            *dark
                        } else {
                            *light
                        },
                    )
                })
                .collect::<Vec<_>>()
                .into(),
        )
        .with_copy_substitutions(Rc::clone(&presentation.copies));
    if let Some(mentions) = render.mentions {
        let tokens = mentions(&presentation.text)
            .into_iter()
            .filter(|token| {
                !presentation
                    .code_ranges
                    .iter()
                    .chain(presentation.link_ranges.iter())
                    .any(|range| range.start < token.range.end && token.range.start < range.end)
            })
            .collect();
        element = element.with_tokens(tokens);
    }
    if let Some((reveal, slot)) = reveal {
        let end = reveal.visible_end(slot, &presentation.text);
        if end < presentation.text.len() {
            element = element.with_reveal_frontier(end, reveal.frontier_reporter());
        }
    }
    if !presentation.color_ranges.is_empty() {
        // A color value the reply wrote shows as a swatch and copies in
        // every notation.
        let values = Rc::clone(&presentation.color_values);
        element = element
            .with_inline_swatches(Rc::clone(&presentation.swatches))
            .shared_context_ranges(
                Rc::clone(&presentation.color_ranges),
                move |index, position, window, cx| {
                    if let Some(value) = values.get(index) {
                        rich::show_copy_menu(position, rich::color_copy_entries(value), window, cx);
                    }
                },
            );
    }
    if presentation.link_ranges.is_empty() {
        return element.into_any_element();
    }
    let destinations = Rc::clone(&presentation.link_destinations);
    element
        .shared_links(
            Rc::clone(&presentation.link_ranges),
            move |index, window, cx| {
                if let Some(destination) = destinations.get(index) {
                    if let Some(path) = file_link_path(destination) {
                        let line = file_link_line(destination);
                        rich::open_reference(
                            &path.to_string_lossy(),
                            line,
                            reference_scope.clone(),
                            window,
                            cx,
                        );
                    } else {
                        cx.open_url(destination);
                    }
                }
            },
        )
        .into_any_element()
}

fn render_code(code: &PreparedCode, render: &RenderContext<'_>) -> AnyElement {
    let theme = render.theme;
    let fence = &code.fence;
    let note = match &fence.rich {
        Some(rich::RichFence::Ready(prepared)) => return render_rich(code, prepared, render),
        // Instant streaming draws an unfinished block as its source.
        Some(rich::RichFence::Pending { .. })
            if crate::streaming::StreamingMode::current()
                == crate::streaming::StreamingMode::Instant =>
        {
            None
        }
        Some(rich::RichFence::Pending { kind }) => {
            // Most blocks draw without a card, so the placeholder has none.
            return div()
                .w_full()
                .py(px(4.0))
                .child(rich::render_pending(kind, theme))
                .into_any_element();
        }
        Some(rich::RichFence::Invalid { note }) => Some(note.clone()),
        None => None,
    };
    let source = fence.source.clone();
    let highlights = fence.highlights(theme);
    let selector = render.id(code.code);
    let placement = if fence.multiline {
        crate::code_card::CopyPlacement::TopRight
    } else {
        crate::code_card::CopyPlacement::CenterRight
    };
    let copy = render.copy.then(|| {
        CodeCopyButton::new(
            render.id(code.copy),
            render.element_id(code.copy_state),
            source.clone(),
            theme,
        )
    });
    let text_id = render.element_id(code.text);
    let text_order = render.selection_order(code.text);
    let diagram = fence.diagram.as_ref();
    let mut body = body_container(theme, MarkdownBodyTone::Muted)
        .relative()
        .font_family(theme.typography.mono.family)
        .text_size(px(ProseTypography::CODE_SIZE_PX))
        .line_height(px(ProseTypography::CODE_LINE_PX))
        .font_weight(FontWeight::NORMAL)
        .letter_spacing(px(0.0))
        .text_color(theme.colors.text.to_paint());
    body = match diagram {
        Some(diagram) => body.child(diagram::DiagramView::new(
            render.element_id(code.code),
            Arc::clone(diagram),
            theme,
        )),
        None => body.child(fence_text(
            text_id,
            fence,
            highlights,
            theme,
            text_order,
            render.selection_scope.clone(),
        )),
    };
    let mut card = crate::code_card::CodeCard::new(*theme, body).debug_selector(selector);
    if let Some(copy) = copy {
        card = card.copy_action(copy, placement);
    }
    if diagram.is_some() {
        card = card.balanced_gutters();
    }
    let code = card.into_any_element();
    match note {
        // A rich fence that could not draw says why under its code.
        Some(note) => div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(code)
            .child(
                div()
                    .text_size(TypographyTokens::size(TextStep::XsMid))
                    .line_height(px(20.0))
                    .text_color(theme.colors.text_muted.to_paint())
                    .child(note),
            )
            .into_any_element(),
        None => code.into_any_element(),
    }
}

/// Ordinary code has a bounded native line viewport; source-owned selection
/// includes every line even when the native list has not painted it.
#[expect(
    clippy::too_many_lines,
    reason = "one sequential pass; splitting it would only thread its locals through helpers"
)]
fn fence_text(
    id: impl Into<gpui::ElementId>,
    fence: &prepared::PreparedFence,
    highlights: Rc<[(Range<usize>, HighlightStyle)]>,
    theme: &ArtisanTheme,
    order: Vec<u64>,
    scope: SharedString,
) -> AnyElement {
    let id = id.into();
    let lines = Arc::clone(&fence.lines);
    let colors = fence.colors.clone();
    let count = lines.len();
    let theme = *theme;
    let line_px = ProseTypography::CODE_LINE_PX;
    let line_namespace = SharedString::from(id.to_string());
    let list_id = id;
    let scroll = fence.scroll.0.borrow().base_handle.clone();
    let list = gpui::uniform_list(list_id, count, move |range, _, _| {
        range
            .filter_map(|index| {
                lines.get(index).map(|(range, text)| {
                    let mut line_order = order.clone();
                    line_order.push(index as u64);
                    let line_id =
                        gpui::ElementId::NamedInteger(line_namespace.clone(), index as u64);
                    let begin = highlights.partition_point(|(token, _)| token.end <= range.start);
                    let runs: Rc<[_]> = highlights[begin..]
                        .iter()
                        .take_while(|(token, _)| token.start < range.end)
                        .filter_map(|(token, style)| {
                            let start = token.start.max(range.start);
                            let end = token.end.min(range.end);
                            (start < end).then(|| (start - range.start..end - range.start, *style))
                        })
                        .collect();
                    let mut leaf =
                        SelectableText::retained(line_id, text.clone(), theme, Vec::new())
                            .selection_order(line_order, "\n")
                            .selection_scope(scope.clone())
                            .with_shared_highlights(runs);
                    let selected: Vec<_> = colors
                        .ranges
                        .iter()
                        .enumerate()
                        .filter_map(|(index, color)| {
                            (color.start >= range.start && color.end <= range.end).then_some(index)
                        })
                        .collect();
                    if !selected.is_empty() {
                        let values: Rc<[_]> = selected
                            .iter()
                            .map(|index| colors.values[*index].clone())
                            .collect();
                        let ranges: Rc<[_]> = selected
                            .iter()
                            .map(|index| {
                                colors.ranges[*index].start - range.start
                                    ..colors.ranges[*index].end - range.start
                            })
                            .collect();
                        let swatches: Rc<[_]> = colors
                            .swatches
                            .iter()
                            .filter(|&swatch| {
                                swatch.offset >= range.start && swatch.offset < range.end
                            })
                            .map(|swatch| crate::selectable_text::InlineSwatch {
                                offset: swatch.offset - range.start,
                                color: swatch.color,
                                alone: swatch.alone,
                            })
                            .collect();
                        let copies: Rc<[_]> = colors
                            .copies
                            .iter()
                            .filter(|&(copy, _text)| {
                                copy.start >= range.start && copy.end <= range.end
                            })
                            .map(|(copy, text)| {
                                (
                                    copy.start - range.start..copy.end - range.start,
                                    text.clone(),
                                )
                            })
                            .collect();
                        leaf = leaf
                            .with_inline_swatches(swatches)
                            .with_copy_substitutions(copies)
                            .shared_context_ranges(ranges, move |index, position, window, cx| {
                                if let Some(value) = values.get(index) {
                                    rich::show_copy_menu(
                                        position,
                                        rich::color_copy_entries(value),
                                        window,
                                        cx,
                                    );
                                }
                            });
                    }
                    let line_selector = format!("{line_namespace}-line-{index}");
                    div()
                        .debug_selector(move || line_selector)
                        .h(px(line_px))
                        .whitespace_nowrap()
                        .overflow_hidden()
                        .child(leaf)
                })
            })
            .collect::<Vec<_>>()
    })
    .w_full()
    .track_scroll(&fence.scroll)
    .with_width_from_item(Some(fence.widest_line))
    .with_horizontal_sizing_behavior(gpui::ListHorizontalSizingBehavior::Unconstrained)
    .h(px(crate::f32_of_count(count.min(24)) * line_px));
    let viewport = div().relative().w_full().min_w_0().child(list).child(
        gpui::canvas(
            |bounds, window, _| window.insert_hitbox(bounds, gpui::HitboxBehavior::Normal),
            move |_, hitbox, window, _| {
                let view = window.current_view();
                let scroll = scroll.clone();
                window.on_mouse_event(move |event: &gpui::ScrollWheelEvent, phase, window, cx| {
                    if phase != gpui::DispatchPhase::Capture || !hitbox.should_handle_scroll(window)
                    {
                        return;
                    }
                    let delta = event.delta.pixel_delta(px(line_px));
                    let offset = scroll.offset();
                    let max = scroll.max_offset();
                    let next = gpui::point(
                        (offset.x + delta.x).clamp(-max.x, px(0.0)),
                        (offset.y + delta.y).clamp(-max.y, px(0.0)),
                    );
                    if next != offset {
                        scroll.set_offset(next);
                        cx.notify(view);
                        // Consume before the transcript's wheel overlay.
                        // At an edge, let the conversation keep scrolling.
                        cx.stop_propagation();
                    }
                });
            },
        )
        .absolute()
        .inset_0(),
    );
    div()
        .w_full()
        .min_w_0()
        .child(viewport)
        .children((count > 24).then(|| {
            div()
                .pt(px(8.0))
                .text_size(px(12.0))
                .text_color(theme.colors.text_muted.to_paint())
                .child(format!("{count} lines · Scroll to read more"))
        }))
        .into_any_element()
}

/// A rich block: code on the fence's glass card with its copy action,
/// everything else on the transcript.
fn render_rich(
    code: &PreparedCode,
    prepared: &rich::Prepared,
    render: &RenderContext<'_>,
) -> AnyElement {
    let cx = rich::BlockContext {
        theme: render.theme,
        media: render.media,
        id: render.id(code.code),
    };
    let content = rich::render_content(prepared, &cx);
    let padding = match prepared.chrome() {
        rich::Chrome::Card => {
            let copy = prepared.copy().filter(|_| render.copy);
            return rich_card(code, render, content, copy);
        }
        rich::Chrome::Open => 4.0,
        rich::Chrome::Bare => 0.0,
    };
    let selector = render.id(code.code);
    div()
        .w_full()
        .min_w_0()
        .py(px(padding))
        .text_color(render.theme.colors.text.to_paint())
        .child(content)
        .debug_selector(move || selector.to_string())
        .into_any_element()
}

/// The fence's glass card around `content`, with the copy action in its
/// corner when there is something to copy.
fn rich_card(
    code: &PreparedCode,
    render: &RenderContext<'_>,
    content: AnyElement,
    copy: Option<SharedString>,
) -> AnyElement {
    let theme = render.theme;
    let selector = render.id(code.code);
    let mut card = crate::code_card::CodeCard::new(*theme, content).debug_selector(selector);
    if let Some(source) = copy {
        card = card.copy_action(
            CodeCopyButton::new(
                render.id(code.copy),
                render.element_id(code.copy_state),
                source,
                theme,
            ),
            crate::code_card::CopyPlacement::TopRight,
        );
    }
    card.into_any_element()
}

/// The fence body as displayed: without the line terminator that ends its
/// last line. The engine keeps the body verbatim, newline included, and a
/// text element paints that terminator as one more, empty, line.
fn fence_display_source(source: &str) -> &str {
    let body = source.strip_suffix('\n').unwrap_or(source);
    body.strip_suffix('\r').unwrap_or(body)
}

fn valid_code_range(token: &CodeToken, source: &str) -> Option<(Range<usize>, CodeTokenKind)> {
    let range = token.range.clone();
    (range.start <= range.end
        && range.end <= source.len()
        && source.is_char_boundary(range.start)
        && source.is_char_boundary(range.end))
    .then_some((range, token.kind))
}

/// One openable link run.
///
/// Its byte range in the flattened source plus the verbatim absolute
/// destination it opens. Carried separately from the
/// highlight runs so click handling and the future selection consumer share
/// one metadata source.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct InlineLink {
    /// Byte range of the visible label in the flattened source.
    pub range: Range<usize>,
    /// Verbatim absolute web, email, or file destination.
    pub destination: String,
}

/// Owned inline presentation for one span slice: flattened text, one
/// ordered non-overlapping highlight list, openable-link metadata, and
/// inline-code ranges.
///
/// FROZEN range contract for the selection extension: `source` owns the
/// text every range addresses; `highlights` are sorted, non-overlapping,
/// and on character boundaries (exactly what one `with_highlights` call
/// consumes); `links` are openable ranges in source order;
/// `code_ranges` are the inline-code sub-ranges in source order,
/// non-overlapping and on character boundaries (exactly what
/// `with_font_family_overrides` consumes for the mono face, with run-level
/// tracking reset beside it). Additive changes only — never reshape.
///
#[derive(Clone, Debug, Default, PartialEq)]
pub struct InlinePresentation {
    /// Flattened visible text.
    pub source: String,
    /// Highlight runs in source order, non-overlapping, on character
    /// boundaries: exactly what one `with_highlights` call consumes.
    pub highlights: Vec<(Range<usize>, HighlightStyle)>,
    /// Openable links in source order.
    pub links: Vec<InlineLink>,
    /// Inline-code ranges in source order for family/tracking treatment.
    pub code_ranges: Vec<Range<usize>>,
}

/// Flattens inline spans into text and source-ordered, nonoverlapping runs.
///
/// Nested styles inherit their parent through [`HighlightStyle::highlight`];
/// one linear traversal emits the combined ranges consumed by StyledText.
///
/// # Must use
///
/// The return value owns the flattened text the ranges address; dropping it
/// silently loses the presentation.
#[must_use]
pub fn present_inline(spans: &[Span], theme: ArtisanTheme) -> InlinePresentation {
    let flat = flatten_inline(spans);
    let highlights = flat.resolve_runs(&theme);
    InlinePresentation {
        source: flat.source,
        highlights,
        links: flat.links,
        code_ranges: flat.code_ranges,
    }
}

/// Recognizes bare HTTP(S) URLs while leaving sentence punctuation outside.
fn bare_url_spans(text: &str) -> Option<Vec<Span>> {
    let mut result = Vec::new();
    let mut consumed = 0;
    let mut cursor = 0;
    while cursor < text.len() {
        let Some(relative) = text[cursor..].find("http") else {
            break;
        };
        let start = cursor + relative;
        cursor = start + 4;
        if !(text[start..].starts_with("https://") || text[start..].starts_with("http://"))
            || text[..start]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_alphanumeric() || c == '_')
        {
            continue;
        }
        let tail = &text[start..];
        let mut end = tail
            .find(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '"' | '\'' | '`'))
            .unwrap_or(tail.len());
        let mut balance = [0isize; 3];
        for ch in tail[..end].chars() {
            for (index, (open, close)) in [('(', ')'), ('[', ']'), ('{', '}')].iter().enumerate() {
                if ch == *open {
                    balance[index] -= 1;
                }
                if ch == *close {
                    balance[index] += 1;
                }
            }
        }
        while let Some(last) = tail[..end].chars().next_back() {
            let closer = [')', ']', '}'].iter().position(|close| *close == last);
            if matches!(last, '.' | ',' | ';' | ':' | '!' | '?') {
                end -= last.len_utf8();
            } else if let Some(index) = closer.filter(|index| balance[*index] > 0) {
                balance[index] -= 1;
                end -= last.len_utf8();
            } else {
                break;
            }
        }
        let destination = &tail[..end];
        let authority = destination
            .split_once("://")
            .map(|(_, rest)| rest.split(['/', '?', '#']).next().unwrap_or_default())
            .unwrap_or_default();
        if authority.is_empty() || !is_openable_link_destination(destination) {
            continue;
        }
        if consumed < start {
            result.push(Span::Text(text[consumed..start].to_owned()));
        }
        result.push(Span::Link {
            label: vec![Span::Text(destination.to_owned())],
            destination: destination.to_owned(),
        });
        consumed = start + end;
        cursor = consumed;
    }
    if result.is_empty() {
        return None;
    }
    if consumed < text.len() {
        result.push(Span::Text(text[consumed..].to_owned()));
    }
    Some(result)
}

/// Web/email links open in the browser; file links open in the thread preview.
fn is_openable_link_destination(destination: &str) -> bool {
    let lower = destination.to_ascii_lowercase();
    lower.starts_with("https://")
        || lower.starts_with("http://")
        || lower.starts_with("mailto:")
        || file_link_path(destination).is_some()
}

/// Resolves absolute native paths and local file URIs, including code references
/// with a trailing line/column or fragment. Never accepts a remote file authority.
fn file_link_path(destination: &str) -> Option<std::path::PathBuf> {
    let path = if destination.to_ascii_lowercase().starts_with("file:") {
        let url = url::Url::parse(destination).ok()?;
        if url.host_str().is_some_and(|host| host != "localhost") {
            return None;
        }
        url.to_file_path().ok()?
    } else {
        let path = destination.split('#').next()?;
        let mut path = path;
        for _ in 0..2 {
            if let Some((base, line)) = path.rsplit_once(':')
                && !line.is_empty()
                && line.bytes().all(|byte| byte.is_ascii_digit())
            {
                path = base;
            }
        }
        std::path::PathBuf::from(path)
    };
    let raw = destination.split('#').next().unwrap_or_default();
    let has_scheme = raw.split_once(':').is_some_and(|(prefix, _)| {
        !prefix.contains(['/', '\\'])
            && !prefix.is_empty()
            && prefix
                .bytes()
                .next()
                .is_some_and(|b| b.is_ascii_alphabetic())
            && prefix
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.'))
    });
    (!path.as_os_str().is_empty()
        && !destination.starts_with("//")
        && (!has_scheme
            || destination.to_ascii_lowercase().starts_with("file:")
            || raw.split_once(':').is_some_and(|(_, suffix)| {
                !suffix.is_empty()
                    && suffix
                        .split(':')
                        .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
            })))
    .then_some(path)
}

fn command_style(theme: &ArtisanTheme) -> HighlightStyle {
    // As the composer paints a special command.
    HighlightStyle {
        color: Some(theme.colors.mention_command.to_paint()),
        font_weight: Some(FontWeight::MEDIUM),
        ..Default::default()
    }
}

fn code_style(theme: &ArtisanTheme, in_link: bool) -> HighlightStyle {
    // Reference inline code reads 400 muted with no wash (`prose.css`
    // inline-code over plugin `code`), except inside a link where `a code`
    // inherits the link color. The 400 weight rides here; mono face and
    // normal tracking ride the frozen text-run contract
    // (`InlinePresentation.code_ranges` → `TextRunOverride` with the mono
    // family and zero tracking). Size deliberately has no override in that
    // API, so inline code keeps the inherited 16 px instead of the
    // reference 0.875 em (14 px) — a stated limit, not parity.
    HighlightStyle {
        color: if in_link {
            None
        } else {
            Some(theme.colors.text_muted.to_paint())
        },
        font_weight: Some(FontWeight::NORMAL),
        ..Default::default()
    }
}

fn strong_style(theme: &ArtisanTheme, in_link: bool) -> HighlightStyle {
    // Plugin `strong` 600 in the headings/bold foreground token
    // (`prose.css` bold rule); body inheritance would dim it to muted.
    // Inside a link, `a strong` inherits the link color instead.
    HighlightStyle {
        color: if in_link {
            None
        } else {
            Some(theme.colors.text.to_paint())
        },
        font_weight: Some(ProseTypography::STRONG_WEIGHT),
        ..Default::default()
    }
}

fn emphasis_style() -> HighlightStyle {
    HighlightStyle {
        font_style: Some(FontStyle::Italic),
        ..Default::default()
    }
}

/// Extracts a line number from common file-link suffixes.
fn file_link_line(destination: &str) -> Option<u32> {
    if let Some((_, fragment)) = destination.split_once('#') {
        return fragment.strip_prefix('L').unwrap_or(fragment).parse().ok();
    }
    let mut parts = destination.rsplit(':');
    let last = parts.next()?.parse::<u32>().ok()?;
    Some(
        parts
            .next()
            .and_then(|part| part.parse().ok())
            .unwrap_or(last),
    )
}

fn link_style(theme: &ArtisanTheme) -> HighlightStyle {
    // Links use the link token and authored label.
    HighlightStyle {
        color: Some(theme.colors.link.to_paint()),
        font_weight: Some(ProseTypography::LINK_WEIGHT),
        ..Default::default()
    }
}

fn code_token_style(theme: &ArtisanTheme, kind: CodeTokenKind) -> HighlightStyle {
    let color = match kind {
        CodeTokenKind::Comment => theme.colors.syntax.comment,
        CodeTokenKind::Str => theme.colors.syntax.string,
        CodeTokenKind::Number => theme.colors.syntax.number,
        CodeTokenKind::Keyword => theme.colors.syntax.keyword,
        CodeTokenKind::Type => theme.colors.syntax.r#type,
        CodeTokenKind::Function => theme.colors.syntax.function,
        CodeTokenKind::Plain => theme.colors.text,
    };
    HighlightStyle {
        color: Some(color.to_paint()),
        ..Default::default()
    }
}

#[cfg(test)]
#[path = "markdown_renderer_tests.rs"]
mod tests;
