//! The open thread. One copy of its items, a viewport, and a pure render.
//!
//! Items come from `GET /threads/{id}/items` pages and the host's event
//! stream and are replaced by id; live text is copied from the host. The
//! transcript is a fold of items + live text + UI toggles into rows, shown
//! in a GPUI list.

mod activity;
mod fold;
mod follow;
mod link_titles;
mod motion;
mod phase;
mod render;
mod reveal;
mod row;
mod thread;
mod viewport;

#[cfg(test)]
mod tests;

pub use phase::{Phase, turn_phase};

use artisan_api::{Answer, BlobHash, Item, ItemId, ItemKind, Run, Status, ThreadId};
use editor_composer::{
    Composer, ComposerEvent, ComposerGoal, ComposerGoalState, ComposerQuestionnaire, ComposerTarget,
};
use editor_core::{Core, CoreEvent, HostData, HostEvent, HostId, Screen};
use editor_ui::markdown_renderer::MarkdownRenderer;
use editor_ui::picker_motion::SlidingHoverState;
use editor_ui::theme::{ArtisanTheme, ThemeMode};
use editor_ui::wheel_glide::WheelGlide;
use fold::{Row, RowKey};
use gpui::{
    App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Image, ListAlignment,
    ListOffset, ListState, ScrollWheelEvent, Subscription, Task, Window, px,
};
use row::{Blob, RowModel, RowView, Shared};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hash as _, Hasher as _};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;
use thread::ThreadView;
use viewport::{Effect, Fetch, Input, Jump};

/// Older items load while less transcript than this sits above the view.
/// Counted in rows it read a whole thread: a turn's work is one row, so a
/// long thread has only a handful.
const READ_AHEAD_PX: f32 = 1200.0;
/// The list lays out (and so measures) rows this far beyond the view. It
/// must reach past the read-ahead: a row not laid out measures zero, and
/// read-ahead would never be satisfied.
const OVERDRAW_PX: f32 = 1600.0;

/// Whether the view is close enough to the oldest loaded row to read on.
pub(crate) fn wants_older(list: &ListState) -> bool {
    let above = -f32::from(list.scroll_px_offset_for_scrollbar().y);
    f32::from(list.viewport_bounds().size.height) <= 0.0 || above < READ_AHEAD_PX
}

/// A thread opening behind the wordmark until its transcript stops moving.
struct Landing {
    since: std::time::Instant,
    /// What the list looked like last frame: items, rows, its height.
    last: Option<(usize, usize, u32)>,
    /// Frames in a row it hasn't changed.
    steady: u8,
}

/// A jump's transcript starts this far short of its target…
const ARRIVAL_PX: f32 = 160.0;
/// …and eases into place over this long.
const ARRIVAL: Duration = Duration::from_millis(180);

/// Frames the transcript must hold still before it shows.
const LANDING_STEADY_FRAMES: u8 = 3;
/// It shows after this at the latest, still or not.
const LANDING_CAP: Duration = Duration::from_secs(3);

#[derive(Clone, Debug)]
pub enum ConversationEvent {
    /// The open thread's items changed (the inspector re-reads them).
    ItemsChanged,
    /// The composer's project menu asked for a new project.
    NewProject,
    /// A subagent's view opened or closed (the title shows its name).
    AgentChanged,
}

pub struct Conversation {
    core: Entity<Core>,
    /// The subagent whose view is open in place of the thread's.
    agent: Option<ItemId>,
    open: Option<(HostId, ThreadId)>,
    view: Option<ThreadView>,
    /// The folded rows and, in the same order, their views.
    rows: Vec<(Row, u64)>,
    row_views: Vec<Entity<RowView>>,
    list: ListState,
    composer: Option<Entity<Composer>>,
    composer_events: Option<Subscription>,
    shared: Rc<Shared>,
    blobs: HashMap<BlobHash, Blob>,
    theme: ArtisanTheme,
    focus: FocusHandle,
    /// Why the last page failed to load.
    load_error: Option<String>,
    /// Height of the composer card floating over the transcript; the end
    /// space under the last row clears it.
    composer_height: f32,
    /// The pointer is over the turn navigator (rail or menu).
    navigator_hovered: bool,
    /// The navigator menu's shared hover pill and the list it measures in.
    navigator_hover: Rc<RefCell<SlidingHoverState>>,
    navigator_hover_surface: Rc<RefCell<Option<gpui::Bounds<gpui::Pixels>>>>,
    /// Per tick (by message), whether it was last active and when that
    /// changed, while it eases between its two looks.
    navigator_ticks: HashMap<ItemId, (bool, Option<std::time::Instant>)>,
    /// Whether a tick was active last frame: the first one placed does not
    /// ease in.
    navigator_had_active: bool,
    /// Eases the list to its end while the reader follows the tail.
    follow: Rc<RefCell<follow::Follow>>,
    /// The open thread is landing: hidden under the wordmark until settled.
    landing: Option<Landing>,
    /// When this frame's render began; its paint reports a slow frame.
    frame_started: Rc<std::cell::Cell<Option<std::time::Instant>>>,
    /// Wheel ticks gliding the transcript, as master's did.
    glide: WheelGlide,
    glide_scheduled: bool,
    /// A jump's arrival: when it began and how far short it started.
    arrival: Option<(std::time::Instant, f32)>,
    /// Ticks the elapsed time while the open thread's run is live.
    clock: Option<Task<()>>,
    host_events: Option<Subscription>,
    _core_events: Subscription,
}

