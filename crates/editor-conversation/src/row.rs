//! One transcript row as its own view. The conversation hands it a model
//! (a snapshot of the row's items, live text, UI toggles and run summary)
//! only when the row's version changes, so a streaming reply re-renders
//! its own row and nothing else. Render is a pure function of the model,
//! plus the prose reveal progress.
//!
//! The look is the old Editor's transcript (`conversation_surface`): the
//! 768 px turn column with a 24 px gutter, the right-hand user bubble, the
//! work section with its divided header and collapsed activity chains on a
//! rail, the one-line shimmering status, compact cards and the destructive
//! alert.

use crate::Conversation;
use crate::activity;
use crate::fold::{Row, RunSummary};
use crate::link_titles::{LinkTitles, RowTitles};
use crate::motion::{
    CompactionPhase, PhraseWord, WorkMotion, eased, header_entering, phrase_or_text,
    tabular_figures, with_entrance,
};
use crate::phase::{self, Phase};
use crate::reveal::Reveal;
use crate::thread::ItemUi;
use crate::viewport::Input;
use artisan_api::{
    Answer, ApprovalRequest, BlobHash, Decision, EngineId, FileOp, Item, ItemId, ItemKind,
    LiveTarget, NoticeKind, Output, RunId, Status, StepStatus, Timestamp, ToolKind,
};
use editor_ui::AssetId;
use editor_ui::asset_seam::asset_glyph;
use editor_ui::button::{Button, ButtonContent, ButtonSize, ButtonVariant};
use editor_ui::card::{CardStyle, compact_card, compact_card_content};
use editor_ui::gradient::vertical_gradient;
use editor_ui::inline_code_text::{
    claude_first_clause, claude_label_line, inline_runs, summary_line,
};
use editor_ui::markdown_renderer::{MarkdownBodyTone, MarkdownRenderer};
use editor_ui::motion::{MotionCurve, MotionPolicy};
use editor_ui::selectable_text::SelectableText;
use editor_ui::shimmer_text::ShimmerText;
use editor_ui::theme::{ArtisanTheme, ProseTypography, RadiusStep, RadiusTokens, SurfaceStep};
use gpui::{
    AnyElement, App, Context, Div, EntityId, FocusHandle, FontWeight, Hsla, Image,
    InteractiveElement as _, IntoElement, ObjectFit, ParentElement as _, Pixels, Render,
    SharedString, StatefulInteractiveElement as _, Styled as _, StyledImage as _, WeakEntity,
    Window, canvas, div, img, prelude::FluentBuilder as _, px, svg,
};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

/// The turn column, gutter included (`TRANSCRIPT_PROSE_WIDTH_PX`).
pub(crate) const COLUMN: f32 = 768.0;
/// The gutter either side of the column (`TRANSCRIPT_GUTTER_PX`).
pub(crate) const GUTTER: f32 = 24.0;
/// The prose measure inside the column (`TRANSCRIPT_PROSE_BODY_WIDTH_PX`).
pub(crate) const PROSE_MEASURE: f32 = 672.0;
/// One body line inside work sections and chains (`steps(6)`).
const LINE: f32 = 24.0;
/// The user bubble's widest face.
const BUBBLE_MAX: f32 = 576.0;
/// Edge of a sent image's square tile (master's `THUMBNAIL_TILE_EDGE`).
const IMAGE_TILE_EDGE: f32 = 96.0;
/// Shown live output keeps this many trailing bytes.
const LIVE_OUTPUT_TAIL: usize = 8 * 1024;
/// Diff lines shown per file.
const DIFF_LINES: usize = 400;

/// A blob shown in the transcript (user images, generated images).
#[derive(Clone)]
pub(crate) enum Blob {
    Loading,
    Ready(Arc<Image>),
    Failed(String),
}

/// What every row shares: renderer, theme, focus, and the conversation it
/// sends actions to.
pub(crate) struct Shared {
    pub markdown: Rc<MarkdownRenderer>,
    pub theme: ArtisanTheme,
    pub focus: FocusHandle,
    pub conversation: WeakEntity<Conversation>,
    /// Page titles shown for the links replies make.
    pub links: LinkTitles,
    /// Prose still revealing its words: the status line waits until it is
    /// read out, so "Thinking" never shows beside a reply still appearing.
    pub revealing: RefCell<HashSet<ItemId>>,
}

impl Shared {
    /// Records whether `id`'s prose still reveals; true when that was the
    /// last prose revealing, so the status line can show again.
    fn note_reveal(&self, id: &ItemId, settling: bool) -> bool {
        let mut revealing = self.revealing.borrow_mut();
        if settling {
            revealing.insert(id.clone());
            false
        } else {
            revealing.remove(id) && revealing.is_empty()
        }
    }
}

/// Everything one row shows.
pub(crate) struct RowModel {
    pub row: Row,
    /// The row's items and their subagent output.
    pub items: HashMap<ItemId, Item>,
    pub children: HashMap<ItemId, Vec<ItemId>>,
    pub live: HashMap<LiveTarget, String>,
    pub ui: HashMap<ItemId, ItemUi>,
    pub run: Option<RunSummary>,
    /// The thread's agents by the id messages address them by: their names.
    pub agents: HashMap<String, String>,
    pub images: HashMap<BlobHash, Blob>,
    pub now: Timestamp,
    /// Space above the row: the transcript rhythm (see `fold::gaps`).
    pub gap: f32,
    /// Space below the row: the end space under the last row, else zero.
    pub end: f32,
}

pub(crate) struct RowView {
    model: RowModel,
    version: u64,
    shared: Rc<Shared>,
    /// Reveal progress of the row's prose (reply and commentary), by item.
    reveals: HashMap<ItemId, Reveal>,
    /// Height of the last render at the current content; the row is drawn
    /// from cache at this height until its content changes.
    pub measured: Option<Pixels>,
    /// A reveal is running: draw it fresh every frame.
    pub animating: bool,
    /// This view, once rendered: the row a link title is fetched for.
    entity: Option<EntityId>,
    /// Disclosures, counting phrases, entrances and the separator.
    motion: WorkMotion,
    /// A closing continuation gives its gap back with this share left.
    gap_scale: f32,
    /// This render finished the last revealing prose: wake the status line.
    revealed: bool,
}

impl Drop for RowView {
    fn drop(&mut self) {
        let mut revealing = self.shared.revealing.borrow_mut();
        for id in self.reveals.keys() {
            revealing.remove(id);
        }
    }
}

impl RowView {
    pub(crate) fn new(model: RowModel, version: u64, shared: Rc<Shared>) -> Self {
        Self {
            model,
            version,
            shared,
            reveals: HashMap::new(),
            measured: None,
            animating: false,
            entity: None,
            motion: WorkMotion::default(),
            gap_scale: 1.0,
            revealed: false,
        }
    }

