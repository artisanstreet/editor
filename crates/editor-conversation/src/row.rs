//! One native transcript row and its immutable Item model.
//! Text rows draw the body the conversation prepared for them; a row
//! builds only its own visible content; the native list owns measurement.

mod presenters;
use presenters::{Action, Actions};

use crate::Conversation;
use crate::activity;
use crate::body::{Body, Full, Markdown, Plain, TextBody};
use crate::content::{SourceOwners, StoredText};
use crate::disclosure::RowDisclosure;
use crate::fold::{PlainSection, Row, RunSummary, Slot, TextSection};
use crate::motion::{
    CompactionPhase, RevealQueue, WorkMotion, eased, header_entering, phrase_or_text,
    tabular_figures, with_entrance,
};
use crate::phase::{self, Phase};
use crate::source_actions::copy_action;
use crate::thread::ItemUi;
use artisan_api::{
    BlobHash, Item, ItemId, ItemKind, LiveTarget, ModelSwitch, NamedModel, NoticeKind, RunId,
    Status, Timestamp,
};
use editor_ui::AssetId;
use editor_ui::asset_seam::asset_glyph;
use editor_ui::button::{Button, ButtonVariant};
use editor_ui::chat_bubble::{ChatBubbleSegment, SegmentPosition};
use editor_ui::code_card::CodeCard;
use editor_ui::inline_code_text::inline_runs;
use editor_ui::markdown_renderer::{MarkdownRenderer, PreparedReveal};
use editor_ui::motion::{MotionCurve, MotionPolicy};
use editor_ui::selectable_text::{SelectableText, register_selection_source};
use editor_ui::shimmer_text::{ShimmerLine, ShimmerText, ShimmerTiming, ShimmerTone};
use editor_ui::theme::{
    ActiveTheme as _, ArtisanTheme, Oklch, Opacity, ProseTypography, RadiusStep, RadiusTokens,
};
use gpui::{
    AnyElement, Context, Div, FontWeight, Hsla, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _,
    WeakEntity, Window, div, prelude::FluentBuilder as _, px, svg,
};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

/// The turn column, gutter included (`TRANSCRIPT_PROSE_WIDTH_PX`).
pub const COLUMN: f32 = 768.0;
/// The gutter either side of the column (`TRANSCRIPT_GUTTER_PX`).
pub const GUTTER: f32 = 24.0;
/// The prose measure inside the column (`TRANSCRIPT_PROSE_BODY_WIDTH_PX`).
pub const PROSE_MEASURE: f32 = 672.0;
/// One body line inside work sections and chains (`steps(6)`).
const LINE: f32 = 24.0;
/// Shown live output keeps this many trailing bytes.
const LIVE_OUTPUT_TAIL: usize = 8 * 1024;

pub use editor_core::BlobImage as Blob;

/// What a row is drawn with, fixed when it is made: the theme, where its
/// actions go, the renderer that draws prepared text, and the reveal order
/// rows take turns in.
#[derive(Clone)]
pub struct RowEnv {
    pub theme: ArtisanTheme,
    pub conversation: WeakEntity<Conversation>,
    pub markdown: Rc<MarkdownRenderer>,
    /// Fresh rows reveal in transcript order: each holds while an earlier
    /// one still settles. Rows check in as they draw.
    pub reveal: Rc<RefCell<RevealQueue>>,
    /// The selection document of the thread or agent shown.
    pub scope: SharedString,
    /// An agent's view orders its task and report around its work.
    pub agent_scope: bool,
    pub host: Option<WeakEntity<editor_core::HostData>>,
}

pub struct RowModel {
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
    /// Space above the row: the transcript rhythm (see `fold::gap`).
    pub gap: f32,
    /// Space below the row: the end space under the last row, else zero.
    pub end: f32,
    /// The text the row shows, as the conversation prepared it.
    pub text: Option<TextBody>,
}

pub struct RowView {
    model: RowModel,
    version: u64,
    env: RowEnv,
    /// Where a disclosure in flight clips this row.
    disclosure: RowDisclosure,
    /// A local paint effect is running.
    pub animating: bool,
    /// Local opacity entrance and the finite compaction separator.
    motion: WorkMotion,
    prose_reveal: Rc<PreparedReveal>,
    queue_held: bool,
    prose_fresh: bool,
    reported_height: Rc<Cell<Option<gpui::Pixels>>>,
    /// What it shows changed since `reported_height` was measured.
    remeasure: Rc<Cell<bool>>,
    /// The body height last told to the conversation.
    reported_body: Rc<Cell<Option<gpui::Pixels>>>,
    stored: SourceOwners,
    collections: crate::collections::CollectionOwners,
    /// Tool output and answers, selectable as one source each.
    plain: presenters::text::PlainSources,
}