impl EventEmitter<ConversationEvent> for Conversation {}

impl Conversation {
    pub fn new(core: Entity<Core>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let core_events =
            cx.subscribe_in(&core, window, |this, _, event: &CoreEvent, window, cx| {
                if matches!(event, CoreEvent::ScreenChanged | CoreEvent::HostsChanged) {
                    this.sync_screen(window, cx);
                }
            });
        // Following is ours (`follow`), eased; GPUI's tail mode snaps.
        let list = ListState::new(0, ListAlignment::Top, px(OVERDRAW_PX));
        let this = cx.entity().downgrade();
        list.set_scroll_handler(move |event, _, cx| {
            // The list is still borrowed here; `scrolled` reads it.
            let (this, top) = (this.clone(), event.visible_range.start);
            cx.defer(move |cx| {
                let _ = this.update(cx, |this, cx| this.scrolled(top, cx));
            });
        });
        let theme = ArtisanTheme::for_mode(ThemeMode::Dark);
        let focus = cx.focus_handle();
        let shared = Rc::new(Shared {
            markdown: Rc::new(MarkdownRenderer::new().with_prose_measure(px(row::PROSE_MEASURE))),
            theme,
            focus: focus.clone(),
            conversation: cx.entity().downgrade(),
            links: link_titles::LinkTitles::default(),
            revealing: RefCell::default(),
        });
        let mut conversation = Self {
            core,
            agent: None,
            open: None,
            view: None,
            rows: Vec::new(),
            row_views: Vec::new(),
            list,
            composer: None,
            composer_events: None,
            shared,
            blobs: HashMap::new(),
            theme,
            focus,
            load_error: None,
            composer_height: 0.0,
            navigator_hovered: false,
            navigator_hover: Rc::default(),
            navigator_hover_surface: Rc::default(),
            navigator_ticks: HashMap::new(),
            navigator_had_active: false,
            follow: Rc::default(),
            landing: None,
            frame_started: Rc::default(),
            glide: WheelGlide::default(),
            glide_scheduled: false,
            arrival: None,
            clock: None,
            host_events: None,
            _core_events: core_events,
        };
        conversation.sync_screen(window, cx);
        conversation
    }

    /// The thread on screen, if any.
    #[must_use]
    pub fn open_thread(&self) -> Option<&(HostId, ThreadId)> {
        self.open.as_ref()
    }

    /// The loaded items of the open thread, in order.
    pub fn items(&self) -> impl Iterator<Item = &Item> {
        self.view.iter().flat_map(|view| view.items.values())
    }

    #[must_use]
    pub fn core(&self) -> &Entity<Core> {
        &self.core
    }

    /// Scrolls to an item of the open thread, loading history until it is in.
    pub fn jump_to(&mut self, item: &Item, cx: &mut Context<Self>) {
        let Some(view) = self
            .view
            .as_mut()
            .filter(|view| view.thread == item.thread_id)
        else {
            return;
        };
        let effects = view.jump(Jump::To(item.position));
        self.perform(effects, cx);
    }

    /// How far the transcript is drawn from its place while a jump arrives;
    /// zero once it has.
    pub(crate) fn arrival_offset(&mut self) -> f32 {
        let Some((since, shift)) = self.arrival else {
            return 0.0;
        };
        let progress = since.elapsed().as_secs_f32() / ARRIVAL.as_secs_f32();
        if progress >= 1.0 {
            self.arrival = None;
            return 0.0;
        }
        shift * (1.0 - motion::eased(editor_ui::motion::MotionCurve::SmoothOut, progress))
    }

    /// A thread opening, before its transcript is in place (master's
    /// landing). It stays hidden under the wordmark until it stops moving:
    /// its pages are in (back to the turn's start, and read-ahead), the
    /// glide to its end is done, no row is still entering, and the list has
    /// kept its size for a few frames. Pages landing above, rows measuring
    /// and images arriving all move it, so showing it any earlier flickered.
    pub(crate) fn landing(&self) -> bool {
        self.agent.is_none() && self.landing.is_some()
    }