    /// Link titles for this row; with `ask`, missing ones are fetched (not
    /// while the text still streams, when a link may be half written).
    fn titles(shared: &Shared, entity: Option<EntityId>, ask: bool) -> RowTitles<'_> {
        shared.links.for_row(entity.filter(|_| ask))
    }

    pub(crate) fn version(&self) -> u64 {
        self.version
    }

    /// Same content, drawn fresh: what it shows depends on other rows.
    pub(crate) fn refresh(&mut self, cx: &mut Context<Self>) {
        self.measured = None;
        cx.notify();
    }

    /// New content: re-render (and re-measure) this row only.
    pub(crate) fn set(&mut self, model: RowModel, version: u64, cx: &mut Context<Self>) {
        self.model = model;
        self.version = version;
        self.measured = None;
        cx.notify();
    }

    fn act(
        &self,
        action: impl FnOnce(&mut Conversation, &mut Context<Conversation>) + 'static,
    ) -> impl Fn(&mut App) + 'static {
        let conversation = self.shared.conversation.clone();
        let action = std::cell::Cell::new(Some(action));
        move |cx: &mut App| {
            if let Some(action) = action.take() {
                let _ = conversation.update(cx, action);
            }
        }
    }

    fn button(
        &self,
        id: impl Into<SharedString>,
        label: &str,
        variant: ButtonVariant,
        disabled: bool,
        on_click: impl Fn(&mut Conversation, &mut Context<Conversation>) + 'static,
    ) -> AnyElement {
        let id: SharedString = id.into();
        let conversation = self.shared.conversation.clone();
        match Button::new(
            id.clone(),
            self.shared.focus.clone(),
            self.shared.theme,
            MotionPolicy::Reduced,
            variant,
            ButtonSize::Small,
            ButtonContent::text(label.to_owned()),
        ) {
            Ok(button) => button
                .disabled(disabled)
                .on_activate(move |_, _, cx| {
                    let _ = conversation.update(cx, |this, cx| on_click(this, cx));
                })
                .into_any_element(),
            Err(error) => {
                tracing::error!(button = %id, "can't build a button: {error:?}");
                div().into_any_element()
            }
        }
    }

    fn item(&self, id: &ItemId) -> Option<&Item> {
        self.model.items.get(id)
    }

    fn live_text(&self, target: &LiveTarget) -> Option<&str> {
        self.model.live.get(target).map(String::as_str)
    }

    fn ui(&self, id: &ItemId) -> Option<&ItemUi> {
        self.model.ui.get(id)
    }

    fn theme(&self) -> ArtisanTheme {
        self.shared.theme
    }

    /// A top-level block's entrance (`queued_block`): one that arrived
    /// while the thread was open opens its height and fades in.
    fn entering(&mut self, id: &ItemId, element: AnyElement) -> AnyElement {
        let fresh = self.ui(id).is_some_and(|ui| ui.fresh);
        let entrance = self.motion.entrance(id.as_str(), fresh);
        with_entrance(element, entrance, px(self.model.gap))
    }

    /// The right-aligned gradient bubble (`user_message_element`).
    fn user_row(&self, id: &ItemId) -> AnyElement {
        let Some(ItemKind::UserMessage { text, images, .. }) = self.item(id).map(|item| &item.kind)
        else {
            return div().into_any_element();
        };
        let mut message = div().w_full().flex().flex_col().items_end().gap(px(8.0));
        if !images.is_empty() {
            let mut tray = div()
                .flex()
                .flex_wrap()
                .justify_end()
                .gap(px(8.0))
                .max_w(px(BUBBLE_MAX));
            for hash in images {
                tray = tray.child(self.image_tile(hash));
            }
            message = message.child(tray);
        }
        if !text.is_empty() {
            message = message.child(self.bubble(format!("user-{id}"), text.clone()));
        }
        message.into_any_element()
    }

    /// The user message's gradient bubble around `text`.
    fn bubble(&self, key: String, text: String) -> Div {
        let theme = self.theme();
        div()
            .max_w(px(BUBBLE_MAX))
            .rounded(RadiusTokens::value(RadiusStep::X2l))
            .bg(vertical_gradient(
                SurfaceStep::S775.oklch(),
                SurfaceStep::S850.oklch(),
            ))
            .shadow(
                theme
                    .elevation
                    .card_shadow
                    .iter()
                    .map(|layer| layer.to_box_shadow())
                    .collect::<Vec<_>>(),
            )
            .px(px(16.0))
            .py(px(12.0))
            .child(
                div()
                    .w_full()
                    .min_w_0()
                    .text_size(px(ProseTypography::BODY_SIZE_PX))
                    .line_height(px(ProseTypography::BODY_LINE_PX))
                    .font_weight(ProseTypography::BODY_WEIGHT)
                    .tracking(ProseTypography::TRACKING_EM)
                    .whitespace_normal()
                    .child(SelectableText::retained(
                        SharedString::from(key),
                        text,
                        theme,
                        Vec::new(),
                    )),
            )
    }

    /// A subagent's view opens on the task it got, as the user's message
    /// opens a thread.
    fn agent_task_row(&self, id: &ItemId) -> AnyElement {
        let Some(ItemKind::Agent {
            description,
            prompt,
            ..
        }) = self.item(id).map(|item| &item.kind)
        else {
            return div().into_any_element();
        };
        let task = prompt.as_ref().unwrap_or(description).clone();
        div()
            .w_full()
            .flex()
            .flex_col()
            .items_end()
            .child(self.bubble(format!("agent-task-{id}"), task))
            .into_any_element()
    }

    /// A message the agent got while it worked, as the user's messages
    /// read in a thread.
    fn agent_message_row(&self, id: &ItemId) -> AnyElement {
        let Some(message) = self.item(id).and_then(activity::agent_message) else {
            return div().into_any_element();
        };
        let text = if message.text.is_empty() {
            message.summary.unwrap_or_default()
        } else {
            message.text
        };
        div()
            .w_full()
            .flex()
            .flex_col()
            .items_end()
            .child(self.bubble(format!("agent-message-{id}"), text))
            .into_any_element()
    }

    /// A live agent's status line, as a thread's: "Working" while a tool of
    /// its runs, "Thinking" otherwise; hidden while its prose still reveals.
    fn agent_status_row(&self, working: bool) -> Option<AnyElement> {
        if !self.shared.revealing.borrow().is_empty() {
            return None;
        }
        let theme = self.theme();
        let muted = theme.colors.muted_foreground.to_paint();
        let copy = phase::status_copy(if working {
            Phase::Working
        } else {
            Phase::Thinking
        })?;
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
                .child(
                    ShimmerText::new(copy, theme, MotionPolicy::Full)
                        .active(true)
                        .delay_seconds(1.5)
                        .duration_seconds(3.0)
                        .text_color(muted)
                        .min_w_0()
                        .truncate()
                        .into_any_element(),
                )
                .into_any_element(),
        )
    }

    /// What the subagent reported back, as the thread's reply.
    fn agent_result_row(&self, id: &ItemId) -> AnyElement {
        let Some(result) = self.item(id).and_then(crate::fold::agent_report) else {
            return div().into_any_element();
        };
        self.shared.markdown.render_source_with_tone(
            result,
            self.theme(),
            format!("agent-result-view-{id}"),
            MarkdownBodyTone::Foreground,
        )
    }

    /// One subagent in the work: its name, its task and how it's doing.
    /// It opens the subagent's own view, as a thread opens from the
    /// sidebar.
    fn agent_row(&self, item: &Item) -> AnyElement {
        let theme = self.theme();
        let ItemKind::Agent { description, .. } = &item.kind else {
            return div().into_any_element();
        };
        let muted = theme.colors.muted_foreground.to_paint();
        let state = match item.status {
            Status::Running | Status::AwaitingUser => "Working",
            Status::Done => "Done",
            Status::Failed => "Failed",
            Status::Cancelled | Status::Declined => "Stopped",
        };
        let id = item.id.clone();
        let open = self.act(move |this, cx| this.open_agent(Some(id), cx));
        div()
            .id(SharedString::from(format!("agent-{}", item.id)))
            .debug_selector(|| "work-agent".to_owned())
            .w_full()
            .min_w_0()
            .py(theme.spacing.steps(0.5))
            .flex()
            .flex_row()
            .items_center()
            .gap(theme.spacing.steps(2.0))
            .cursor_pointer()
            .text_size(px(ProseTypography::BODY_SIZE_PX))
            .line_height(px(LINE))
            .font_weight(ProseTypography::BODY_WEIGHT)
            .tracking(ProseTypography::TRACKING_EM)
            .text_color(muted)
            .child(
                asset_glyph(AssetId::TABLER_BOT_ID)
                    .size(px(16.0))
                    .flex_none(),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .text_color(theme.colors.foreground.to_paint())
                    .child(item.agent_label().unwrap_or_default()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(description.lines().next().unwrap_or_default().to_owned()),
            )
            .child(div().flex_shrink_0().child(state))
            .on_click(move |_, _, cx| open(cx))
            .into_any_element()
    }

    /// A sent image as master's thumbnail: a square tile, cropped to fill.
    fn image_tile(&self, hash: &BlobHash) -> AnyElement {
        let theme = self.theme();
        let radius = RadiusTokens::value(RadiusStep::Md);
        let tile = div()
            .relative()
            .size(px(IMAGE_TILE_EDGE))
            .flex_none()
            .overflow_hidden()
            .rounded(radius)
            .border_1()
            .border_color(theme.colors.border.to_paint())
            .bg(theme.colors.card.to_paint());
        match self.model.images.get(hash) {
            Some(Blob::Ready(image)) => {
                let full = image.clone();
                tile.id(SharedString::from(format!("user-image-{hash}")))
                    .cursor_pointer()
                    .on_click(editor_ui::image_preview::open_on_click(full))
                    .child(
                        img(image.clone())
                            .size_full()
                            .rounded(radius)
                            .object_fit(ObjectFit::Cover),
                    )
                    .into_any_element()
            }
            Some(Blob::Failed(_)) => tile
                .flex()
                .items_center()
                .justify_center()
                .px(px(8.0))
                .text_size(theme.typography.label_text)
                .text_color(theme.colors.muted_foreground.to_paint())
                .child("Couldn't load")
                .into_any_element(),
            _ => tile.into_any_element(),
        }
    }

    fn image(&self, hash: &BlobHash, height: Pixels) -> AnyElement {
        let theme = self.theme();
        match self.model.images.get(hash) {
            Some(Blob::Ready(image)) => {
                let full = image.clone();
                img(image.clone())
                    .id(SharedString::from(format!("image-{hash}")))
                    .max_h(height)
                    .max_w(px(BUBBLE_MAX))
                    .rounded(RadiusTokens::value(RadiusStep::Lg))
                    .cursor_pointer()
                    .on_click(editor_ui::image_preview::open_on_click(full))
                    .into_any_element()
            }
            Some(Blob::Failed(error)) => div()
                .text_size(theme.typography.control_text)
                .text_color(theme.colors.muted_foreground.to_paint())
                .child(format!("Couldn't load the image: {error}"))
                .into_any_element(),
            _ => div()
                .size(height)
                .rounded(RadiusTokens::value(RadiusStep::Lg))
                .bg(theme.colors.muted.to_paint())
                .into_any_element(),
        }
    }

    /// Prose (the reply, or commentary inside the work), revealed word by
    /// word while it streams. A reply begun while the thread was open
    /// reveals from its first word; one already streaming resumes at its
    /// stable end; settled prose paints whole.
    fn prose(&mut self, id: &ItemId, window: &mut Window) -> AnyElement {
        let theme = self.theme();
        let Some(item) = self.item(id) else {
            return div().into_any_element();
        };
        let ItemKind::AssistantMessage { text, .. } = &item.kind else {
            return div().into_any_element();
        };
        let streaming = item.status == Status::Running;
        let body = self
            .live_text(&LiveTarget::Text {
                item_id: id.clone(),
            })
            .filter(|live| live.len() >= text.len())
            .unwrap_or(text)
            .to_owned();
        let fresh = self.ui(id).is_some_and(|ui| ui.fresh);
        let selector = format!("prose-{id}");
        if !self.reveals.contains_key(id) {
            if fresh {
                self.reveals
                    .insert(id.clone(), Reveal::begin(Instant::now()));
            } else if streaming {
                self.reveals
                    .insert(id.clone(), Reveal::resume(&body, Instant::now()));
            }
        }
        let shared = self.shared.clone();
        let titles = Self::titles(&shared, self.entity, !streaming);
        let gated = self.motion.gate_closed();
        if gated
            && self
                .reveals
                .get(id)
                .is_some_and(|r| r.revealed().is_empty())
        {
            // Waiting in the reveal queue with nothing shown: it streams in
            // from its first word once what is above settles.
            if let Some(reveal) = self.reveals.get_mut(id) {
                reveal.wait(Instant::now());
            }
            self.animating = true;
            window.request_animation_frame();
            return div().into_any_element();
        }
        let Some(reveal) = self.reveals.get_mut(id) else {
            return shared.markdown.render_source_with_tone_and_titles(
                &body,
                theme,
                selector,
                MarkdownBodyTone::Foreground,
                &titles,
            );
        };
        let now = Instant::now();
        let animating = if gated {
            // Something above still settles: what shows stays put, and
            // frames keep coming until the queue releases it.
            reveal.wait(now);
            true
        } else {
            reveal.advance(&body, streaming, MotionPolicy::Full, now)
        };
        let settling = reveal.settling(&body, streaming, now);
        if settling {
            self.motion.note_settling();
        }
        self.revealed |= self.shared.note_reveal(id, settling);
        let (element, visible) = shared.markdown.render_revealing_source(
            reveal.revealed(),
            theme,
            selector,
            MarkdownBodyTone::Foreground,
            &titles,
            &reveal.fade,
        );
        reveal.rendered(visible);
        if animating {
            self.animating = true;
            window.request_animation_frame();
        }
        div().w_full().child(element).into_any_element()
    }

    /// One stretch of a run's work (`render_work_group`): the header with
    /// its divider on the first segment, then the rows, consecutive
    /// activities folded into trace chains, in a panel that opens and
    /// closes its measured height.
    fn work_row(
        &mut self,
        run: Option<&RunId>,
        segment: usize,
        entries: &[ItemId],
        window: &mut Window,
    ) -> Option<AnyElement> {
        let theme = self.theme();
        let summary = self.model.run.clone();
        let live = summary.as_ref().is_some_and(RunSummary::live);
        let continuation = segment > 0;
        let disclosable = run.is_some() && !entries.is_empty();
        // A live section cannot be collapsed by hand: no chevron, no toggle.
        let controlled = disclosable && !live;
        let toggle = controlled && !continuation;
        let open = !disclosable || summary.as_ref().is_none_or(|summary| summary.open);
        let selector = format!(
            "work-{}-{segment}",
            run.map_or_else(String::new, ToString::to_string)
        );
        let frame = self.motion.disclosure(&selector, open);
        // A folded continuation gives its gap back: it paints nothing, and
        // a closing one gives it back as it closes.
        if continuation {
            if !frame.content_mounted() {
                return None;
            }
            self.gap_scale = frame.fraction;
        }
        let header = (!continuation)
            .then(|| summary.as_ref().map(|summary| self.section_title(summary)))
            .flatten();
        let mut section = div().w_full().min_w_0().flex().flex_col();
        if let Some(label) = header.clone() {
            let words = self.motion.phrase(&format!("{selector}-label"), &label);
            section =
                section.child(self.work_header(label, words, toggle, frame.fraction, open, run));
        }
        let opened = self
            .motion
            .begin_disclosure(&selector, frame.content_mounted());
        let rows = frame.content_mounted().then(|| {
            let mut items = div()
                .w_full()
                .flex()
                .flex_col()
                .gap(theme.spacing.steps(2.0))
                .when(header.is_some(), |items| items.pt(theme.spacing.steps(2.0)));
            let mut index = 0;
            while index < entries.len() {
                // Tools and agents chain together ("ran a command, talked
                // to 2 agents"); an agent opens from its row in the chain.
                let chain_len = entries[index..]
                    .iter()
                    .take_while(|id| {
                        self.item(id)
                            .is_some_and(|item| activity::category(item).is_some())
                    })
                    .count();
                if chain_len > 0 {
                    let chain: Vec<ItemId> = entries[index..index + chain_len].to_vec();
                    let key = format!("chain:{}", chain[0]);
                    // Behind prose still revealing (or a block still
                    // entering) above it, the chain waits its turn; so
                    // does everything after it.
                    if self.motion.hold(&key) {
                        break;
                    }
                    let fresh = self.ui(&chain[0]).is_some_and(|ui| ui.fresh);
                    let element = self.trace_chain(&chain);
                    let entrance = self.motion.entrance(&key, fresh);
                    items = items.child(with_entrance(element, entrance, theme.spacing.steps(2.0)));
                    index += chain_len;
                    continue;
                }
                let id = entries[index].clone();
                index += 1;
                let kind = self.item(&id).map(|item| &item.kind);
                let is_prose = matches!(kind, Some(ItemKind::AssistantMessage { .. }));
                // The user's own steer shows the moment it is sent; it never
                // waits behind a reply still revealing above it.
                let is_steer = matches!(kind, Some(ItemKind::UserMessage { .. }));
                if is_prose {
                    // Prose streams in rather than entering.
                    items = items.child(self.prose(&id, window));
                } else if !is_steer && self.motion.hold(id.as_str()) {
                    break;
                } else if let Some(item) = self.item(&id) {
                    let element = self.entry(item, 0);
                    let fresh = self.ui(&id).is_some_and(|ui| ui.fresh);
                    let entrance = self.motion.entrance(id.as_str(), fresh);
                    items = items.child(with_entrance(element, entrance, theme.spacing.steps(2.0)));
                }
            }
            items.into_any_element()
        });
        self.motion.end_disclosure(opened);
        Some(frame.panel(section, rows).into_any_element())
    }

    /// A section is always titled with its run's own state, live or
    /// settled; never a generic title.
    fn section_title(&self, summary: &RunSummary) -> String {
        let end = summary.ended_at.unwrap_or(self.model.now);
        let elapsed = u64::try_from(end.saturating_sub(summary.started_at)).unwrap_or(0);
        phase::section_title(summary.phase, summary.worked, elapsed)
    }

    /// The session header row (`work_group_header_row`): muted label in
    /// tabular figures (counting like an odometer while it moves), the
    /// chevron once settled, and the 1 px divider. A header mounted while
    /// its run works enters once.
    fn work_header(
        &mut self,
        label: String,
        words: Option<Vec<PhraseWord>>,
        toggle: bool,
        fraction: f32,
        open: bool,
        run: Option<&RunId>,
    ) -> AnyElement {
        let theme = self.theme();
        let muted = theme.colors.muted_foreground.to_paint();
        let label = phrase_or_text(words, label, px(LINE));
        let near: AnyElement = if toggle {
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(theme.spacing.steps(1.0))
                .min_w_0()
                .text_color(muted)
                .child(div().min_w_0().truncate().child(label))
                .child(chevron(fraction, 16.0, muted))
                .into_any_element()
        } else {
            div()
                .min_w_0()
                .truncate()
                .text_color(muted)
                .child(label)
                .into_any_element()
        };
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
            .child(
                div()
                    .absolute()
                    .bottom(px(0.0))
                    .left(px(0.0))
                    .w_full()
                    .h(px(1.0))
                    .bg(theme.colors.border.to_paint()),
            );
        if let Some(progress) = self.motion.header_entrance() {
            header = header_entering(header, progress);
        }
        match run.cloned().filter(|_| toggle) {
            Some(run) => {
                let id = SharedString::from(format!("work-{run}"));
                let flip = self.act(move |this, cx| this.toggle_work(&run, !open, cx));
                div()
                    .id(id)
                    .min_w_0()
                    .cursor_pointer()
                    .child(header)
                    .on_click(move |_, _, cx| flip(cx))
                    .into_any_element()
            }
            None => div().min_w_0().child(header).into_any_element(),
        }
    }

    /// One run of consecutive activities (`render_trace_chain`): icon,
    /// counted clause (counting up as steps arrive) and chevron; open, the
    /// rows hang off a 2 px rail in a panel that opens its height. A chain
    /// starts closed, live and failed ones included.
    #[expect(
        clippy::too_many_lines,
        reason = "one GPUI builder composes the chain header, its rail, its rows and the accordion in visual order"
    )]
    fn trace_chain(&mut self, ids: &[ItemId]) -> AnyElement {
        let theme = self.theme();
        let Some(first) = ids.first().cloned() else {
            return div().into_any_element();
        };
        let open = self
            .ui(&first)
            .and_then(|ui| ui.chain_open)
            .unwrap_or(false);
        let selector = format!("chain-{first}");
        let frame = self.motion.disclosure(&selector, open);
        let color = if open {
            theme.colors.foreground.to_paint()
        } else {
            theme.colors.muted_foreground.to_paint()
        };
        let categories = activity::chain_categories(ids.iter().filter_map(|id| self.item(id)));
        let clause = activity::chain_clause(categories.iter().copied());
        let icon = activity::chain_icon(categories.iter().copied());
        let words = self.motion.phrase(&format!("{selector}-clause"), &clause);
        let clause = phrase_or_text(words, clause, px(LINE));
        let flip = {
            let first = first.clone();
            self.act(move |this, cx| this.toggle_chain(&first, !open, cx))
        };
        let header = div()
            .id(SharedString::from(selector.clone()))
            .flex()
            .flex_row()
            .items_center()
            .gap(theme.spacing.steps(1.0))
            .w_full()
            .min_w_0()
            .py(theme.spacing.steps(0.5))
            .cursor_pointer()
            .debug_selector(|| "work-chain".to_owned())
            .on_click(move |_, _, cx| flip(cx))
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
            .child(chevron(frame.fraction, 14.0, color));
        let opened = self
            .motion
            .begin_disclosure(&selector, frame.content_mounted());
        let rows = frame.content_mounted().then(|| {
            let mut rows = div()
                .relative()
                .w_full()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(theme.spacing.steps(1.0))
                .pl(theme.spacing.steps(6.0))
                .child(
                    div()
                        .absolute()
                        .top(px(0.0))
                        .bottom(px(0.0))
                        .left(px(0.0))
                        .w(theme.spacing.steps(4.0))
                        .child(
                            div()
                                .absolute()
                                .top(px(0.0))
                                .bottom(px(0.0))
                                .left(theme.spacing.steps(1.75))
                                .w(px(2.0))
                                .bg(theme.colors.border.with_alpha(0.6).to_paint()),
                        ),
                );
            for id in ids {
                let Some(item) = self.item(id) else {
                    continue;
                };
                let step = if matches!(item.kind, ItemKind::Agent { .. }) {
                    self.agent_row(item)
                } else {
                    self.activity_row(item, 0)
                };
                let fresh = self.ui(id).is_some_and(|ui| ui.fresh);
                let entrance = self.motion.entrance(id.as_str(), fresh);
                rows = rows.child(with_entrance(step, entrance, theme.spacing.steps(1.0)));
            }
            rows.into_any_element()
        });
        self.motion.end_disclosure(opened);
        let disclosure = div().flex().flex_col().min_w_0().child(header);
        frame.panel(disclosure, rows).into_any_element()
    }

    /// One activity row: the category label in the foreground, then the
    /// detail, mono for a shell command. Rows with output, diffs or
    /// subagent work open their details on click.
    fn activity_row(&self, item: &Item, depth: usize) -> AnyElement {
        let theme = self.theme();
        let Some(category) = activity::category(item) else {
            return self.entry(item, depth);
        };
        let message = activity::agent_message(item);
        let detail = message.as_ref().map_or_else(
            || activity::detail_text(item, category),
            |message| {
                format!(
                    "Talked to {}",
                    activity::recipient(&message.to, &self.model.agents)
                )
            },
        );
        let shell = matches!(
            item.kind,
            ItemKind::Tool {
                tool: ToolKind::Shell { .. },
                ..
            }
        );
        let detail_element = if shell {
            div()
                .min_w_0()
                .flex_1()
                .truncate()
                .font_family(theme.typography.mono.family)
                .tracking(0.0)
                .text_size(theme.typography.control_text)
                .text_color(theme.colors.muted_foreground.to_paint())
                .child(detail)
        } else {
            div()
                .min_w_0()
                .flex_1()
                .truncate()
                .text_color(theme.colors.muted_foreground.to_paint())
                .child(detail)
        };
        let has_details = self.has_details(item, message.as_ref());
        let expanded = has_details
            && self
                .ui(&item.id)
                .and_then(|ui| ui.expanded)
                .unwrap_or(false);
        let mut line = div()
            .id(SharedString::from(format!("activity-{}", item.id)))
            .debug_selector(|| "work-activity".to_owned())
            .w_full()
            .min_w_0()
            .py(theme.spacing.steps(0.5))
            .text_size(px(ProseTypography::BODY_SIZE_PX))
            .line_height(px(LINE))
            .font_weight(ProseTypography::BODY_WEIGHT)
            .tracking(ProseTypography::TRACKING_EM)
            .text_color(theme.colors.muted_foreground.to_paint())
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(theme.spacing.steps(2.0))
                    .min_w_0()
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_color(theme.colors.foreground.to_paint())
                            .child(category.label()),
                    )
                    .child(detail_element),
            );
        if has_details {
            let id = item.id.clone();
            let flip = self.act(move |this, cx| this.toggle_item(&id, !expanded, cx));
            line = line.cursor_pointer().on_click(move |_, _, cx| flip(cx));
        }
        let mut block = div().w_full().min_w_0().flex().flex_col().child(line);
        if expanded {
            block = block.child(match &item.kind {
                ItemKind::Tool { .. } if message.is_some() => {
                    self.message_details(item, message.map(|message| message.text))
                }
                ItemKind::Tool { tool, output, .. } => {
                    let streamed = self
                        .live_text(&LiveTarget::Shell {
                            item_id: item.id.clone(),
                        })
                        .filter(|_| item.status == Status::Running);
                    self.tool_details(item, tool, output.as_ref(), streamed)
                }
                _ => self.agent_details(item, depth),
            });
        }
        block.into_any_element()
    }

    /// Whether an activity row opens: on output, a diff, an agent's work or
    /// what a message said.
    fn has_details(&self, item: &Item, message: Option<&activity::AgentMessage>) -> bool {
        match &item.kind {
            // A message opens on what it said.
            ItemKind::Tool { .. } if message.is_some() => message
                .as_ref()
                .is_some_and(|message| !message.text.is_empty()),
            ItemKind::Tool { tool, output, .. } => {
                output.as_ref().is_some_and(|output| !output.is_empty())
                    || matches!(tool, ToolKind::Shell { .. })
                    || matches!(tool, ToolKind::Edit { changes } if changes.iter().any(|change| change.diff.is_some()))
                    || self
                        .live_text(&LiveTarget::Shell {
                            item_id: item.id.clone(),
                        })
                        .is_some()
            }
            ItemKind::Agent { result, .. } => {
                result.is_some() || self.model.children.contains_key(&item.id)
            }
            _ => false,
        }
    }

    /// What a message to an agent said, under its "Talked to …" row.
    fn message_details(&self, item: &Item, text: Option<String>) -> AnyElement {
        let theme = self.theme();
        div()
            .w_full()
            .min_w_0()
            .pt(theme.spacing.steps(1.0))
            .child(self.shared.markdown.render_source_with_tone(
                &text.unwrap_or_default(),
                theme,
                format!("agent-message-{}", item.id),
                MarkdownBodyTone::Muted,
            ))
            .into_any_element()
    }

    /// A subagent's own work and result, nested under its row.
    fn agent_details(&self, item: &Item, depth: usize) -> AnyElement {
        let theme = self.theme();
        let ItemKind::Agent { result, .. } = &item.kind else {
            return div().into_any_element();
        };
        let mut nested = div()
            .w_full()
            .min_w_0()
            .pt(theme.spacing.steps(1.0))
            .flex()
            .flex_col()
            .gap(theme.spacing.steps(1.0));
        if depth < 8 {
            for child in self.model.children.get(&item.id).into_iter().flatten() {
                if let Some(child) = self.item(child) {
                    nested = nested.child(self.activity_row(child, depth + 1));
                }
            }
        }
        if let Some(result) = result {
            nested = nested.child(self.shared.markdown.render_source_with_tone(
                result,
                theme,
                format!("agent-result-{}", item.id),
                MarkdownBodyTone::Muted,
            ));
        }
        nested.into_any_element()
    }

    /// A non-activity row inside the work: plan, notice, nested messages.
    fn entry(&self, item: &Item, depth: usize) -> AnyElement {
        let theme = self.theme();
        let muted = theme.colors.muted_foreground.to_paint();
        match &item.kind {
            ItemKind::AssistantMessage { text, .. } => {
                self.shared.markdown.render_source_with_tone_and_titles(
                    text,
                    theme,
                    format!("prose-{}", item.id),
                    MarkdownBodyTone::Foreground,
                    &Self::titles(&self.shared, self.entity, item.status != Status::Running),
                )
            }
            ItemKind::Tool { .. } => self.activity_row(item, depth),
            ItemKind::Agent { .. } => self.agent_row(item),
            // A steer inside the work, as the bubble it was sent as.
            ItemKind::UserMessage { .. } => self.user_row(&item.id),
            ItemKind::Plan { .. } => self.plan_entry(item),
            ItemKind::Notice { kind, text } => {
                let (asset, prefix) = match kind {
                    NoticeKind::Goal { state } => {
                        (AssetId::TABLER_TARGET_ARROW, format!("Goal {state}: "))
                    }
                    NoticeKind::Safety => (AssetId::TABLER_SHIELD_LOCK, String::new()),
                    NoticeKind::Reroute { model } => (
                        AssetId::TABLER_ARROW_RIGHT,
                        format!("Switched to {model}: "),
                    ),
                    NoticeKind::Info => (AssetId::TABLER_BELL, String::new()),
                };
                body_line(div(), &theme)
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(theme.spacing.steps(1.0))
                    .text_color(muted)
                    .child(
                        svg()
                            .path(asset.as_str())
                            .size(px(16.0))
                            .flex_shrink_0()
                            .text_color(muted),
                    )
                    .child(div().min_w_0().truncate().child(format!("{prefix}{text}")))
                    .into_any_element()
            }
            ItemKind::Question { .. } => self.question_card(item),
            ItemKind::Error { .. } => self.error_alert(item),
            ItemKind::Compaction { .. } | ItemKind::Image { .. } => div().into_any_element(),
        }
    }

    fn plan_entry(&self, item: &Item) -> AnyElement {
        let theme = self.theme();
        let muted = theme.colors.muted_foreground.to_paint();
        let ItemKind::Plan {
            steps,
            proposal,
            approval,
        } = &item.kind
        else {
            return div().into_any_element();
        };
        let mut block = body_line(div(), &theme)
            .w_full()
            .flex()
            .flex_col()
            .gap(theme.spacing.steps(0.5));
        for (index, step) in steps.iter().enumerate() {
            let (asset, color) = match step.status {
                StepStatus::Done => (AssetId::TABLER_CIRCLE_CHECK, muted),
                StepStatus::InProgress => {
                    (AssetId::TABLER_LOADER_2, theme.colors.foreground.to_paint())
                }
                StepStatus::Cancelled => (AssetId::TABLER_CIRCLE_X, muted),
                StepStatus::Pending => (AssetId::TABLER_CHECKLIST, muted),
            };
            block = block.child(
                div()
                    .id(SharedString::from(format!("plan-{}-{index}", item.id)))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(theme.spacing.steps(2.0))
                    .text_color(color)
                    .when(step.status == StepStatus::Done, gpui::Styled::line_through)
                    .child(icon(asset, color))
                    .child(step.text.clone()),
            );
        }
        if let Some(proposal) = proposal {
            block = block.child(self.shared.markdown.render_source_with_tone(
                proposal,
                theme,
                format!("plan-proposal-{}", item.id),
                MarkdownBodyTone::Foreground,
            ));
        }
        if let Some(decision) = approval.as_ref().and_then(|approval| approval.decision) {
            block = block.child(div().text_color(muted).child(decision_note(decision)));
        }
        block.into_any_element()
    }

    fn tool_details(
        &self,
        item: &Item,
        tool: &ToolKind,
        output: Option<&Output>,
        live: Option<&str>,
    ) -> AnyElement {
        let theme = self.theme();
        let mut panel = div()
            .mt(theme.spacing.steps(1.0))
            .p(px(12.0))
            .rounded(RadiusTokens::value(RadiusStep::Lg))
            .bg(theme.colors.muted.to_paint())
            .font_family(theme.typography.mono.family)
            .tracking(0.0)
            .text_size(px(12.5))
            .line_height(px(19.0))
            .flex()
            .flex_col()
            .gap(px(6.0))
            .overflow_hidden();
        match tool {
            ToolKind::Shell { command, cwd, .. } => {
                panel = panel.child(
                    div()
                        .text_color(theme.colors.foreground.to_paint())
                        .whitespace_normal()
                        .child(format!("$ {command}")),
                );
                if let Some(cwd) = cwd {
                    panel = panel.child(
                        div()
                            .text_color(theme.colors.muted_foreground.to_paint())
                            .child(cwd.clone()),
                    );
                }
            }
            ToolKind::Edit { changes } => {
                for (index, change) in changes.iter().enumerate() {
                    let op = match &change.op {
                        FileOp::Add => "Added".to_owned(),
                        FileOp::Modify => "Modified".to_owned(),
                        FileOp::Delete => "Deleted".to_owned(),
                        FileOp::Move { from } => format!("Moved from {from}"),
                    };
                    panel = panel.child(
                        div()
                            .text_color(theme.colors.foreground.to_paint())
                            .child(format!("{op} {}", change.path)),
                    );
                    if let Some(diff) = &change.diff {
                        panel =
                            panel.child(diff_lines(diff, &theme, &format!("{}-{index}", item.id)));
                    }
                }
            }
            _ => {}
        }
        let text = match (live, output) {
            (Some(live), _) => {
                let mut start = live.len().saturating_sub(LIVE_OUTPUT_TAIL);
                while !live.is_char_boundary(start) {
                    start += 1;
                }
                Some(live[start..].to_owned())
            }
            (None, Some(output)) if !output.is_empty() => Some(if output.truncated_bytes > 0 {
                format!(
                    "{}\n… {} bytes not kept …\n{}",
                    output.head, output.truncated_bytes, output.tail
                )
            } else {
                format!("{}{}", output.head, output.tail)
            }),
            _ => None,
        };
        if let Some(text) = text {
            panel = panel.child(
                div()
                    .text_color(theme.colors.muted_foreground.to_paint())
                    .child(SelectableText::retained(
                        SharedString::from(format!("output-{}", item.id)),
                        text.trim_end().to_owned(),
                        theme,
                        Vec::new(),
                    )),
            );
        }
        panel.into_any_element()
    }

    /// The compact card (`compact_card` with `card_heading` and the
    /// `body_text` details).
    fn card(&self, heading: &str, details: impl IntoElement) -> Div {
        let theme = self.theme();
        let style = CardStyle::resolve(theme);
        compact_card(style)
            .w_full()
            .child(compact_card_content(style).child(card_heading(heading, &theme)))
            .child(compact_card_content(style).child(details))
    }

    fn answer_state(&self, id: &ItemId) -> (bool, Option<String>) {
        self.ui(id)
            .map_or((false, None), |ui| (ui.sending, ui.error.clone()))
    }

    fn details_column(&self) -> Div {
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap(self.theme().spacing.steps(2.0))
    }

    fn approval_card(&self, item: &Item) -> AnyElement {
        let theme = self.theme();
        let muted = theme.colors.muted_foreground.to_paint();
        let (approval, files, proposal) = match &item.kind {
            ItemKind::Tool { approval, tool, .. } => {
                let files = match tool {
                    ToolKind::Edit { changes } => {
                        changes.iter().map(|change| change.path.clone()).collect()
                    }
                    _ => Vec::new(),
                };
                (approval.as_ref(), files, None)
            }
            ItemKind::Plan {
                approval, proposal, ..
            } => (approval.as_ref(), Vec::new(), proposal.clone()),
            _ => (None, Vec::new(), None),
        };
        let Some(approval) = approval else {
            return self.entry(item, 0);
        };
        let mut details = self.details_column();
        match &approval.request {
            ApprovalRequest::Command { command, cwd } => {
                details = details.child(
                    body_text(&format!("$ {command}"), &theme)
                        .font_family(theme.typography.mono.family)
                        .tracking(0.0),
                );
                if let Some(cwd) = cwd {
                    details =
                        details.child(body_text(&format!("in {cwd}"), &theme).text_color(muted));
                }
            }
            ApprovalRequest::FileChange => {
                details = details.child(body_text("Apply these file changes?", &theme));
                for file in files {
                    details = details.child(
                        body_text(&file, &theme)
                            .font_family(theme.typography.mono.family)
                            .tracking(0.0),
                    );
                }
            }
            ApprovalRequest::Action { description } => {
                details = details.child(body_text(description, &theme));
            }
        }
        if let Some(proposal) = &proposal {
            details = details.child(self.shared.markdown.render_source_with_tone(
                proposal,
                theme,
                format!("proposal-{}", item.id),
                MarkdownBodyTone::Foreground,
            ));
        }
        if let Some(reason) = &approval.reason {
            details = details.child(body_text(reason, &theme).text_color(muted));
        }
        let (sending, error) = self.answer_state(&item.id);
        let mut actions = div()
            .w_full()
            .flex()
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap(theme.spacing.steps(2.0));
        for (index, decision) in approval.options.iter().copied().enumerate() {
            let variant = if index == 0 {
                ButtonVariant::Default
            } else {
                ButtonVariant::Outline
            };
            let id = item.id.clone();
            actions = actions.child(self.button(
                format!("approve-{}-{index}", item.id),
                decision_label(decision),
                variant,
                sending,
                move |this, cx| this.answer(id.clone(), Answer::Approval { decision }, cx),
            ));
        }
        details = details.child(actions);
        if let Some(error) = error {
            details = details.child(body_text(&error, &theme).text_color(muted));
        }
        let heading = if proposal.is_some() {
            "Approve this plan?"
        } else {
            "Approval requested"
        };
        self.card(heading, details).into_any_element()
    }

    /// A question as the transcript's record of it
    /// (`render_question_card`): questions are answered in the composer,
    /// which extends upward with every open questionnaire, so the card
    /// shows what was asked and, once given, the answer.
    fn question_card(&self, item: &Item) -> AnyElement {
        let theme = self.theme();
        let muted = theme.colors.muted_foreground.to_paint();
        let ItemKind::Question {
            questions, answers, ..
        } = &item.kind
        else {
            return div().into_any_element();
        };
        let waiting = item.status == Status::AwaitingUser;
        let mut details = self.details_column().gap(theme.spacing.steps(2.0));
        let mut all_skipped = answers.is_some();
        for (index, question) in questions.iter().enumerate() {
            details = details.child(body_text(&question.text, &theme));
            let status = match answers
                .as_ref()
                .map(|answers| answers.get(index).cloned().flatten())
            {
                Some(Some(chosen)) => {
                    all_skipped = false;
                    format!("You answered: {}", chosen.join(", "))
                }
                Some(None) => "You skipped this question.".to_owned(),
                // One line under the last question says where to answer.
                None if waiting => {
                    if index + 1 < questions.len() {
                        continue;
                    }
                    "Answer it above the message box.".to_owned()
                }
                None => "Not answered.".to_owned(),
            };
            details = details.child(
                body_text(&status, &theme)
                    .text_color(muted)
                    .debug_selector(move || format!("question-status-{index}")),
            );
        }
        let heading = match (answers.is_some(), all_skipped) {
            (false, _) => "Question",
            (true, true) => "Skipped question",
            (true, false) => "Answered question",
        };
        self.card(heading, details).into_any_element()
    }

    /// The compaction separator (`render_compaction_separator`): a
    /// full-width rule with a centred label, "Compacting" under the shimmer
    /// while the engine compacts. Appearing live, it opens, draws its rules
    /// outward and lifts its label in; settling, the word swaps and one
    /// highlight runs from the label to both ends of the rule.
    #[expect(
        clippy::too_many_lines,
        reason = "one GPUI builder composes the label slot, its swap and both drawing rules"
    )]
    fn compaction_row(&mut self, id: &ItemId) -> AnyElement {
        /// The checkpoint highlight's length along a rule.
        const PULSE_LENGTH_PX: f32 = 56.0;
        /// The highlight's peak opacity over the rule.
        const PULSE_PEAK_ALPHA: f32 = 0.55;
        let theme = self.theme();
        let Some(item) = self.item(id) else {
            return div().into_any_element();
        };
        let phase = match item.status {
            Status::Running => CompactionPhase::Running,
            Status::Done => CompactionPhase::Done,
            _ => CompactionPhase::Stopped,
        };
        let fresh = self.ui(id).is_some_and(|ui| ui.fresh);
        let frame = self.motion.compaction(id.as_str(), phase, fresh);
        let muted = theme.colors.muted_foreground.to_paint();
        let rule_color = theme.colors.border.to_paint();
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
                    ShimmerText::new(phase.label(), theme, MotionPolicy::Full)
                        .active(phase == CompactionPhase::Running)
                        .delay_seconds(0.0)
                        .duration_seconds(2.0)
                        .text_color(muted),
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

    /// The reference's single destructive card (`render_error`): circle-x,
    /// "Error", the message in muted text.
    fn error_alert(&self, item: &Item) -> AnyElement {
        let theme = self.theme();
        let ItemKind::Error { message, .. } = &item.kind else {
            return div().into_any_element();
        };
        let title = "Error";
        let destructive = theme.colors.destructive.to_paint();
        div()
            .relative()
            .flex()
            .flex_col()
            .w_full()
            .gap(theme.spacing.steps(1.5))
            .px(theme.spacing.steps(3.5))
            .py(theme.spacing.steps(3.0))
            .rounded(RadiusTokens::value(RadiusStep::Xl))
            .border_1()
            .border_color(theme.colors.destructive.with_alpha(0.25).to_paint())
            .bg(theme.colors.destructive.with_alpha(0.05).to_paint())
            .text_color(destructive)
            .text_size(theme.typography.control_text)
            .overflow_hidden()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_start()
                    .gap(theme.spacing.steps(2.0))
                    .w_full()
                    .child(
                        div().flex_shrink_0().mt(px(2.0)).child(
                            svg()
                                .path(AssetId::TABLER_CIRCLE_X.as_str())
                                .size(theme.spacing.steps(4.0))
                                .text_color(destructive),
                        ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(theme.spacing.steps(1.5))
                            .min_w_0()
                            .flex_1()
                            .child(div().min_w_0().font_weight(FontWeight::MEDIUM).child(title))
                            .child(
                                div()
                                    .min_w_0()
                                    .text_color(theme.colors.muted_foreground.to_paint())
                                    .child(SelectableText::retained(
                                        SharedString::from(format!("error-{}", item.id)),
                                        message.clone(),
                                        theme,
                                        Vec::new(),
                                    )),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn image_row(&self, id: &ItemId) -> AnyElement {
        let theme = self.theme();
        let Some(ItemKind::Image { blob, caption }) = self.item(id).map(|item| &item.kind) else {
            return div().into_any_element();
        };
        div()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(self.image(blob, px(480.0)))
            .when_some(caption.clone(), |this, caption| {
                this.child(
                    div()
                        .text_size(theme.typography.control_text)
                        .text_color(theme.colors.muted_foreground.to_paint())
                        .child(caption),
                )
            })
            .into_any_element()
    }

    /// The live line (`render_status`): the reasoning summary reduced to
    /// one line by the engine's policy, else what the run is doing, as one
    /// shimmering muted line. Stop lives in the composer.
    fn status_row(&self, run: &RunId) -> Option<AnyElement> {
        if !self.shared.revealing.borrow().is_empty() {
            return None;
        }
        let theme = self.theme();
        let muted = theme.colors.muted_foreground.to_paint();
        let summary = self.model.run.as_ref();
        let phase = summary.map_or(Phase::Thinking, |summary| summary.phase);
        let engine = summary.and_then(|summary| summary.engine);
        let reduced = self
            .live_text(&LiveTarget::Reasoning {
                run_id: run.clone(),
            })
            .filter(|_| phase == Phase::Thinking)
            .and_then(|reasoning| summary_copy(reasoning, engine));
        let live = summary.is_none_or(RunSummary::live);
        let content: AnyElement = if let Some(copy) = reduced {
            let runs = inline_runs(&copy, theme);
            ShimmerText::new(runs.text, theme, MotionPolicy::Full)
                .text_runs(
                    format!("status-summary-{run}"),
                    runs.highlights,
                    runs.overrides,
                )
                .active(live)
                .delay_seconds(0.0)
                .duration_seconds(2.0)
                .text_color(muted)
                .min_w_0()
                .truncate()
                .into_any_element()
        } else {
            let copy = self
                .live_text(&LiveTarget::Status {
                    run_id: run.clone(),
                })
                .filter(|text| !text.trim().is_empty())
                .map(str::to_owned)
                .or_else(|| phase::status_copy(phase).map(str::to_owned))?;
            ShimmerText::new(copy, theme, MotionPolicy::Full)
                .active(live)
                .delay_seconds(1.5)
                .duration_seconds(3.0)
                .text_color(muted)
                .min_w_0()
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
        self.entity = Some(cx.entity_id());
        self.gap_scale = 1.0;
        let policy = if cx.reduce_motion() {
            MotionPolicy::Reduced
        } else {
            MotionPolicy::Full
        };
        let live = self.model.run.as_ref().is_some_and(RunSummary::live);
        self.motion.begin_frame(live, policy);
        let row = self.model.row.clone();
        let content: Option<AnyElement> = match &row {
            Row::User(id) => Some(self.user_row(id)),
            Row::Reply(id) => Some(self.prose(id, window)),
            Row::Work {
                run,
                segment,
                entries,
            } => self.work_row(run.as_ref(), *segment, entries, window),
            Row::Prompt(id) => {
                let card = match self.item(id) {
                    Some(item) if matches!(item.kind, ItemKind::Question { .. }) => {
                        self.question_card(item)
                    }
                    Some(item) => self.approval_card(item),
                    None => div().into_any_element(),
                };
                Some(self.entering(id, card))
            }
            Row::Compaction(id) => Some(self.compaction_row(id)),
            Row::Error(id) => {
                let alert = self
                    .item(id)
                    .map_or_else(|| div().into_any_element(), |item| self.error_alert(item));
                Some(self.entering(id, alert))
            }
            Row::Image(id) => {
                let image = self.image_row(id);
                Some(self.entering(id, image))
            }
            Row::Gap => Some(
                div()
                    .w_full()
                    .flex()
                    .justify_center()
                    .child(self.button(
                        "load-newer",
                        "Load newer messages",
                        ButtonVariant::Secondary,
                        false,
                        |this, cx| {
                            this.viewport(vec![Input::BottomReached], cx);
                        },
                    ))
                    .into_any_element(),
            ),
            Row::Status(run) => self.status_row(run),
            Row::AgentTask(id) => Some(self.agent_task_row(id)),
            Row::AgentResult(id) => Some(self.agent_result_row(id)),
            Row::AgentMessage(id) => Some(self.agent_message_row(id)),
            Row::AgentStatus(_, working) => self.agent_status_row(*working),
        };
        if std::mem::take(&mut self.revealed) {
            let conversation = self.shared.conversation.clone();
            cx.defer(move |cx| {
                let _ = conversation.update(cx, Conversation::wake_status);
            });
        }
        if self.shared.links.has_queued() {
            let conversation = self.shared.conversation.clone();
            cx.defer(move |cx| {
                let _ = conversation.update(cx, Conversation::fetch_link_titles);
            });
        }
        // Measures the content so the next frames can draw this row from cache.
        let row_view = cx.entity().downgrade();
        let measure = canvas(
            move |bounds, window, cx| {
                let changed = row_view
                    .update(cx, |row, _| match row.measured {
                        None => {
                            row.measured = Some(bounds.size.height);
                            false
                        }
                        Some(height) if height != bounds.size.height => {
                            row.measured = None;
                            true
                        }
                        Some(_) => false,
                    })
                    .unwrap_or(false);
                if changed {
                    // The content outgrew its cached height: draw it fresh next frame.
                    window.request_animation_frame();
                }
            },
            |_, (), _, _| {},
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();
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
        if self.motion.end_frame() {
            self.animating = true;
            window.request_animation_frame();
        }
        let column = match content {
            Some(content) => column
                .pt(px(self.model.gap * self.gap_scale))
                .pb(below + px(self.model.end))
                .child(content),
            // Nothing to show: the row keeps only the end space.
            None => column.pb(px(self.model.end)),
        };
        div().relative().w_full().child(column).child(measure)
    }
}

/// Reduces a raw reasoning summary to its one thinking line: Claude's
/// first meaningful line trimmed to its first clause, every other engine's
/// newest finished sentence (`SummaryLinePolicy`).
pub(crate) fn summary_copy(summary: &str, engine: Option<EngineId>) -> Option<String> {
    match engine {
        Some(EngineId::Claude) => {
            claude_label_line(summary).map(|label| claude_first_clause(&label))
        }
        _ => summary_line(summary),
    }
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

/// A card heading: control text, medium weight.
fn card_heading(title: &str, theme: &ArtisanTheme) -> Div {
    div()
        .w_full()
        .min_w_0()
        .text_size(theme.typography.control_text)
        .font_weight(FontWeight::MEDIUM)
        .child(title.to_owned())
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

fn decision_label(decision: Decision) -> &'static str {
    match decision {
        Decision::AllowOnce => "Approve",
        Decision::AllowAlways => "Always allow",
        Decision::DenyOnce => "Deny",
        Decision::DenyAlways => "Always deny",
        Decision::Cancelled => "Cancel",
    }
}

fn decision_note(decision: Decision) -> &'static str {
    match decision {
        Decision::AllowOnce => "You approved this request.",
        Decision::AllowAlways => "You allowed this from now on.",
        Decision::DenyOnce => "You denied this request.",
        Decision::DenyAlways => "You denied this from now on.",
        Decision::Cancelled => "Cancelled.",
    }
}

fn diff_lines(diff: &str, theme: &ArtisanTheme, id: &str) -> AnyElement {
    let mut block = div()
        .id(SharedString::from(format!("diff-{id}")))
        .flex()
        .flex_col();
    let total = diff.lines().count();
    for line in diff.lines().take(DIFF_LINES) {
        let color = if line.starts_with('+') && !line.starts_with("+++") {
            theme.colors.banner_success.to_paint()
        } else if line.starts_with('-') && !line.starts_with("---") {
            theme.colors.destructive.to_paint()
        } else {
            theme.colors.muted_foreground.to_paint()
        };
        block = block.child(
            div()
                .text_color(color)
                .whitespace_nowrap()
                .child(line.to_owned()),
        );
    }
    if total > DIFF_LINES {
        block = block.child(
            div()
                .text_color(theme.colors.muted_foreground.to_paint())
                .child(format!("… {} more lines", total - DIFF_LINES)),
        );
    }
    block.into_any_element()
}