impl Drop for RowView {
    fn drop(&mut self) {
        self.env.reveal.borrow_mut().remove(&self.model.row.key());
    }
}

impl RowView {
    /// Its text is still preparing: the placeholder shows, at another height.
    pub(crate) const fn preparing(&self) -> bool {
        matches!(
            self.model.text,
            Some(TextBody {
                body: Body::Loading(_),
                ..
            })
        )
    }

    pub(crate) fn awaiting_reveal_admission(&self) -> bool {
        match self.model.row.semantic() {
            Row::Text {
                slot: Slot::Prose, ..
            } => self.prose_fresh && !self.prose_reveal.has_presented(),
            Row::WorkEntry { .. } | Row::Chain { .. } | Row::Prompt(_) | Row::Image(_) => {
                !self.motion.entrance_started()
                    && self
                        .model
                        .row
                        .anchor()
                        .is_some_and(|id| self.ui(id).is_some_and(ItemUi::fresh))
            }
            Row::Projected { .. }
            | Row::Work { .. }
            | Row::EarlierWork { .. }
            | Row::Text { .. }
            | Row::Compaction(_)
            | Row::Gap
            | Row::Status(_)
            | Row::AgentResult(_)
            | Row::AgentStatus(..) => false,
        }
    }

    fn actions(&self) -> Actions {
        Actions::new(self.env.conversation.clone(), self.env.scope.clone())
    }
    fn text(&self) -> presenters::text::TextPresenter<'_> {
        presenters::text::TextPresenter {
            theme: self.env.theme,
            owners: &self.stored,
            host: self.env.host.clone(),
            plain: &self.plain,
            scope: &self.env.scope,
            agent_scope: self.env.agent_scope,
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
    pub(crate) fn new(
        model: RowModel,
        version: u64,
        env: RowEnv,
        prose_fresh: bool,
        disclosure: RowDisclosure,
    ) -> Self {
        Self {
            model,
            version,
            env,
            disclosure,
            animating: false,
            motion: WorkMotion::default(),
            prose_reveal: Rc::new(PreparedReveal::default()),
            queue_held: false,
            prose_fresh,
            reported_height: Rc::default(),
            remeasure: Rc::default(),
            reported_body: Rc::default(),
            stored: Rc::new(RefCell::new(HashMap::new())),
            collections: crate::collections::CollectionOwners::default(),
            plain: presenters::text::PlainSources::default(),
        }
    }

    /// A disclosure's flight moved: where it clips this row, and whether
    /// the row waits for it.
    pub(crate) fn set_disclosure(&mut self, disclosure: RowDisclosure, cx: &mut Context<Self>) {
        if self.disclosure == disclosure {
            return;
        }
        let clip_changed = self.disclosure.clip != disclosure.clip;
        let gap_changed = self.disclosure.gap != disclosure.gap;
        self.disclosure = disclosure;
        if clip_changed || gap_changed {
            self.animating =
                self.motion.end_frame() || self.prose_reveal.is_animating() || self.queue_held;
            self.refresh(cx);
        } else {
            cx.notify();
        }
    }

    /// What it shows changed: draw it fresh, and measure it again.
    pub(crate) fn refresh(&self, cx: &mut Context<Self>) {
        // The last height stays, so a new one reaches the conversation.
        self.remeasure.set(true);
        cx.notify();
    }

    /// The height to draw this row from cache at: known, and no motion
    /// running.
    pub(crate) fn cached_height(&self) -> Option<gpui::Pixels> {
        self.reported_height
            .get()
            .filter(|_| !self.animating && !self.remeasure.get())
    }

    pub(crate) fn ready_to_read(&self) -> bool {
        !self.queue_held && !self.prose_reveal.is_animating() && !self.disclosure.hidden
    }

    pub(crate) const fn version(&self) -> u64 {
        self.version
    }

    /// New content: re-render (and re-measure) this row only.
    pub(crate) fn set(&mut self, model: RowModel, version: u64, cx: &mut Context<Self>) {
        self.stored.borrow_mut().retain(|(id, field), (_, owner)| {
            let source = model.items.get(id).and_then(|item| field.text(item));
            let Some(artisan_api::TextContent::Stored(source)) = source else {
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

    /// Its body finished preparing, or its live text moved on.
    pub(crate) fn set_body(&mut self, text: Option<TextBody>, cx: &mut Context<Self>) {
        self.model.text = text;
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
            self.model.live.insert(target.clone(), Arc::clone(text));
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

    const fn theme(&self) -> ArtisanTheme {
        self.env.theme
    }

    /// Newly observed compact content fades in without changing its measured height.
    fn entering(&mut self, id: &ItemId, element: AnyElement) -> AnyElement {
        if self.disclosure.clip.is_some() {
            // Unfolded by the reader, it is history: it has entered, so it
            // neither holds the rows after it nor enters once the slide ends.
            let _ = self.motion.entrance(id.as_str(), false);
            return element;
        }
        if self.queue_held {
            return element;
        }
        let fresh = self.ui(id).is_some_and(ItemUi::fresh);
        let entrance = self.motion.entrance(id.as_str(), fresh);
        with_entrance(element, entrance, px(self.model.gap))
    }

    /// The prepared body this row was given, drawn.
    fn body(
        &mut self,
        body: &Body,
        reveal: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = self.theme();
        if reveal && matches!(body, Body::Failed { .. } | Body::Disconnected) {
            self.prose_reveal.mark_presented();
        }
        match body {
            Body::Empty => div().into_any_element(),
            Body::Loading(text) => div()
                .debug_selector(|| "text-loading".to_owned())
                .child(*text)
                .into_any_element(),
            Body::Disconnected => div()
                .child("This Forge is no longer connected.")
                .into_any_element(),
            Body::Failed { error, full } => div()
                .child(error.clone())
                .child(match full {
                    Full::Copy(key, source) => copy_action(key.to_string(), &theme, source.clone()),
                    Full::Stored(source, field) => {
                        let id = self.model.row.anchor().cloned().unwrap_or_default();
                        StoredText {
                            host: self.env.host.clone(),
                            source: source.clone(),
                            theme,
                            key: (id, field.clone()),
                            owners: Rc::clone(&self.stored),
                        }
                        .into_any_element()
                    }
                })
                .into_any_element(),
            Body::NativeFailed { error, key, row } => {
                let renderer = Rc::clone(&self.env.markdown);
                let owner = self.env.conversation.clone();
                let (retry_row, source_key) = (row.clone(), key.clone());
                div()
                    .child(error.clone())
                    .child(
                        Button::text(
                            format!("retry-native-{}", key.replace(":leaf:", "-")),
                            theme,
                            "Retry",
                        )
                        .on_activate(move |_, _, cx| {
                            renderer.retry_native_window(&source_key);
                            owner
                                .update(cx, |this, cx| this.retry_native_body(&retry_row, cx))
                                .ok();
                        }),
                    )
                    .into_any_element()
            }
            Body::Markdown(markdown) => self.markdown(markdown, reveal, window),
            Body::Plain(plain) => self.plain(plain, window, cx),
        }
    }

    fn markdown(&mut self, body: &Markdown, reveal: bool, window: &Window) -> AnyElement {
        let theme = self.theme();
        let renderer = &self.env.markdown;
        renderer.set_selection_prefix(body.order.clone());
        let content = if reveal {
            let content = renderer.render_prepared_revealing_range(
                &body.prepared,
                body.range.clone(),
                theme,
                body.selector.clone(),
                body.tone,
                &self.prose_reveal,
            );
            if self.prose_reveal.is_animating() {
                self.animating = true;
                window.request_animation_frame();
            }
            content
        } else if body.code_copy {
            renderer.render_prepared_range(
                &body.prepared,
                body.range.clone(),
                theme,
                body.selector.clone(),
                body.tone,
            )
        } else {
            renderer.render_preview_range(
                &body.prepared,
                body.range.clone(),
                theme,
                body.selector.clone(),
                body.tone,
            )
        };
        if !body.wrapped {
            return content;
        }
        div()
            .w_full()
            .child(content)
            .when_some(body.notice.clone(), |root, notice| {
                root.child(editor_ui::page::notice(&theme, notice))
                    .children(test_mark("live-preview"))
            })
            .when_some(body.copy.clone(), |root, (key, source)| {
                root.child(copy_action(key.to_string(), &theme, source))
            })
            .into_any_element()
    }

    #[expect(
        clippy::significant_drop_tightening,
        reason = "false positive: the text and owners move into the returned element"
    )]
    fn plain(&self, body: &Plain, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        register_selection_source(&body.source, window, cx);
        let theme = self.theme();
        let mut text =
            SelectableText::retained(body.key.clone(), body.text.clone(), theme, Vec::new());
        if body.slice {
            text = text.source_slice(0..body.text.len());
        }
        let text = text
            .selection_scope(self.env.scope.clone())
            .selection_order(body.order.clone(), body.separator);
        let Some(native) = &body.native else {
            return text.into_any_element();
        };
        let owner = self.env.conversation.clone();
        let (notify_owner, layout_owner, paint_owner, failure_owner) =
            (owner.clone(), owner.clone(), owner.clone(), owner);
        let (layout_base, paint_base, failed_base) = (
            native.base.clone(),
            native.base.clone(),
            native.base.clone(),
        );
        let (layout_leaf, paint_leaf, failed_leaf) = (
            Arc::clone(&native.leaf),
            Arc::clone(&native.leaf),
            Arc::clone(&native.leaf),
        );
        let notify_base = native.base.clone();
        text.native_window(
            Rc::clone(&self.env.markdown),
            native.key.clone(),
            native.visual,
            native.window.clone(),
            editor_ui::selectable_text::NativeWindowEvents {
                notify: Rc::new(move |cx| {
                    notify_owner
                        .update(cx, |this, cx| {
                            if let Row::Text { id, slot, .. } = &notify_base {
                                this.body_ready(id, *slot, cx);
                            }
                        })
                        .ok();
                }),
                layout_ready: Rc::new(move |ready, cx| {
                    layout_owner
                        .update(cx, |this, cx| {
                            this.native_body_ready(&layout_base, &layout_leaf, &ready, false, cx);
                        })
                        .ok();
                }),
                ready: Rc::new(move |ready, cx| {
                    paint_owner
                        .update(cx, |this, cx| {
                            this.native_body_ready(&paint_base, &paint_leaf, &ready, true, cx);
                        })
                        .ok();
                }),
            },
        )
        .native_failure_handler(Rc::new(move |error, cx| {
            failure_owner
                .update(cx, |this, cx| {
                    this.native_body_failed(&failed_base, &failed_leaf, error, cx);
                })
                .ok();
        }))
        .into_any_element()
    }

    /// A text row: a reply's prose, a formatted section, or a literal
    /// body in its bubble, error card or caption.
    fn text_row(&mut self, slot: Slot, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(text) = self.model.text.clone() else {
            return div().into_any_element();
        };
        let Slot::Plain(section) = slot else {
            if slot == Slot::Text(TextSection::Note) {
                let content = self.body(&text.body, false, window, cx);
                return self.note_body(content, text.first, text.last);
            }
            if slot == Slot::Text(TextSection::User) {
                let content = self.body(&text.body, false, window, cx);
                return self.user_body(content, text.first, text.last);
            }
            if slot.is_card() {
                let content = self.body(&text.body, false, window, cx);
                return CodeCard::new(self.theme(), content).into_any_element();
            }
            if slot == Slot::Prose {
                let running = self
                    .model
                    .row
                    .anchor()
                    .and_then(|id| self.item(id))
                    .is_some_and(|item| item.status == Status::Running);
                self.prose_reveal
                    .begin_frame(running, self.prose_fresh, cx.reduce_motion());
            }
            return self.body(&text.body, slot == Slot::Prose, window, cx);
        };
        let content = self.body(&text.body, false, window, cx);
        let theme = self.theme();
        let (first, last) = (text.first, text.last);
        match section {
            PlainSection::User => return self.user_body(content, first, last),
            PlainSection::Error => return self.error_body(content, first, last),
            PlainSection::ImageCaption => {
                return div()
                    .w_full()
                    .text_size(theme.typography.control_text)
                    .text_color(theme.colors.text_muted.to_paint())
                    .child(content)
                    .into_any_element();
            }
            PlainSection::AgentTask | PlainSection::AgentMessage => {}
        }
        div()
            .w_full()
            .flex()
            .flex_col()
            .items_end()
            .child(ChatBubbleSegment::new(
                theme,
                segment_position(first, last),
                content,
            ))
            .into_any_element()
    }

    /// Only the header lives here. Expanded descendants are independent native rows.
    fn work_row(
        &mut self,
        run: Option<&RunId>,
        segment: usize,
        has_work: bool,
        _cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if segment != 0 {
            return None;
        }
        let summary = self.model.run.as_ref()?;
        let label = self.section_title(summary);
        let open = summary.open;
        let controlled = !summary.live() && run.is_some() && has_work;
        let theme = self.theme();
        let muted = theme.colors.text_muted.to_paint();
        let key = format!("work-{}", run.map_or_else(String::new, ToString::to_string));
        let words = self.motion.phrase(&format!("{key}-label"), &label);
        let label = phrase_or_text(words, label, px(LINE));
        let local_fraction = self.motion.chevron(&key, open);
        let fraction = self.disclosure.fraction.unwrap_or(local_fraction);
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
            .tracking(theme.typography.tracking_em)
            .text_color(muted)
            .font_features(tabular_figures())
            .pb(theme.spacing.steps(2.0))
            .debug_selector(|| "work-header".to_owned())
            .child(near)
            .child(
                div()
                    .absolute()
                    .bottom_0()
                    .left_0()
                    .w_full()
                    .h(px(1.0))
                    .bg(theme.colors.line.to_paint()),
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
            theme.colors.text.to_paint()
        } else {
            theme.colors.text_muted.to_paint()
        };
        let key = format!("chain-{first}");
        let words = self.motion.phrase(&format!("{key}-clause"), &clause);
        let clause = phrase_or_text(words, clause, px(LINE));
        let local_fraction = self.motion.chevron(&key, open);
        let fraction = self.disclosure.fraction.unwrap_or(local_fraction);
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
                    .tracking(theme.typography.tracking_em)
                    .line_height(px(LINE))
                    .text_color(color)
                    .font_features(tabular_figures())
                    .child(clause),
            )
            .child(chevron(fraction, 14.0, color))
            .into_any_element()
    }

    /// A non-activity row inside the work: plan, notice, nested messages.
    fn entry(&mut self, item: &Item, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        match &item.kind {
            ItemKind::Tool { .. }
            | ItemKind::UserMessage {
                from_agent: Some(_),
                ..
            } => {
                let live = self.live_text(&LiveTarget::Shell {
                    item_id: item.id.clone(),
                });
                let expanded = self
                    .ui(&item.id)
                    .and_then(|ui| ui.expanded)
                    .unwrap_or(false);
                // An opened activity shows its preview once it has run.
                let preview = self
                    .model
                    .text
                    .clone()
                    .filter(|_| {
                        expanded
                            && activity::agent_message(item).is_none()
                            && (live.is_none() || item.status.is_terminal())
                    })
                    .map(|text| self.body(&text.body, false, window, cx));
                self.tools().activity_row(
                    item,
                    expanded,
                    live.as_deref(),
                    &self.model.agents,
                    preview,
                )
            }
            ItemKind::Agent { .. } => self.agent().agent_row(item),
            ItemKind::Plan { .. } => self.tools().plan_entry(item),
            ItemKind::Notice { .. } => self.text().notice(item),
            ItemKind::Question { .. } => self.requests().question_card(item),
            // These have rows of their own.
            ItemKind::Compaction { .. }
            | ItemKind::AssistantMessage { .. }
            | ItemKind::UserMessage { .. }
            | ItemKind::Error { .. }
            | ItemKind::Image { .. } => div().into_any_element(),
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
            Status::AwaitingUser | Status::Failed | Status::Declined | Status::Cancelled => {
                CompactionPhase::Stopped
            }
        };
        let switch = match &item.kind {
            ItemKind::Compaction { switch, .. } => switch.clone(),
            ItemKind::UserMessage { .. }
            | ItemKind::AssistantMessage { .. }
            | ItemKind::Tool { .. }
            | ItemKind::Agent { .. }
            | ItemKind::Question { .. }
            | ItemKind::Plan { .. }
            | ItemKind::Image { .. }
            | ItemKind::Notice { .. }
            | ItemKind::Error { .. } => None,
        };
        let fresh = self.ui(id).is_some_and(ItemUi::fresh);
        let frame = self.motion.compaction(id.as_str(), phase, fresh);
        // The slot keeps the widest word it can show in flow, invisibly; the
        // visible words sit centred over it, so the rules keep their length.
        // A switch says the same line in every phase.
        let widest = match phase {
            CompactionPhase::Stopped => CompactionPhase::Stopped.label(),
            CompactionPhase::Running | CompactionPhase::Done => CompactionPhase::Running.label(),
        };
        let label = |phase: CompactionPhase| {
            switch.as_ref().map_or_else(
                || {
                    ShimmerText::new("compaction-status", phase.label(), theme)
                        .active(phase == CompactionPhase::Running)
                        .timing(ShimmerTiming::Progress)
                        .tone(ShimmerTone::Muted)
                        .into_any_element()
                },
                |switch| {
                    ShimmerLine::new("compaction-status", switch_line(switch, &theme), theme)
                        .active(phase == CompactionPhase::Running)
                        .timing(ShimmerTiming::Progress)
                        .into_any_element()
                },
            )
        };
        let word = |phase: CompactionPhase| {
            div()
                .absolute()
                .top(px(0.0))
                .left(px(0.0))
                .right(px(0.0))
                .flex()
                .justify_center()
                .child(label(phase))
        };
        let placeholder = switch.as_ref().map_or_else(
            || div().child(widest).into_any_element(),
            |switch| switch_line(switch, &theme),
        );
        let mut slot = div()
            .relative()
            .flex_shrink_0()
            .child(div().opacity(0.0).child(placeholder));
        // A switch's line doesn't change as it settles; only the highlight runs.
        slot = match frame.swap.filter(|_| switch.is_none()) {
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
                let enter = eased(MotionCurve::EaseInOut, swap.mul_add(2.0, -1.0));
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
        let muted = theme.colors.text_muted.to_paint();
        let rule_color = theme.colors.line.to_paint();
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
                    .bg(theme.colors.text.fade(alpha).to_paint());
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
            .tracking(theme.typography.tracking_em)
            .text_color(muted)
            .debug_selector(|| "compaction".to_owned())
            .child(rule(true))
            .child(slot)
            .child(rule(false))
            .into_any_element()
    }

    /// Error body styling is shared across its bounded native leaves.
    fn error_body(&self, content: AnyElement, first: bool, last: bool) -> AnyElement {
        let theme = self.theme();
        let mut root = div()
            .w_full()
            .min_w_0()
            .px(theme.spacing.steps(3.5))
            .pt(px(if first { 12.0 } else { 0.0 }))
            .pb(px(if last { 12.0 } else { 0.0 }))
            .border_x_1()
            .border_color(theme.colors.red.border.to_paint())
            .bg(theme.colors.red.bg.to_paint())
            .text_color(theme.colors.text_muted.to_paint())
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
                    .text_color(theme.colors.destructive_text.to_paint())
                    .child("Error"),
            );
        }
        root.child(content).into_any_element()
    }

    /// A user message: its attachments over its first block, its markdown
    /// in the bubble. One another thread's agent sent is tinted with the
    /// sender's harness.
    fn user_body(&self, content: AnyElement, first: bool, last: bool) -> AnyElement {
        let item = self.model.row.anchor().and_then(|id| self.item(id));
        let (engine, empty) = match item.map(|item| &item.kind) {
            Some(ItemKind::UserMessage { sender, text, .. }) => (
                sender.as_ref().map(|sender| sender.engine),
                matches!(text, artisan_api::TextContent::Inline(text) if text.is_empty()),
            ),
            _ => (None, false),
        };
        let attachments = first
            .then(|| item.and_then(|item| self.media().user_attachments(item)))
            .flatten();
        let root = div()
            .w_full()
            .flex()
            .flex_col()
            .items_end()
            .gap(px(8.0))
            .children(attachments);
        if empty {
            return root.into_any_element();
        }
        let position = segment_position(first, last);
        let bubble = ChatBubbleSegment::new(self.theme(), position, content);
        let Some(engine) = engine else {
            return root.child(bubble).into_any_element();
        };
        let block = match self.model.row.semantic() {
            Row::Text { block, .. } => *block,
            Row::Projected { .. }
            | Row::Work { .. }
            | Row::EarlierWork { .. }
            | Row::WorkEntry { .. }
            | Row::Chain { .. }
            | Row::Prompt(_)
            | Row::Compaction(_)
            | Row::Image(_)
            | Row::Gap
            | Row::Status(_)
            | Row::AgentResult(_)
            | Row::AgentStatus(..) => 0,
        };
        root.child(bubble.tinted(editor_ui::engine_accent(&self.theme(), engine)))
            .children(test_mark(format!("sent-by:{engine}:{block}:{position:?}")))
            .into_any_element()
    }

    /// A "You should know" note: a tinted card headed by its tag, the note
    /// below. "Heads up" (a decision to judge) is tinted as
    /// a warning, "You should know" as information.
    fn note_body(&self, content: AnyElement, first: bool, last: bool) -> AnyElement {
        let theme = self.theme();
        let heads_up = matches!(
            self.model
                .row
                .anchor()
                .and_then(|id| self.item(id))
                .map(|item| &item.kind),
            Some(ItemKind::Notice {
                kind: NoticeKind::YouShouldKnow { heads_up: true },
                ..
            })
        );
        let (tint, asset, tag) = if heads_up {
            (
                theme.colors.banner_warning,
                AssetId::TABLER_ALERT_TRIANGLE,
                "Heads up",
            )
        } else {
            (
                theme.colors.banner_info,
                AssetId::TABLER_SPARKLES,
                "You should know",
            )
        };
        let mut root = div()
            .w_full()
            .min_w_0()
            .px(theme.spacing.steps(3.5))
            .pt(px(if first { 12.0 } else { 0.0 }))
            .pb(px(if last { 12.0 } else { 0.0 }))
            .border_x_1()
            .border_color(tint.border.to_paint())
            .bg(tint.bg.to_paint())
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
            let color = tint.text;
            root = root.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(theme.spacing.steps(1.0))
                    .text_size(theme.typography.control_text)
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(color.to_paint())
                    .child(icon(asset, color))
                    .child(tag),
            );
        }
        root.child(content).into_any_element()
    }