    /// Advances the landing by one frame; true while it goes on.
    pub(crate) fn advance_landing(&mut self, cx: &App) -> bool {
        let Some(landing) = self.landing.as_mut() else {
            return false;
        };
        let Some(view) = self.view.as_ref() else {
            self.landing = None;
            return false;
        };
        let window = view.window();
        let loading = window.in_flight
            || view.needs_turn_start()
            || (!window.exhausted && wants_older(&self.list) && self.load_error.is_none());
        let moving =
            !self.follow.borrow().idle() || self.row_views.iter().any(|row| row.read(cx).animating);
        // A live reply keeps growing the list; its pages and rows settling is
        // what counts.
        let live = view
            .live
            .keys()
            .any(|target| matches!(target, artisan_api::LiveTarget::Text { .. }));
        let height = if live {
            0
        } else {
            f32::from(self.list.max_offset_for_scrollbar().y)
                .round()
                .to_bits()
        };
        let now = (view.items.len(), self.rows.len(), height);
        if loading || moving || landing.last != Some(now) {
            landing.steady = 0;
        } else {
            landing.steady += 1;
        }
        landing.last = Some(now);
        let done = landing.steady >= LANDING_STEADY_FRAMES
            || landing.since.elapsed() >= LANDING_CAP
            || self.load_error.is_some();
        if done {
            tracing::info!(
                thread = %view.thread,
                ms = landing.since.elapsed().as_millis(),
                items = view.items.len(),
                rows = self.rows.len(),
                settled = landing.steady >= LANDING_STEADY_FRAMES,
                "thread opened"
            );
            self.landing = None;
        }
        !done
    }

    /// Shows the subagent `agent`'s own work in place of the thread's, like
    /// opening its thread; `None` goes back to the thread.
    pub fn open_agent(&mut self, agent: Option<ItemId>, cx: &mut Context<Self>) {
        if self.agent == agent {
            return;
        }
        self.agent = agent;
        // Another transcript: build it fresh and land on its end.
        self.rows.clear();
        self.row_views.clear();
        self.list.reset(0);
        self.glide.cancel();
        self.follow.borrow_mut().snap_next();
        self.rebuild(cx);
        if self.follow.borrow_mut().take_snap() {
            self.list.scroll_to_end();
        }
        cx.emit(ConversationEvent::AgentChanged);
        cx.emit(ConversationEvent::ItemsChanged);
        cx.notify();
    }

    /// The open subagent and the agents above it, outermost first, each
    /// with its name: the title's trail.
    #[must_use]
    pub fn agent_trail(&self) -> Vec<(ItemId, String)> {
        let (Some(view), Some(agent)) = (self.view.as_ref(), self.agent.as_ref()) else {
            return Vec::new();
        };
        let mut trail = Vec::new();
        let mut next = Some(agent);
        while let Some(id) = next.take() {
            let Some(item) = view.item(id) else { break };
            if let Some(label) = item.agent_label() {
                trail.push((id.clone(), label));
            }
            next = item.parent_id.as_ref();
            if trail.len() > 16 {
                break;
            }
        }
        trail.reverse();
        trail
    }

    /// Back to the live tail.
    pub fn jump_to_latest(&mut self, cx: &mut Context<Self>) {
        let Some(view) = self.view.as_mut() else {
            return;
        };
        let effects = view.jump(Jump::Latest);
        self.perform(effects, cx);
    }

    fn host(&self, cx: &App) -> Option<Entity<HostData>> {
        let (host, _) = self.open.as_ref()?;
        self.core.read(cx).host(host).cloned()
    }

    fn sync_screen(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let screen = self.core.read(cx).screen().clone();
        let target = match screen {
            Screen::Thread { host, thread } => {
                if self.open.as_ref() != Some(&(host.clone(), thread.clone())) {
                    self.open_thread_on(&host, &thread, cx);
                }
                ComposerTarget::Thread { host, thread }
            }
            Screen::NewThread { host, project } => {
                self.close(cx);
                ComposerTarget::NewThread { host, project }
            }
            Screen::Home | Screen::Settings(_) | Screen::Marketplace => return,
        };
        if let Some(composer) = &self.composer {
            if composer.read(cx).target() != &target {
                composer.update(cx, |composer, cx| composer.set_target(target, window, cx));
            }
        } else {
            let core = self.core.clone();
            let composer = cx.new(|cx| Composer::new(core, target, window, cx));
            self.composer_events = Some(cx.subscribe(
                &composer,
                |this, _, event: &ComposerEvent, cx| {
                    match event {
                        ComposerEvent::NewProject => cx.emit(ConversationEvent::NewProject),
                        // Sending brings the reader back to the end, where
                        // their message and the reply land (master).
                        ComposerEvent::Sent { thread, .. } => {
                            if this.open.as_ref().is_some_and(|(_, open)| open == thread) {
                                this.jump_to_latest(cx);
                            }
                        }
                    }
                },
            ));
            self.composer = Some(composer);
        }
        cx.notify();
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        if self.agent.take().is_some() {
            cx.emit(ConversationEvent::AgentChanged);
        }
        if self.open.take().is_some() {
            self.view = None;
            self.host_events = None;
            self.clock = None;
            self.rebuild(cx);
            cx.emit(ConversationEvent::ItemsChanged);
        }
    }

    fn open_thread_on(&mut self, host_id: &HostId, thread: &ThreadId, cx: &mut Context<Self>) {
        self.close(cx);
        let Some(host) = self.core.read(cx).host(host_id).cloned() else {
            tracing::warn!(host = %host_id, thread = %thread, "can't open a thread on an unknown host");
            return;
        };
        self.host_events = Some(cx.subscribe(&host, |this, host, event, cx| {
            this.host_event(&host, event, cx);
        }));
        self.open = Some((host_id.clone(), thread.clone()));
        self.load_error = None;
        let (mut view, effects) = ThreadView::open(thread.clone());
        view.set_live(host.read(cx).live_targets(thread));
        self.view = Some(view);
        self.list.reset(0);
        self.follow.borrow_mut().snap_next();
        self.landing = Some(Landing {
            since: std::time::Instant::now(),
            last: None,
            steady: 0,
        });
        self.rows.clear();
        self.row_views.clear();
        self.perform(effects, cx);
        self.rebuild(cx);
        cx.emit(ConversationEvent::ItemsChanged);
    }

    fn host_event(&mut self, host: &Entity<HostData>, event: &HostEvent, cx: &mut Context<Self>) {
        let Some(view) = self.view.as_mut() else {
            return;
        };
        let thread = view.thread.clone();
        match event {
            HostEvent::Item(item) => {
                if item.thread_id != thread {
                    return;
                }
                let changed = view.apply_item((**item).clone());
                view.set_live(host.read(cx).live_targets(&thread));
                if changed {
                    self.rebuild(cx);
                    cx.emit(ConversationEvent::ItemsChanged);
                }
            }
            HostEvent::Live { thread_id, .. } => {
                if *thread_id == thread && view.set_live(host.read(cx).live_targets(&thread)) {
                    self.rebuild(cx);
                }
            }
            HostEvent::Changed => {
                view.set_live(host.read(cx).live_targets(&thread));
                self.rebuild(cx);
            }
            HostEvent::Reset => {
                self.follow.borrow_mut().snap_next();
                let effects = view.reset();
                view.set_live(host.read(cx).live_targets(&thread));
                self.perform(effects, cx);
                self.rebuild(cx);
                cx.emit(ConversationEvent::ItemsChanged);
            }
        }
    }

    fn perform(&mut self, effects: Vec<Effect>, cx: &mut Context<Self>) {
        for effect in effects {
            match effect {
                Effect::Fetch(fetch) => self.fetch(fetch, cx),
                Effect::ScrollToEnd => {
                    self.glide.cancel();
                    // A thread opening lands on its end; anything else
                    // glides there (`follow`, now that the reader follows).
                    if self.follow.borrow_mut().take_snap() {
                        self.list.scroll_to_end();
                    }
                }
                Effect::ScrollTo(position) => {
                    let index = self.view.as_ref().and_then(|view| {
                        self.rows.iter().position(|(row, _)| {
                            row.anchor()
                                .and_then(|id| view.position_of(id))
                                .is_some_and(|start| start >= position)
                        })
                    });
                    if let Some(item_ix) = index {
                        self.glide.cancel();
                        self.follow.borrow_mut().stop();
                        let from = self.list.logical_scroll_top().item_ix;
                        self.list.scroll_to(ListOffset {
                            item_ix,
                            offset_in_item: px(0.0),
                        });
                        // The transcript is drawn a little short, on the side
                        // it came from, and eases in: a jump reads as a quick
                        // scroll rather than a cut.
                        if !cx.reduce_motion() && item_ix != from {
                            let shift = if item_ix < from {
                                -ARRIVAL_PX
                            } else {
                                ARRIVAL_PX
                            };
                            self.arrival = Some((std::time::Instant::now(), shift));
                        }
                    }
                }
            }
        }
        cx.notify();
    }