    /// Adapters supply canonical status and reasoning copy. The row shows a bounded snippet.
    fn status_row(&self, run: &RunId) -> Option<AnyElement> {
        let theme = self.theme();
        let muted = theme.colors.text_muted.to_paint();
        let summary = self.model.run.as_ref();
        let phase = summary.map_or(Phase::Thinking, |summary| summary.phase);
        // The compaction divider is the live line while the engine compacts.
        if phase == Phase::Compacting {
            return None;
        }
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
                .tracking(theme.typography.tracking_em)
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
        // The mode can flip while the row is cached; draw with the current one.
        self.env.theme = *cx.theme();
        self.animating = false;
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
                | Row::Text {
                    slot: Slot::Plain(PlainSection::User) | Slot::Text(TextSection::User),
                    ..
                }
        );
        let position = row
            .anchor()
            .and_then(|id| self.item(id))
            .map_or(u64::MAX, |item| item.position);
        let (part, ordinal) = match row.semantic() {
            Row::Chain { .. } => (0, 0),
            Row::Text {
                slot: Slot::Prose | Slot::Text(_),
                block,
                ..
            } => (2, *block),
            Row::Projected { .. }
            | Row::Work { .. }
            | Row::EarlierWork { .. }
            | Row::WorkEntry { .. }
            | Row::Text { .. }
            | Row::Prompt(_)
            | Row::Compaction(_)
            | Row::Image(_)
            | Row::Gap
            | Row::Status(_)
            | Row::AgentResult(_)
            | Row::AgentStatus(..) => (1, 0),
        };
        let rushed = self.env.reveal.borrow().rushed(position);
        self.queue_held = self.env.reveal.borrow_mut().begin(
            key.clone(),
            (position, part, ordinal),
            !bypass && !rushed && policy == MotionPolicy::Full,
        );
        self.prose_reveal.set_paused(self.queue_held);
        self.prose_reveal.set_rushed(rushed);
        let clip = self.disclosure.clip;
        // A disclosure in flight eases the space above the row after it.
        let gap = self.disclosure.gap.unwrap_or(self.model.gap);
        let deferred_by_group =
            clip.is_none() && self.disclosure.fraction.is_none() && self.disclosure.hidden;
        let content: Option<AnyElement> = if self.queue_held || deferred_by_group {
            Some(div().into_any_element())
        } else {
            match &row {
                Row::Text { slot, .. } => Some(self.text_row(*slot, window, cx)),
                Row::Work {
                    run,
                    segment,
                    entries,
                    unloaded,
                } => self.work_row(
                    run.as_ref(),
                    *segment,
                    !entries.is_empty() || !unloaded.is_empty(),
                    cx,
                ),
                Row::EarlierWork { first, .. } => Some(
                    div()
                        .w_full()
                        .flex()
                        .justify_center()
                        .child(self.actions().button(
                            &self.theme(),
                            SharedString::from(format!("earlier-work-{first}")),
                            "Load earlier work",
                            ButtonVariant::Secondary,
                            false,
                            Action::LoadEarlierWork(*first),
                        ))
                        .into_any_element(),
                ),
                Row::Projected { row, .. } => match row.as_ref() {
                    Row::Text { slot, .. } => Some(self.text_row(*slot, window, cx)),
                    Row::Projected { .. }
                    | Row::Work { .. }
                    | Row::EarlierWork { .. }
                    | Row::WorkEntry { .. }
                    | Row::Chain { .. }
                    | Row::Prompt(_)
                    | Row::Compaction(_)
                    | Row::Image(_)
                    | Row::Gap
                    | Row::Status(_)
                    | Row::AgentResult(_)
                    | Row::AgentStatus(..) => None,
                },
                Row::WorkEntry { id, .. } => {
                    let content = self.model.items.get(id).cloned().map_or_else(
                        || div().into_any_element(),
                        |item| self.entry(&item, window, cx),
                    );
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
                Row::Prompt(id) => {
                    let card = match self.model.items.get(id).cloned() {
                        Some(item) if matches!(item.kind, ItemKind::Question { .. }) => {
                            self.requests().question_card(&item)
                        }
                        Some(item) => {
                            let card = self
                                .requests()
                                .approval_card(&item, self.answer_state(&item.id));
                            card.unwrap_or_else(|| self.entry(&item, window, cx))
                        }
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
                            &self.theme(),
                            "load-newer",
                            "Load newer messages",
                            ButtonVariant::Secondary,
                            false,
                            Action::LoadNewer,
                        ))
                        .into_any_element(),
                ),
                Row::Status(run) => self.status_row(run),
                // Expanded into its text before it is published.
                Row::AgentResult(_) => None,
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
            .text_color(theme.colors.text.to_paint());
        let moving = self.motion.end_frame();
        self.animating |= moving || clip.is_some();
        let animating = moving || self.prose_reveal.is_animating() || self.queue_held;
        if self.animating {
            window.request_animation_frame();
        }
        let frontier = self.env.reveal.borrow().frontier_key();
        self.env.reveal.borrow_mut().finish(
            &key,
            !bypass
                && !rushed
                && (self.queue_held
                    || self.motion.is_settling()
                    || self.prose_reveal.is_settling()),
        );
        if self.env.reveal.borrow().frontier_key() != frontier {
            // The conversation reveals up to the frontier, and this row may
            // render without it.
            let owner = self.env.conversation.clone();
            cx.defer(move |cx| {
                owner.update(cx, |_, cx| cx.notify()).ok();
            });
        }
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
                Row::Text { slot, .. } => slot.is_card(),
                Row::Projected { .. }
                | Row::Work { .. }
                | Row::EarlierWork { .. }
                | Row::Chain { .. }
                | Row::Prompt(_)
                | Row::Compaction(_)
                | Row::Image(_)
                | Row::Gap
                | Row::Status(_)
                | Row::AgentResult(_)
                | Row::AgentStatus(..) => false,
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
                            .top(px(-gap))
                            .bottom_0()
                            .left(theme.spacing.steps(1.75))
                            .w(px(2.0))
                            .bg(theme.colors.line.to_paint()),
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
                    gap
                }))
                .pb(below)
                .child(content),
            // Nothing to show: the row keeps only the end space.
            None => column,
        };
        let measured = Rc::clone(&self.reported_height);
        let remeasure = Rc::clone(&self.remeasure);
        let reported_body = Rc::clone(&self.reported_body);
        let clipped = clip.is_some();
        let owner = self.env.conversation.clone();
        let (body_owner, body_key) = (owner.clone(), key.clone());
        let end = px(self.model.end);
        let mut body = div()
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .debug_selector(|| format!("row:{}", row.selector()))
            .child(column.flex_shrink_0())
            .children(test_mark("animating").filter(|_| animating))
            .children(test_mark("disclosing").filter(|_| clip.is_some()))
            .on_children_prepainted(move |bounds, window, cx| {
                // The row's whole height, which a disclosure clips to.
                if let Some(bounds) = bounds.first()
                    && reported_body.replace(Some(bounds.size.height)) != Some(bounds.size.height)
                {
                    if clipped {
                        window.request_animation_frame();
                    }
                    let (owner, key, height) =
                        (body_owner.clone(), body_key.clone(), bounds.size.height);
                    cx.defer(move |cx| {
                        owner
                            .update(cx, |this, _| {
                                this.row_heights.insert(key, height);
                            })
                            .ok();
                    });
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
                    remeasure.set(false);
                    if measured
                        .replace(Some(height))
                        .is_some_and(|previous| previous != height)
                    {
                        let owner = owner.clone();
                        let key = key.clone();
                        cx.defer(move |cx| {
                            owner
                                .update(cx, |this, cx| this.row_height_changed(&key, cx))
                                .ok();
                        });
                    }
                }
            })
    }
}