    fn fetch(&mut self, fetch: Fetch, cx: &mut Context<Self>) {
        let (Some(host), Some(view)) = (self.host(cx), self.view.as_ref()) else {
            return;
        };
        let client = host.read(cx).client.clone();
        let thread = view.thread.clone();
        // Reading back to a turn's start takes the biggest pages.
        let limit = if view.needs_turn_start() {
            thread::SEARCH_PAGE
        } else {
            thread::PAGE
        };
        let query = thread::query(fetch, limit);
        cx.spawn(async move |this, cx| {
            let result = client.items(&thread, &query).await;
            let _ = this.update(cx, |this, cx| {
                let Some(view) = this.view.as_mut().filter(|view| view.thread == thread) else {
                    return;
                };
                let effects = match result {
                    Ok(page) => {
                        this.load_error = None;
                        view.apply_page(fetch, page)
                    }
                    Err(error) => {
                        tracing::warn!(thread = %thread, ?fetch, "couldn't load items: {error:#}");
                        this.load_error = Some(format!("Couldn't load this thread: {error:#}"));
                        view.page_failed()
                    }
                };
                this.rebuild(cx);
                this.perform(effects, cx);
                // Only part of a turn is in: keep reading back to its start
                // (see `ThreadView::shown`).
                if this.view.as_ref().is_some_and(ThreadView::needs_turn_start) {
                    this.viewport(vec![Input::TopReached], cx);
                }
                cx.emit(ConversationEvent::ItemsChanged);
            });
        })
        .detach();
    }

    /// Glides a wheel tick over the transcript, as master's did. Trackpad
    /// deltas and reduced motion go to the list, which moves directly.
    pub(crate) fn transcript_wheel(
        &mut self,
        event: &ScrollWheelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let max = f32::from(self.list.max_offset_for_scrollbar().y).max(0.0);
        if max <= 0.0 {
            return;
        }
        if event.delta.precise() || cx.reduce_motion() {
            self.glide.cancel();
            return;
        }
        let delta = f32::from(event.delta.pixel_delta(window.line_height()).y);
        if delta.abs() <= f32::EPSILON {
            return;
        }
        cx.stop_propagation();
        self.glide.push(self.list_offset(max), delta, max);
        self.schedule_glide(window, cx);
    }

    /// The list's offset in GPUI's `[-max, 0]`; following the tail sits
    /// past the end, which is `-max`.
    fn list_offset(&self, max: f32) -> f32 {
        f32::from(self.list.scroll_px_offset_for_scrollbar().y).clamp(-max, 0.0)
    }

    fn schedule_glide(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.glide.active() || self.glide_scheduled {
            return;
        }
        self.glide_scheduled = true;
        cx.on_next_frame(window, |this, window, cx| {
            this.glide_scheduled = false;
            let max = f32::from(this.list.max_offset_for_scrollbar().y).max(0.0);
            let current = this.list_offset(max);
            if let Some(next) = this.glide.step(current, max) {
                this.list.scroll_by(px(current - next));
                // `scroll_by` skips the list's scroll handler: tell the
                // viewport what the wheel would have.
                let top = this.list.logical_scroll_top().item_ix;
                this.scrolled(top, cx);
                cx.notify();
            }
            this.schedule_glide(window, cx);
        });
    }

    /// The reader scrolled; `top` is the first visible row. Only the very
    /// end counts as the bottom: a tall last row in view isn't, or following
    /// would pull a reader scrolling up through it back down.
    fn scrolled(&mut self, top: usize, cx: &mut Context<Self>) {
        let max = f32::from(self.list.max_offset_for_scrollbar().y).max(0.0);
        let at_end = -f32::from(self.list.scroll_px_offset_for_scrollbar().y) >= max - 1.0;
        let Some(view) = self.view.as_mut() else {
            return;
        };
        let mut inputs = Vec::new();
        // A subagent's view shows items already loaded with it.
        if wants_older(&self.list) && self.agent.is_none() {
            inputs.push(Input::TopReached);
        }
        // The gap row loads what is below it as soon as it shows.
        let gap_shown = matches!(self.rows.last(), Some((Row::Gap, _)))
            && self.list.item_is_below_viewport(self.rows.len() - 1) == Some(false);
        if at_end || gap_shown {
            inputs.push(Input::BottomReached);
        } else if let Some(anchor) = self
            .rows
            .get(top)
            .and_then(|(row, _)| row.anchor())
            .and_then(|id| view.position_of(id))
        {
            inputs.push(Input::ScrolledAway { anchor });
        }
        self.viewport(inputs, cx);
    }

    /// Feeds the viewport and performs what it asks for.
    pub(crate) fn viewport(&mut self, inputs: Vec<Input>, cx: &mut Context<Self>) {
        let Some(view) = self.view.as_mut() else {
            return;
        };
        let effects: Vec<Effect> = inputs
            .into_iter()
            .flat_map(|input| view.handle(&input))
            .collect();
        self.perform(effects, cx);
    }

    /// The latest run of the open thread.
    fn run(&self, cx: &App) -> Option<Run> {
        let (_, thread) = self.open.as_ref()?;
        self.host(cx)?.read(cx).runs.get(thread).cloned()
    }

    /// Refolds the rows, hands changed rows their new model, and tells the
    /// list which rows came and went. Unchanged rows stay cached.
    fn rebuild(&mut self, cx: &mut Context<Self>) {
        let run = self.run(cx);
        let now = artisan_api::now();
        let folded = self
            .view
            .as_ref()
            .map(|view| fold::fold(view, run.as_ref(), self.agent.as_ref()));
        let (rows, runs) = folded
            .map(|folded| (folded.rows, folded.runs))
            .unwrap_or_default();
        let old_keys: Vec<RowKey> = self.rows.iter().map(|(row, _)| row.key()).collect();
        let new_keys: Vec<RowKey> = rows.iter().map(|(row, _)| row.key()).collect();
        splice(&self.list, &old_keys, &new_keys);
        let mut kept: HashMap<RowKey, Entity<RowView>> =
            old_keys.into_iter().zip(self.row_views.drain(..)).collect();
        let children = self
            .view
            .as_ref()
            .map(subagent_children)
            .unwrap_or_default();
        let agents = self.view.as_ref().map(agent_names).unwrap_or_default();
        let agents_version = {
            let mut sorted: Vec<_> = agents.iter().collect();
            sorted.sort();
            let mut hasher = DefaultHasher::new();
            sorted.hash(&mut hasher);
            hasher.finish()
        };
        let mut views = Vec::with_capacity(rows.len());
        let gaps = self
            .view
            .as_ref()
            .map(|view| fold::gaps(view, &rows))
            .unwrap_or_default();
        let end_space = self.end_space();
        for (index, ((row, base), key)) in rows.iter().zip(new_keys).enumerate() {
            let summary = row.run().and_then(|run| runs.get(run)).cloned();
            let gap = gaps.get(index).copied().unwrap_or(0.0);
            let end = if index + 1 == rows.len() {
                end_space
            } else {
                0.0
            };
            let images = self.row_images(row, cx);
            let mut hasher = DefaultHasher::new();
            base.hash(&mut hasher);
            agents_version.hash(&mut hasher);
            (gap.to_bits(), end.to_bits()).hash(&mut hasher);
            for (hash, blob) in &images {
                (hash, std::mem::discriminant(blob)).hash(&mut hasher);
            }
            // A live section's header counts the seconds.
            if summary.as_ref().is_some_and(fold::RunSummary::live)
                && matches!(row, Row::Work { segment: 0, .. })
            {
                (now / 1_000).hash(&mut hasher);
            }
            let version = hasher.finish();
            let model = |this: &Self| {
                this.model(
                    row,
                    &children,
                    summary.clone(),
                    images.clone(),
                    now,
                    gap,
                    end,
                    &agents,
                )
            };
            let view = if let Some(view) = kept.remove(&key) {
                if view.read(cx).version() != version {
                    let model = model(self);
                    view.update(cx, |view, cx| view.set(model, version, cx));
                    self.list.remeasure_items(index..index + 1);
                }
                view
            } else {
                let (model, shared) = (model(self), self.shared.clone());
                cx.new(|_| RowView::new(model, version, shared))
            };
            views.push(view);
        }
        self.rows = rows;
        self.row_views = views;
        self.sync_goal(cx);
        self.sync_questionnaires(cx);
        let live = run.is_some_and(|run| run.status.is_live());
        if live && self.clock.is_none() {
            self.clock = Some(cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(Duration::from_secs(1)).await;
                    if this.update(cx, Self::rebuild).is_err() {
                        break;
                    }
                }
            }));
        } else if !live {
            self.clock = None;
        }
        cx.notify();
    }

    /// A snapshot of what one row shows.
    #[allow(clippy::too_many_arguments)]
    fn model(
        &self,
        row: &Row,
        children: &HashMap<ItemId, Vec<ItemId>>,
        run: Option<fold::RunSummary>,
        images: HashMap<BlobHash, Blob>,
        now: artisan_api::Timestamp,
        gap: f32,
        end: f32,
        agents: &HashMap<String, String>,
    ) -> RowModel {
        let mut model = RowModel {
            agents: agents.clone(),
            row: row.clone(),
            items: HashMap::new(),
            children: HashMap::new(),
            live: HashMap::new(),
            ui: HashMap::new(),
            run,
            images,
            now,
            gap,
            end,
        };
        let Some(view) = self.view.as_ref() else {
            return model;
        };
        model.live.clone_from(&view.live);
        let mut stack: Vec<ItemId> = match row {
            Row::Work { entries, .. } => entries.clone(),
            other => other.anchor().cloned().into_iter().collect(),
        };
        // A subagent's task and report read the agent alone.
        let nest = !matches!(row, Row::AgentTask(_) | Row::AgentResult(_));
        while let Some(id) = stack.pop() {
            if let Some(item) = view.item(&id) {
                model.items.insert(id.clone(), item.clone());
            }
            if let Some(ui) = view.ui.get(&id) {
                model.ui.insert(id.clone(), ui.clone());
            }
            if let Some(nested) = children.get(&id).filter(|_| nest) {
                stack.extend(nested.iter().cloned());
                model.children.insert(id, nested.clone());
            }
        }
        model
    }

    /// The images a row shows, loading any not yet fetched: its anchor's,
    /// and in a work section every item's (a steer sent with images sits
    /// there, not on a row of its own).
    fn row_images(&mut self, row: &Row, cx: &mut Context<Self>) -> HashMap<BlobHash, Blob> {
        let ids: Vec<ItemId> = match row {
            Row::Work { entries, .. } => entries.clone(),
            other => other.anchor().cloned().into_iter().collect(),
        };
        let hashes: Vec<BlobHash> = ids
            .iter()
            .filter_map(|id| self.view.as_ref()?.item(id))
            .flat_map(|item| match &item.kind {
                ItemKind::UserMessage { images, .. } => images.clone(),
                ItemKind::Image { blob, .. } => vec![blob.clone()],
                _ => Vec::new(),
            })
            .collect();
        hashes
            .into_iter()
            .filter_map(|hash| Some((hash.clone(), self.blob(&hash, cx)?)))
            .collect()
    }

    fn toggle_work(&mut self, run: &artisan_api::RunId, open: bool, cx: &mut Context<Self>) {
        if let Some(view) = self.view.as_mut() {
            view.work_open.insert(run.clone(), open);
            self.rebuild(cx);
        }
    }

    fn toggle_chain(&mut self, id: &ItemId, open: bool, cx: &mut Context<Self>) {
        if let Some(view) = self.view.as_mut() {
            view.ui.entry(id.clone()).or_default().chain_open = Some(open);
            self.rebuild(cx);
        }
    }

    /// The questions waiting on the user, oldest first, to the composer,
    /// where they are answered.
    /// The last revealing prose finished: the status line, drawn from
    /// cache while it hid, renders again.
    fn wake_status(&mut self, cx: &mut Context<Self>) {
        for (index, ((row, _), view)) in self.rows.iter().zip(&self.row_views).enumerate() {
            if matches!(row, Row::Status(_) | Row::AgentStatus(..)) {
                view.update(cx, RowView::refresh);
                self.list.remeasure_items(index..index + 1);
            }
        }
    }

    fn sync_questionnaires(&mut self, cx: &mut Context<Self>) {
        let Some(composer) = self.composer.clone() else {
            return;
        };
        let open: Vec<ComposerQuestionnaire> = self
            .view
            .iter()
            .flat_map(|view| view.items.values())
            .filter(|item| item.status == Status::AwaitingUser)
            .filter_map(|item| match &item.kind {
                ItemKind::Question {
                    questions,
                    answers: None,
                    ..
                } => Some(ComposerQuestionnaire {
                    item: item.id.clone(),
                    questions: questions.clone(),
                }),
                _ => None,
            })
            .collect();
        composer.update(cx, |composer, cx| composer.set_questionnaires(open, cx));
    }

    /// The thread's goal, as its latest goal notice left it, on the
    /// composer's strip.
    fn sync_goal(&mut self, cx: &mut Context<Self>) {
        let Some(composer) = self.composer.clone() else {
            return;
        };
        let goal = self.view.as_ref().and_then(|view| {
            view.items.values().rev().find_map(|item| match &item.kind {
                ItemKind::Notice {
                    kind: artisan_api::NoticeKind::Goal { state },
                    text,
                } => Some((state.clone(), text.clone())),
                _ => None,
            })
        });
        let goal = goal.and_then(|(state, objective)| {
            let state = match state.as_str() {
                "active" => ComposerGoalState::Active,
                "paused" => ComposerGoalState::Paused,
                _ => return None,
            };
            Some(ComposerGoal {
                objective: objective.into(),
                state,
                detail: None,
            })
        });
        composer.update(cx, |composer, cx| composer.set_goal(goal, cx));
    }

    /// The space under the last row (`TRANSCRIPT_END_SPACE_PX`, or more
    /// when the composer card is taller): the latest words always clear
    /// the floating composer.
    fn end_space(&self) -> f32 {
        const END_SPACE: f32 = 192.0;
        if self.composer.is_none() {
            return render::COMPOSER_GAP;
        }
        END_SPACE.max(self.composer_height + render::COMPOSER_BOTTOM + render::COMPOSER_GAP)
    }

    /// The composer card's measured height changed: the end space follows.
    fn composer_measured(&mut self, height: f32, cx: &mut Context<Self>) {
        if (height - self.composer_height).abs() >= 0.5 {
            self.composer_height = height;
            self.rebuild(cx);
        }
    }

    fn toggle_item(&mut self, id: &ItemId, open: bool, cx: &mut Context<Self>) {
        if let Some(view) = self.view.as_mut() {
            view.ui.entry(id.clone()).or_default().expanded = Some(open);
            self.rebuild(cx);
        }
    }

    /// Sends an approval decision.
    fn answer(&mut self, item: ItemId, answer: Answer, cx: &mut Context<Self>) {
        let (Some(host), Some(view)) = (self.host(cx), self.view.as_mut()) else {
            return;
        };
        let ui = view.ui.entry(item.clone()).or_default();
        if ui.sending {
            return;
        }
        ui.sending = true;
        ui.error = None;
        self.rebuild(cx);
        let client = host.read(cx).client.clone();
        cx.spawn(async move |this, cx| {
            let result = client.answer(&item, &answer).await;
            let _ = this.update(cx, |this, cx| {
                if let Some(ui) = this.view.as_mut().and_then(|view| view.ui.get_mut(&item)) {
                    ui.sending = false;
                    if let Err(error) = &result {
                        tracing::warn!(item = %item, "couldn't send the answer: {error:#}");
                        ui.error = Some(format!("Couldn't send the answer: {error:#}"));
                    }
                }
                this.rebuild(cx);
            });
        })
        .detach();
    }

    /// A blob's state, starting its download the first time it is asked for.
    fn blob(&mut self, hash: &BlobHash, cx: &mut Context<Self>) -> Option<Blob> {
        if !self.blobs.contains_key(hash) {
            let host = self.host(cx)?;
            self.blobs.insert(hash.clone(), Blob::Loading);
            let client = host.read(cx).client.clone();
            let hash = hash.clone();
            cx.spawn(async move |this, cx| {
                let result = client.blob(&hash).await;
                let _ = this.update(cx, |this, cx| {
                    let blob = match result {
                        Ok(bytes) => {
                            Blob::Ready(Arc::new(Image::from_bytes(image_format(&bytes), bytes)))
                        }
                        Err(error) => {
                            tracing::warn!(blob = %hash, "couldn't load an image: {error:#}");
                            Blob::Failed(format!("{error:#}"))
                        }
                    };
                    this.blobs.insert(hash, blob);
                    this.rebuild(cx);
                });
            })
            .detach();
        }
        self.blobs.get(hash).cloned()
    }
}