/// A zero-size marker naming a state of what shows, for tests; nothing
/// outside them.
#[cfg_attr(
    not(test),
    expect(
        clippy::needless_pass_by_value,
        reason = "test builds take the name for the mark"
    )
)]
#[cfg_attr(
    test,
    expect(clippy::unnecessary_wraps, reason = "other builds return no mark")
)]
pub fn test_mark(name: impl Into<String>) -> Option<AnyElement> {
    #[cfg(test)]
    {
        let name = name.into();
        Some(div().absolute().debug_selector(|| name).into_any_element())
    }
    #[cfg(not(test))]
    {
        let _ = name;
        None
    }
}

/// Adapter reasoning is already the canonical status copy.
pub fn summary_copy(summary: &str) -> Option<String> {
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
        .tracking(theme.typography.tracking_em)
}

/// Icons in tinted text take that colour, slightly muted.
fn icon(asset: AssetId, color: Oklch) -> gpui::Svg {
    svg()
        .path(asset.as_str())
        .size(px(16.0))
        .flex_shrink_0()
        .text_color(color.at(Opacity::ToneIcon).to_paint())
}

/// Where a block sits in its message's bubble.
const fn segment_position(first: bool, last: bool) -> SegmentPosition {
    match (first, last) {
        (true, true) => SegmentPosition::Single,
        (true, false) => SegmentPosition::Head,
        (false, true) => SegmentPosition::Tail,
        (false, false) => SegmentPosition::Body,
    }
}

/// "Changed from <logo> Opus 5.5 to <logo> GPT-6 Astra": the compaction
/// divider's words when the thread moved to another engine.
fn switch_line(switch: &ModelSwitch, theme: &ArtisanTheme) -> AnyElement {
    let color = theme.colors.text_muted.to_paint();
    let model = |model: &NamedModel| {
        div()
            .flex()
            .items_center()
            .gap(px(4.0))
            .child(
                asset_glyph(editor_ui::engine_icon(theme, model.engine))
                    .size(px(14.0))
                    .flex_shrink_0(),
            )
            .child(model.name.clone())
    };
    div()
        .flex()
        .items_center()
        .gap(px(6.0))
        .whitespace_nowrap()
        .text_color(color)
        .child("Changed from")
        .child(model(&switch.from))
        .child("to")
        .child(model(&switch.to))
        .into_any_element()
}