/// The thread's agents by the id `SendMessage` addresses them by: their
/// names, for "Talked to …".
fn agent_names(view: &ThreadView) -> HashMap<String, String> {
    view.items
        .values()
        .filter_map(|item| match &item.kind {
            ItemKind::Agent {
                agent_id: Some(agent_id),
                ..
            } => Some((agent_id.clone(), item.agent_label()?)),
            _ => None,
        })
        .collect()
}

/// Subagent items under their Agent item, by id.
fn subagent_children(view: &ThreadView) -> HashMap<ItemId, Vec<ItemId>> {
    let mut children: HashMap<ItemId, Vec<ItemId>> = HashMap::new();
    for item in view.shown() {
        if let Some(parent) = &item.parent_id {
            children
                .entry(parent.clone())
                .or_default()
                .push(item.id.clone());
        }
    }
    children
}

/// Tells the list which rows came and went, so rows that stay keep their
/// measured heights and the scroll position stays on what the reader sees
/// (older items prepended above don't move it).
fn splice(list: &ListState, old: &[RowKey], new: &[RowKey]) {
    let old_set: HashSet<&RowKey> = old.iter().collect();
    let new_set: HashSet<&RowKey> = new.iter().collect();
    let (mut i, mut j) = (0, 0);
    while i < old.len() || j < new.len() {
        if i < old.len() && j < new.len() && old[i] == new[j] {
            i += 1;
            j += 1;
            continue;
        }
        let removed = old[i..]
            .iter()
            .take_while(|key| !new_set.contains(key))
            .count();
        if removed > 0 {
            list.splice(j..j + removed, 0);
            i += removed;
            continue;
        }
        let inserted = new[j..]
            .iter()
            .take_while(|key| !old_set.contains(key))
            .count();
        if inserted > 0 {
            list.splice(j..j, inserted);
            j += inserted;
            continue;
        }
        // Rows changed order (rare): replace the rest.
        list.splice(j..j + (old.len() - i), new.len() - j);
        break;
    }
}

fn image_format(bytes: &[u8]) -> gpui::ImageFormat {
    match bytes {
        [0x89, b'P', b'N', b'G', ..] => gpui::ImageFormat::Png,
        [0xFF, 0xD8, ..] => gpui::ImageFormat::Jpeg,
        [b'G', b'I', b'F', ..] => gpui::ImageFormat::Gif,
        _ => gpui::ImageFormat::Webp,
    }
}
