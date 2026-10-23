//! The open thread. One copy of its items, a viewport, and a pure render.
//!
//! Items come from `GET /threads/{id}/items` pages and the host's event
//! stream and are replaced by id; immutable live Rope snapshots are shared with the host. The
//! transcript is a fold of items + live text + UI toggles into rows, shown
//! in a GPUI list.

mod activity;
mod admission;
mod body;
mod collections;
mod content;
mod disclosure;
mod fold;
mod motion;
mod phase;
mod preview;
mod render;
mod row;
mod source_actions;
mod thread;
mod viewport;

#[cfg(test)]
mod tests;

pub use phase::{Phase, turn_phase};

use artisan_api::{Answer, BlobHash, Item, ItemId, ItemKind, Run, ThreadId};
use body::Bodies;
use editor_composer::{
    Composer, ComposerEvent, ComposerGoal, ComposerGoalState, ComposerNote, ComposerQuestionnaire,
    ComposerTarget,
};
use editor_core::{Core, CoreEvent, HostData, HostEvent, HostId, Screen};
use editor_ui::theme::ActiveTheme as _;
use editor_ui::theme::ArtisanTheme;
use editor_ui::wheel_glide::WheelGlide;
use fold::{Row, RowKey, Slot};
use gpui::{
    App, AppContext as _, Context, Entity, EventEmitter, FollowMode, ListAlignment, ListOffset,
    ListState, ScrollWheelEvent, SharedString, Subscription, Task, Window, px,
};
use row::{Blob, RowEnv, RowModel, RowView};
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

/// Row views kept mounted, most recently shown first out.
const ROW_VIEWS: usize = 128;

/// Whether the view is close enough to the oldest loaded row to read on.
pub(crate) fn wants_older(list: &ListState) -> bool {
    let above = -f32::from(list.scroll_px_offset_for_scrollbar().y);
    f32::from(list.viewport_bounds().size.height) > 0.0 && above < READ_AHEAD_PX
}

/// Whether the Editor's window is the one in front.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WindowState {
    Active,
    Inactive,
}

impl WindowState {
    fn of(window: &Window) -> Self {
        if window.is_window_active() {
            Self::Active
        } else {
            Self::Inactive
        }
    }
}

/// A jump's transcript starts this far short of its target…
const ARRIVAL_PX: f32 = 160.0;
/// …and eases into place over this long.
const ARRIVAL: Duration = Duration::from_millis(180);

#[derive(Clone, Debug)]
pub enum ConversationEvent {
    /// The open thread's items changed (the inspector re-reads them).
    ItemsChanged,
    /// The composer's project menu asked for a new project.
    NewProject,
    /// A subagent's view opened or closed (the title shows its name).
    AgentChanged,
}

#[derive(Default)]
struct RowContext {
    runs: HashMap<artisan_api::RunId, fold::RunSummary>,
    agents: Arc<HashMap<ItemId, String>>,
    agents_version: u64,
    gaps: Vec<f32>,
    versions: Vec<u64>,
    now: artisan_api::Timestamp,
    end_space: f32,
    /// A terminal run's activity boundary is read only with its newest current output.
    read_boundary: Option<(ItemId, artisan_api::Seq)>,
    item_rows: HashMap<ItemId, Vec<usize>>,
    key_rows: HashMap<RowKey, usize>,
    status_rows: HashMap<artisan_api::RunId, Vec<usize>>,
}

pub struct Conversation {
    core: Entity<Core>,
    /// The subagent whose view is open in place of the thread's.
    agent: Option<ItemId>,
    /// The new-thread host's connection phase (whether Forge was reached
    /// before) and when it began, for the delayed reassurance line.
    waiting: Option<(bool, std::time::Instant)>,
    waiting_generation: u64,
    waiting_timer: Option<gpui::Task<()>>,
    open: Option<(HostId, ThreadId)>,
    view: Option<ThreadView>,
    /// The last fold of the items, run and toggles, before text bodies
    /// split into blocks, with each run's summary.
    folded: (
        Vec<(Row, u64)>,
        HashMap<artisan_api::RunId, fold::RunSummary>,
    ),
    /// The published rows and, in the same order, their views.
    rows: Vec<(Row, u64)>,
    row_views: Vec<Option<Entity<RowView>>>,
    row_context: RowContext,
    disclosures: HashMap<RowKey, disclosure::Flight>,
    /// What each flight paints this frame: row clips, the members it hides
    /// and its header's open fraction.
    disclosure_clips: HashMap<RowKey, disclosure::RowClip>,
    disclosure_hidden: HashSet<ItemId>,
    disclosure_fractions: HashMap<RowKey, f32>,
    /// The space above the row after each group in flight, eased between
    /// its closed and open spacing.
    disclosure_gaps: HashMap<RowKey, f32>,
    /// Each painted row's measured height, which a disclosure clips.
    row_heights: HashMap<RowKey, gpui::Pixels>,
    /// Rows hold their reveal until the fresh content before them has.
    reveal: Rc<RefCell<motion::RevealQueue>>,
    /// The row fresh content reveals up to; later rows wait hidden.
    reveal_frontier: Option<RowKey>,
    row_cache_order: std::collections::VecDeque<usize>,
    list: ListState,
    /// The transcript's scroll edges: under the header and over the composer.
    edges: [editor_ui::scroll_edge::EdgeFade; 2],
    composer: Option<Entity<Composer>>,
    composer_events: Option<Subscription>,
    bodies: Bodies,
    theme: ArtisanTheme,
    /// Why the last page failed to load.
    load_error: Option<String>,
    /// Height of the composer card floating over the transcript; the end
    /// space under the last row clears it.
    composer_height: f32,
    /// The pointer is over the turn navigator (rail or menu).
    navigator_hovered: bool,
    navigator_hover: Rc<RefCell<editor_ui::picker_motion::SlidingHoverState>>,
    navigator_hover_surface: editor_ui::measure::Measured,
    /// The menu's rows, scrolled to the turn in view as it opens.
    navigator_scroll: gpui::UniformListScrollHandle,
    /// Rebuilt only when transcript rows change; labels retain bounded first lines.
    /// Forge's outline as markers, and the summary they were made from.
    navigator_markers: Option<(usize, Arc<[render::Marker]>)>,
    /// Per tick (by message), whether it was last active and when that
    /// changed, while it eases between its two looks.
    navigator_ticks: HashMap<ItemId, (bool, Option<std::time::Instant>)>,
    /// Whether a tick was active last frame: the first one placed does not
    /// ease in.
    navigator_had_active: bool,
    /// The page being read. Reopening, closing or a stream reset drops it,
    /// so a reply for another scope never lands.
    page_task: Option<Task<()>>,
    /// The open agent's ancestry being read; dropped like `page_task`.
    agent_task: Option<Task<()>>,
    /// The unloaded work span being read, by its first position.
    work_reading: Option<artisan_api::Seq>,
    /// That read; dropped like `page_task`.
    work_task: Option<Task<()>>,
    /// The opened thread is hidden until its first screen is ready (see
    /// `landed`), at most until this deadline.
    landing: Option<std::time::Instant>,
    landing_timer: Option<Task<()>>,
    /// How many times the reader has scrolled; source pages never move
    /// back the way they came until it changes.
    reader_moves: u64,
    /// Whether the reader last scrolled toward the end.
    reader_toward_end: Option<bool>,
    /// When this frame's render began; its paint reports a slow frame.
    frame_started: Rc<std::cell::Cell<Option<std::time::Instant>>>,
    /// Wheel ticks glide; precise trackpad deltas use native scrolling.
    glide: WheelGlide,
    glide_scheduled: bool,
    /// A jump's arrival: when it began and how far short it started.
    arrival: Option<(std::time::Instant, f32)>,
    /// Ticks the elapsed time while the open thread's run is live.
    clock: Option<Task<()>>,
    host_events: Option<Subscription>,
    /// The new-thread screen's host: its connection card follows it.
    new_thread_host: Option<(HostId, Subscription)>,
    _core_events: Subscription,
    /// Whether the window is active: what arrives then is seen arriving,
    /// and animates.
    window: WindowState,
    selection_window: gpui::WindowId,
    _activation: Subscription,
    _release: Subscription,
}

impl EventEmitter<ConversationEvent> for Conversation {}

impl Conversation {
    pub fn new(core: Entity<Core>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let selection_window = window.window_handle().window_id();
        let release = cx.on_release(move |this, cx| {
            editor_ui::selectable_text::release_selection_scope(
                &this.bodies.scope,
                selection_window,
                cx,
            );
            this.bodies.markdown.clear(cx);
        });
        let activation = cx.observe_window_activation(window, |this, window, _| {
            this.window = WindowState::of(window);
        });
        let core_events =
            cx.subscribe_in(&core, window, |this, _, event: &CoreEvent, window, cx| {
                if matches!(event, CoreEvent::ScreenChanged | CoreEvent::HostsChanged) {
                    this.sync_screen(window, cx);
                }
            });
        // The native list owns tail following and measured scroll geometry.
        let list = ListState::new(0, ListAlignment::Top, px(OVERDRAW_PX));
        let this = cx.entity().downgrade();
        list.set_scroll_handler(move |event, _, cx| {
            // The list is still borrowed here; `scrolled` reads it.
            let (this, top) = (this.clone(), event.visible_range.start);
            cx.defer(move |cx| {
                this.update(cx, |this, cx| this.scrolled(top, cx)).ok();
            });
        });
        let theme = *cx.theme();
        let mut conversation = Self {
            core,
            agent: None,
            waiting: None,
            waiting_generation: 0,
            waiting_timer: None,
            open: None,
            view: None,
            folded: Default::default(),
            rows: Vec::new(),
            row_views: Vec::new(),
            row_context: RowContext::default(),
            disclosures: HashMap::new(),
            disclosure_clips: HashMap::new(),
            disclosure_hidden: HashSet::new(),
            disclosure_fractions: HashMap::new(),
            disclosure_gaps: HashMap::new(),
            row_heights: HashMap::new(),
            reveal: Rc::default(),
            reveal_frontier: None,
            row_cache_order: std::collections::VecDeque::new(),
            list,
            edges: Default::default(),
            composer: None,
            composer_events: None,
            bodies: Bodies::new(),
            theme,
            load_error: None,
            composer_height: 0.0,
            navigator_hovered: false,
            navigator_hover: Rc::default(),
            navigator_hover_surface: editor_ui::measure::Measured::default(),
            navigator_scroll: gpui::UniformListScrollHandle::new(),
            navigator_markers: None,
            navigator_ticks: HashMap::new(),
            navigator_had_active: false,
            page_task: None,
            agent_task: None,
            work_reading: None,
            work_task: None,
            landing: None,
            landing_timer: None,
            reader_moves: 0,
            reader_toward_end: None,
            frame_started: Rc::default(),
            glide: WheelGlide::default(),
            glide_scheduled: false,
            arrival: None,
            clock: None,
            host_events: None,
            new_thread_host: None,
            _core_events: core_events,
            window: WindowState::of(window),
            selection_window,
            _activation: activation,
            _release: release,
        };
        conversation.sync_screen(window, cx);
        conversation
    }

    fn update_selection_scope(&mut self) {
        self.bodies.agent_scope = self.agent.is_some();
        if let Some((host, thread)) = &self.open {
            let scope: SharedString = format!(
                "{host}/{thread}/{}",
                self.agent.as_ref().map_or("root", ItemId::as_str)
            )
            .into();
            self.bodies.markdown.set_selection_scope(scope.clone());
            self.bodies.scope = scope;
        }
    }
    fn release_selection(&self, cx: &mut App) {
        editor_ui::selectable_text::release_selection_scope(
            &self.bodies.scope,
            self.selection_window,
            cx,
        );
    }

    /// Another transcript: its prepared bodies, motion and measurements go.
    fn forget_transcript(&mut self, cx: &mut App) {
        self.release_selection(cx);
        self.bodies.clear(cx);
        self.disclosures.clear();
        self.reveal_frontier = None;
        self.disclosure_clips.clear();
        self.disclosure_hidden.clear();
        self.disclosure_fractions.clear();
        self.disclosure_gaps.clear();
        self.row_heights.clear();
    }

    /// The thread on screen, if any.
    #[must_use]
    pub const fn open_thread(&self) -> Option<&(HostId, ThreadId)> {
        self.open.as_ref()
    }

    /// The loaded items of the open thread, in order.
    pub fn items(&self) -> impl Iterator<Item = &Item> {
        self.view
            .iter()
            .flat_map(|view| view.items.values().map(Arc::as_ref))
    }

    #[must_use]
    pub const fn core(&self) -> &Entity<Core> {
        &self.core
    }

    /// Scrolls to an item of the open thread, loading history until it is in.
    pub fn jump_to(&mut self, item: &Item, cx: &mut Context<Self>) {
        if self
            .view
            .as_ref()
            .is_some_and(|view| view.thread == item.thread_id)
        {
            self.jump_to_position(item.position, cx);
        }
    }

    /// Scrolls to `position` in the open thread, loading the stretch around
    /// it when it isn't loaded.
    pub(crate) fn jump_to_position(&mut self, position: artisan_api::Seq, cx: &mut Context<Self>) {
        let Some(view) = self.view.as_mut() else {
            return;
        };
        let effects = view.jump(Jump::To(position));
        self.clear_page_frontiers();
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

    /// The opened thread is still preparing its first screen: its first
    /// page, the edges around the view and the text in it.
    pub(crate) const fn landing(&self) -> bool {
        self.agent.is_none()
            && self.load_error.is_none()
            && self.view.is_some()
            && self.landing.is_some()
    }

    /// Shows the subagent `agent`'s own work in place of the thread's, like
    /// opening its thread; `None` goes back to the thread.
    /// The open agent's context: tokens used and its model, when its view
    /// is open (`Some(None)` while it has no reading yet).
    #[must_use]
    pub fn agent_context(&self) -> Option<Option<(u64, Option<String>)>> {
        let agent = self.agent.as_ref()?;
        let item = self.view.as_ref()?.item(agent);
        Some(item.and_then(|item| match &item.kind {
            ItemKind::Agent {
                context_used: Some(used),
                model,
                ..
            } => Some((*used, model.clone())),
            ItemKind::UserMessage { .. }
            | ItemKind::AssistantMessage { .. }
            | ItemKind::Tool { .. }
            | ItemKind::Agent { .. }
            | ItemKind::Question { .. }
            | ItemKind::Plan { .. }
            | ItemKind::Image { .. }
            | ItemKind::Compaction { .. }
            | ItemKind::Notice { .. }
            | ItemKind::Error { .. } => None,
        }))
    }

    pub fn open_agent(&mut self, agent: Option<ItemId>, cx: &mut Context<Self>) {
        if self.agent == agent {
            return;
        }
        self.page_task = None;
        self.agent_task = None;
        self.load_error = None;
        self.forget_transcript(cx);
        let context = agent
            .as_ref()
            .and_then(|id| self.view.as_ref()?.shared_item(id));
        self.agent.clone_from(&agent);
        self.update_selection_scope();
        let mut effects = Some(if let Some(view) = self.view.as_mut() {
            view.context.clear();
            if let Some(item) = context {
                view.context.insert(item.id.clone(), item);
            }
            view.reset()
        } else {
            Vec::new()
        });
        if let (Some(agent), Some(host), Some(view)) = (agent, self.host(cx), self.view.as_ref()) {
            let client = host.read(cx).client.clone();
            let thread = view.thread.clone();
            let effects = effects.take().unwrap_or_default();
            let open = self.open.clone();
            let requested_agent = agent.clone();
            self.agent_task = Some(cx.spawn(async move |this, cx| {
                let mut context = HashMap::new();
                let mut next = Some(agent);
                let mut error = None;
                for _ in 0..16 {
                    let Some(id) = next.take() else { break };
                    if context.contains_key(&id) {
                        error = Some("Agent ancestry contains a cycle".to_owned());
                        break;
                    }
                    match client.item(&id).await {
                        Ok(item)
                            if item.thread_id == thread
                                && matches!(item.kind, ItemKind::Agent { .. }) =>
                        {
                            next.clone_from(&item.parent_id);
                            context.insert(id, Arc::new(item));
                        }
                        Ok(_) => {
                            error = Some("Agent does not belong to this thread".to_owned());
                            break;
                        }
                        Err(err) => {
                            error = Some(format!("Couldn't load agent: {err:#}"));
                            break;
                        }
                    }
                }
                if next.is_some() {
                    error = Some("Agent ancestry exceeds 16 levels".to_owned());
                }
                this.update(cx, |this, cx| {
                    if this.open != open || this.agent.as_ref() != Some(&requested_agent) {
                        return;
                    }
                    this.agent_task = None;
                    if let Some(view) = this.view.as_mut() {
                        for (id, item) in context {
                            if view.context.get(&id).is_none_or(|old| old.seq < item.seq) {
                                view.context.insert(id, item);
                            }
                        }
                    }
                    if error.is_some() {
                        this.load_error = error;
                    }
                    this.rebuild(cx);
                    if this.load_error.is_none() {
                        this.perform(effects, cx);
                    }
                    cx.emit(ConversationEvent::AgentChanged);
                })
                .ok();
            }));
        }
        // Another transcript: build it fresh and land on its end.
        self.rows.clear();
        self.navigator_markers = None;
        self.row_views.clear();
        self.row_cache_order.clear();
        self.list.reset(0);
        self.glide.cancel();
        self.list.set_follow_mode(FollowMode::Tail);
        self.perform(effects.unwrap_or_default(), cx);
        self.rebuild(cx);
        self.list.scroll_to_end();
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
        self.clear_page_frontiers();
        self.perform(effects, cx);
    }

    fn clear_page_frontiers(&mut self) {
        self.bodies.frontiers.clear();
        for state in self.bodies.native.values_mut() {
            state.frontier = None;
        }
    }

    fn host(&self, cx: &App) -> Option<Entity<HostData>> {
        let (host, _) = self.open.as_ref()?;
        self.core.read(cx).host(host).cloned()
    }

    fn follow_new_thread_host(&mut self, id: &HostId, cx: &mut Context<Self>) {
        if self
            .new_thread_host
            .as_ref()
            .is_some_and(|(followed, _)| followed == id)
        {
            return;
        }
        let Some(host) = self.core.read(cx).host(id).cloned() else {
            self.new_thread_host = None;
            return;
        };
        let subscription = cx.subscribe(&host, |_, _, event: &HostEvent, cx| {
            if matches!(event, HostEvent::Changed | HostEvent::Reset) {
                cx.notify();
            }
        });
        self.new_thread_host = Some((id.clone(), subscription));
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
                self.follow_new_thread_host(&host, cx);
                ComposerTarget::NewThread { host, project }
            }
            Screen::Home | Screen::Settings(_) | Screen::Marketplace | Screen::Usage => return,
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
                        // their message and reply arrive.
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
        self.sync_goal(cx);
        self.sync_questionnaires(cx);
        cx.notify();
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        self.page_task = None;
        self.agent_task = None;
        self.work_reading = None;
        self.work_task = None;
        self.landing = None;
        self.landing_timer = None;
        self.forget_transcript(cx);
        if self.agent.take().is_some() {
            cx.emit(ConversationEvent::AgentChanged);
        }
        if let Some((host_id, _)) = self.open.take() {
            if let Some(host) = self.core.read(cx).host(&host_id).cloned() {
                host.update(cx, |host, cx| host.observe_thread(None, cx));
            }
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
        let client = host.read(cx).client.clone();
        let resource_thread = thread.clone();
        self.bodies.markdown.set_media_fetcher(Some(
            editor_ui::markdown_renderer::MediaFetcher::new(
                format!("{host_id}/{thread}"),
                move |request| {
                    let client = client.clone();
                    let thread = resource_thread.clone();
                    Box::pin(async move {
                        client
                            .resource(&thread, &request.path, request.from_home)
                            .await
                            .ok()
                            .map(std::sync::Arc::<[u8]>::from)
                    })
                },
            )
            .reference_scope(editor_ui::markdown_renderer::ResourceScope {
                host: host_id.0.clone(),
                thread: thread.clone(),
            }),
        ));
        host.update(cx, |host, cx| host.observe_thread(Some(thread.clone()), cx));
        self.host_events = Some(cx.subscribe(&host, |this, host, event, cx| {
            this.host_event(&host, event, cx);
        }));
        self.open = Some((host_id.clone(), thread.clone()));
        self.update_selection_scope();
        self.load_error = None;
        let (mut view, effects) = ThreadView::open(thread.clone());
        view.set_live(host.read(cx).live_targets(thread));
        self.view = Some(view);
        self.list.reset(0);
        self.list.set_follow_mode(FollowMode::Tail);
        self.rows.clear();
        self.navigator_markers = None;
        self.row_views.clear();
        self.row_cache_order.clear();
        self.begin_landing(cx);
        self.perform(effects, cx);
        self.rebuild(cx);
        cx.emit(ConversationEvent::ItemsChanged);
    }

    /// Hides the transcript until its first screen is ready, or the
    /// deadline passes: a slow Forge still shows what it has.
    fn begin_landing(&mut self, cx: &Context<Self>) {
        const LANDING_AT_MOST: Duration = Duration::from_millis(1500);
        self.landing = Some(std::time::Instant::now() + LANDING_AT_MOST);
        self.landing_timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(LANDING_AT_MOST).await;
            this.update(cx, Self::landed).ok();
        }));
    }

    /// The first screen is ready: the transcript shows, and from here on
    /// nothing in view moves on its own.
    pub(crate) fn landed(&mut self, cx: &mut Context<Self>) {
        if self.landing.take().is_some() {
            self.landing_timer = None;
            cx.notify();
        }
    }

    fn host_event(&mut self, host: &Entity<HostData>, event: &HostEvent, cx: &mut Context<Self>) {
        // Seen arriving: the window is active and the transcript has landed
        // (while it opens, live items race its first page and are history).
        let watched = self.window == WindowState::Active && !self.landing();
        let Some(view) = self.view.as_mut() else {
            return;
        };
        let thread = view.thread.clone();
        match event {
            HostEvent::Item(item) => {
                // A message the root sent the open agent shows in its view.
                let directed = matches!(&item.kind,
                    ItemKind::Tool { tool: artisan_api::ToolKind::AgentMessage { target: Some(target), direction: artisan_api::MessageDirection::Sent, .. }, .. }
                        if self.agent.as_ref() == Some(target));
                if item.thread_id != thread
                    || (item.parent_id != self.agent
                        && !view.context.contains_key(&item.id)
                        && !directed)
                {
                    return;
                }
                let changed = view.apply_item(Arc::clone(item), watched);
                view.set_live(host.read(cx).live_targets(&thread));
                // The user sent the next message: the reply before it shows
                // the rest at once rather than keep pacing.
                let sent = matches!(
                    item.kind,
                    ItemKind::UserMessage {
                        steer: false,
                        from_agent: None,
                        ..
                    }
                ) && view.ui.get(&item.id).is_some_and(thread::ItemUi::fresh);
                if changed {
                    if watched && matches!(item.kind, ItemKind::AssistantMessage { .. }) {
                        self.observe_live_prose_arrival(&item.id);
                    }
                    if sent {
                        self.reveal.borrow_mut().rush(item.position);
                    }
                    self.rebuild(cx);
                    cx.emit(ConversationEvent::ItemsChanged);
                }
            }
            HostEvent::ThreadSummaryChanged { thread_id } => {
                if *thread_id == thread {
                    self.sync_goal(cx);
                    self.sync_questionnaires(cx);
                }
            }
            HostEvent::ImagesChanged => self.reexpand(cx),
            HostEvent::Live { thread_id, target } => {
                if *thread_id == thread {
                    let text = host
                        .read(cx)
                        .live_snapshot(&thread, target)
                        .map(|snapshot| snapshot.text);
                    if view.update_live(target, text) {
                        if watched && let artisan_api::LiveTarget::Text { item_id } = target {
                            self.observe_live_prose_arrival(item_id);
                        }
                        self.refresh_live_rows(target, cx);
                    }
                }
            }
            HostEvent::ThreadChanged { thread_id } => {
                if *thread_id == thread {
                    view.set_live(host.read(cx).live_targets(&thread));
                    self.rebuild(cx);
                }
            }
            HostEvent::EngineInstall { .. }
            | HostEvent::WslDistro { .. }
            | HostEvent::EditorCall { .. } => {}
            HostEvent::Changed => cx.notify(),
            HostEvent::Reset => {
                self.page_task = None;
                self.work_reading = None;
                self.work_task = None;
                self.list.set_follow_mode(FollowMode::Tail);
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
                    self.list.set_follow_mode(FollowMode::Tail);
                    self.list.scroll_to_end();
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
                        self.list.set_follow_mode(FollowMode::Normal);
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

    fn fetch(&mut self, fetch: Fetch, cx: &Context<Self>) {
        let (Some(host), Some(view)) = (self.host(cx), self.view.as_ref()) else {
            return;
        };
        let client = host.read(cx).client.clone();
        let thread = view.thread.clone();
        let open = self.open.clone();
        let agent = self.agent.clone();
        self.page_task = Some(cx.spawn(async move |this, cx| {
            let result = if let Some(agent) = agent.as_ref() {
                let query = thread::query(fetch, thread::PAGE);
                client.agent_items(&thread, agent, &query).await
            } else {
                // The thread pages in rows; its work loads as it opens.
                let query = thread::query(fetch, thread::ROWS_PAGE);
                client.transcript(&thread, &query).await
            };
            this.update(cx, |this, cx| {
                if this.open != open {
                    return;
                }
                let anchors = if matches!(fetch, Fetch::Older { .. } | Fetch::Newer { .. }) {
                    this.reading_anchors()
                } else {
                    Vec::new()
                };
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
                this.restore_anchor(&anchors);
                this.perform(effects, cx);
                cx.emit(ConversationEvent::ItemsChanged);
            })
            .ok();
        }));
    }

    /// Loads the newest stretch of the unloaded work span at `first`, which
    /// an open section shows as an earlier-work row. One at a time.
    pub(crate) fn load_work(&mut self, first: artisan_api::Seq, cx: &Context<Self>) {
        if self.work_reading.is_some() || self.agent.is_some() {
            return;
        }
        let (Some(host), Some(view)) = (self.host(cx), self.view.as_ref()) else {
            return;
        };
        let Some(span) = view.work.get(&first) else {
            return;
        };
        let client = host.read(cx).client.clone();
        let thread = view.thread.clone();
        let query = thread::work_query(span);
        let open = self.open.clone();
        let task = cx.spawn(async move |this, cx| {
            let result = client.work(&thread, &query).await;
            this.update(cx, |this, cx| {
                this.work_reading = None;
                if this.open != open {
                    return;
                }
                let Some(view) = this.view.as_mut().filter(|view| view.thread == thread) else {
                    return;
                };
                // What it loads goes in under its earlier-work row: above
                // the view (the native splice keeps the view), or opening
                // under a row in view, as a section does.
                match result {
                    Ok(page) => {
                        this.load_error = None;
                        if view.apply_work(first, page) {
                            this.rebuild(cx);
                            cx.emit(ConversationEvent::ItemsChanged);
                        }
                    }
                    Err(error) => {
                        tracing::warn!(thread = %thread, first, "couldn't load work: {error:#}");
                        this.load_error = Some(format!("Couldn't load this work: {error:#}"));
                    }
                }
                cx.notify();
            })
            .ok();
        });
        self.work_reading = Some(first);
        self.work_task = Some(task);
    }

    /// Where the rows on screen sit, for a reader not following the tail:
    /// a page can replace the work row at the loaded edge, and the native
    /// splice cannot keep an anchor whose key disappears.
    fn reading_anchors(&self) -> Vec<(RowKey, Option<ItemId>, gpui::Pixels)> {
        if self.list.is_following_tail() {
            return Vec::new();
        }
        let viewport = self.list.viewport_bounds();
        self.rows
            .iter()
            .enumerate()
            .filter_map(|(index, (row, _))| {
                let bounds = self.list.bounds_for_item(index)?;
                (bounds.bottom() > viewport.top() && bounds.top() < viewport.bottom()).then(|| {
                    (
                        row.key(),
                        row.anchor().cloned(),
                        viewport.top() - bounds.top(),
                    )
                })
            })
            .collect()
    }

    /// Puts the first surviving row of `anchors` back where it was.
    fn restore_anchor(&self, anchors: &[(RowKey, Option<ItemId>, gpui::Pixels)]) {
        if let Some((item_ix, offset_in_item)) = anchors.iter().find_map(|(key, item, offset)| {
            self.rows
                .iter()
                .position(|(row, _)| row.key() == *key && row.anchor() == item.as_ref())
                .map(|index| (index, *offset))
        }) {
            self.list.scroll_to(ListOffset {
                item_ix,
                offset_in_item,
            });
        }
    }

    /// Glides a wheel tick over the transcript. Trackpad
    /// deltas and reduced motion go to the list, which moves directly.
    pub(crate) fn transcript_wheel(
        &mut self,
        event: &ScrollWheelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let towards = f32::from(event.delta.pixel_delta(window.line_height()).y);
        if towards.abs() > f32::EPSILON {
            self.reader_toward_end = Some(towards < 0.0);
        }
        let max = f32::from(self.list.max_offset_for_scrollbar().y).max(0.0);
        if max <= 0.0 {
            if towards < 0.0 {
                // Down in a list that fits: back to following its tail.
                self.viewport(vec![Input::BottomReached], cx);
            } else if towards > 0.0
                && let Some(anchor) = self.view.as_ref().and_then(|view| view.window().oldest)
            {
                // Even a short list has an explicit reader position when the
                // reader asks for older content. Stop native tail following
                // before that page changes the height of the transcript.
                self.list.scroll_to(self.list.logical_scroll_top());
                // The whole list is on screen.
                let through = self.shown_through(0).unwrap_or(anchor);
                self.viewport(
                    vec![Input::ScrolledAway { anchor, through }, Input::TopReached],
                    cx,
                );
            }
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

    fn schedule_glide(&mut self, window: &mut Window, cx: &Context<Self>) {
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
        self.reader_moves += 1;
        let max = f32::from(self.list.max_offset_for_scrollbar().y).max(0.0);
        // A list that fits has no end to scroll to: the wheel says which
        // way the reader went (`transcript_wheel`), or reading older pages
        // would turn straight back into following and trim them away.
        let at_end =
            max > 0.0 && -f32::from(self.list.scroll_px_offset_for_scrollbar().y) >= max - 1.0;
        // A stored body's later page at the top: its source pages back
        // before the thread does.
        let source_prefix = self.rows.first().is_some_and(|(row, _)| {
            row.visual().is_some_and(|visual| visual > 0)
                || matches!(row.semantic(), Row::Text { id, slot, block, .. }
                    if *block > 0 && self.bodies.stored.contains_key(&(id.clone(), *slot)))
        });
        // The gap row loads what is below it as soon as it shows.
        let gap_shown = matches!(self.revealed_rows().last(), Some((Row::Gap, _)))
            && self.list.item_is_below_viewport(self.rows.len() - 1) == Some(false);
        let Some(view) = self.view.as_mut() else {
            return;
        };
        let mut inputs = Vec::new();
        // Root and agent scopes page through the same bounded window.
        if wants_older(&self.list) && !source_prefix {
            inputs.push(Input::TopReached);
        }
        if at_end || gap_shown {
            inputs.push(Input::BottomReached);
        } else if let Some(anchor) = self
            .rows
            .get(top)
            .and_then(|(row, _)| row.anchor())
            .and_then(|id| view.position_of(id))
        {
            let through = self.shown_through(top).unwrap_or(anchor);
            inputs.push(Input::ScrolledAway { anchor, through });
        }
        self.viewport(inputs, cx);
    }

    /// The newest loaded position among the rows on screen from `top` down.
    fn shown_through(&self, top: usize) -> Option<artisan_api::Seq> {
        let view = self.view.as_ref()?;
        self.rows
            .iter()
            .enumerate()
            .skip(top)
            .take_while(|(ix, _)| {
                *ix == top || self.list.item_is_below_viewport(*ix) == Some(false)
            })
            .filter_map(|(_, (row, _))| row.last_anchor())
            .filter_map(|id| view.position_of(id))
            .max()
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

    /// Live content does not change canonical Item order or create transcript structure.
    fn refresh_live_rows(&mut self, target: &artisan_api::LiveTarget, cx: &mut Context<Self>) {
        let text = self
            .view
            .as_ref()
            .and_then(|view| view.live.get(target))
            .cloned();
        let indices = match target {
            artisan_api::LiveTarget::Text { item_id }
            | artisan_api::LiveTarget::Shell { item_id }
            | artisan_api::LiveTarget::Progress { item_id } => {
                self.row_context.item_rows.get(item_id)
            }
            artisan_api::LiveTarget::Reasoning { run_id }
            | artisan_api::LiveTarget::Status { run_id } => {
                self.row_context.status_rows.get(run_id)
            }
        }
        .cloned()
        .unwrap_or_default();
        for index in indices {
            let Some(view) = self.row_views.get(index).and_then(Clone::clone) else {
                continue;
            };
            // Live text is prepared like any reply text.
            let body = matches!(target, artisan_api::LiveTarget::Text { .. })
                .then(|| self.text_body(&self.rows[index].0.clone(), cx));
            view.update(cx, |view, cx| {
                view.set_live(target, text.as_ref(), cx);
                if let Some(body) = body {
                    view.set_body(body, cx);
                }
            });
            self.list.remeasure_items(index..index + 1);
        }
        cx.notify();
    }

    /// Native geometry belongs to the current immutable literal source leaf.
    fn current_native_leaf(
        &self,
        row: &Row,
        leaf: &editor_ui::markdown_renderer::LiteralLeaf,
    ) -> Option<(ItemId, Slot, usize)> {
        let Row::Text {
            id,
            slot: slot @ Slot::Plain(_),
            block,
            ..
        } = row.semantic()
        else {
            return None;
        };
        let item = self.view.as_ref()?.item(id)?;
        let current = matches!(slot.text(item),
            Some(artisan_api::TextContent::Stored(reference)) if reference == &leaf.source_ref)
            && self
                .bodies
                .stored
                .get(&(id.clone(), *slot))
                .is_some_and(|source| {
                    source.source_ref() == &leaf.source_ref
                        && source.leaf_descriptor(*block).is_some_and(|descriptor| {
                            descriptor.source_start == leaf.source_range.start as u64
                                && descriptor.source_end == leaf.source_range.end as u64
                        })
                });
        current.then(|| (id.clone(), *slot, *block))
    }

    /// A long literal leaf was laid out (or painted) at some width: its
    /// visual rows are known.
    pub(crate) fn native_body_ready(
        &mut self,
        row: &Row,
        leaf: &editor_ui::markdown_renderer::LiteralLeaf,
        ready: &editor_ui::selectable_text::NativeRowReady,
        painted: bool,
        cx: &mut Context<Self>,
    ) {
        let Some((id, slot, block)) = self.current_native_leaf(row, leaf) else {
            return;
        };
        if ready.row_count == 0 {
            return;
        }
        let Some(source) = self.bodies.literal.get(&(id.clone(), slot, block)) else {
            return;
        };
        let native_key: SharedString = format!("{id}:{slot}:leaf:{block}").into();
        // A deferred old-width/style frame must not replace the current
        // native layout after a resize or a presentation change.
        if self.bodies.markdown.native_row_count(
            &native_key,
            &source.visible,
            ready.font_size,
            ready.width,
            &ready.page.runs,
        ) != Some(ready.row_count)
        {
            return;
        }
        let key = row.semantic().key();
        let fresh = || body::NativeBodyWindow {
            reference: leaf.source_ref.clone(),
            range: leaf.source_range.clone(),
            width: ready.width,
            font_size: ready.font_size,
            runs: Arc::clone(&ready.page.runs),
            count: ready.row_count,
            window: ready.row_count.saturating_sub(body::NATIVE_BODY_WINDOW)..ready.row_count,
            frontier: None,
            painted: HashSet::default(),
            error: None,
        };
        let state = self.bodies.native.entry(key.clone());
        let (state, changed) = match state {
            std::collections::hash_map::Entry::Vacant(entry) => (entry.insert(fresh()), true),
            std::collections::hash_map::Entry::Occupied(entry) => {
                let state = entry.into_mut();
                let changed = state.reference != leaf.source_ref
                    || state.range != leaf.source_range
                    || state.width != ready.width
                    || state.font_size != ready.font_size
                    || !Arc::ptr_eq(&state.runs, &ready.page.runs)
                    || state.count != ready.row_count;
                if changed {
                    *state = fresh();
                }
                (state, changed)
            }
        };
        state.error = None;
        let newly_painted =
            painted && state.window.contains(&ready.ordinal) && state.painted.insert(ready.ordinal);
        // Geometry discovery and resizes expand only the current window;
        // registered rows keep their keys and the list its anchor.
        let desired = if state.count > 1 {
            state.window.len()
        } else {
            1
        };
        let registered = self
            .rows
            .iter()
            .filter(|(candidate, _)| candidate.semantic().key() == key)
            .count();
        if changed || registered != desired {
            self.respace(&id, slot, cx);
        } else if newly_painted {
            self.refresh_item(&id, cx);
        }
    }

    pub(crate) fn native_body_failed(
        &mut self,
        row: &Row,
        leaf: &editor_ui::markdown_renderer::LiteralLeaf,
        error: String,
        cx: &mut Context<Self>,
    ) {
        let Some((id, slot, _)) = self.current_native_leaf(row, leaf) else {
            return;
        };
        if let Some(state) = self.bodies.native.get_mut(&row.key()) {
            if state.error.as_ref() == Some(&error) {
                return;
            }
            state.error = Some(error);
            state.painted.clear();
        } else {
            // No geometry has been published yet. Keep the failure on the
            // original row, with nothing painted.
            self.bodies.native.insert(
                row.key(),
                body::NativeBodyWindow {
                    reference: leaf.source_ref.clone(),
                    range: leaf.source_range.clone(),
                    width: px(0.0),
                    font_size: px(0.0),
                    runs: Arc::from([]),
                    count: 1,
                    window: 0..1,
                    frontier: None,
                    painted: HashSet::default(),
                    error: Some(error),
                },
            );
        }
        self.respace(&id, slot, cx);
    }

    /// Marks a row's view most recently shown, evicting the least recent
    /// beyond the bound. Literal source selections follow the mounted rows.
    pub(crate) fn touch_row_cache(&mut self, index: usize, created: bool) {
        if self.row_cache_order.back() != Some(&index) {
            self.row_cache_order.retain(|cached| *cached != index);
            self.row_cache_order.push_back(index);
        }
        let mut evicted = false;
        while self.row_cache_order.len() > ROW_VIEWS {
            if let Some(oldest) = self.row_cache_order.pop_front() {
                self.row_views[oldest] = None;
                let key = self.rows[oldest].0.key();
                self.forget_height(&key);
                evicted = true;
            }
        }
        if created || evicted {
            let live: HashSet<_> = self
                .row_cache_order
                .iter()
                .filter_map(|index| match self.rows.get(*index)?.0.semantic() {
                    Row::Text {
                        id,
                        slot: slot @ Slot::Plain(_),
                        block,
                        ..
                    } => Some((id.clone(), *slot, *block)),
                    Row::Projected { .. }
                    | Row::Work { .. }
                    | Row::EarlierWork { .. }
                    | Row::WorkEntry { .. }
                    | Row::Chain { .. }
                    | Row::Text { .. }
                    | Row::Prompt(_)
                    | Row::Compaction(_)
                    | Row::Image(_)
                    | Row::Gap
                    | Row::Status(_)
                    | Row::AgentResult(_)
                    | Row::AgentStatus(..) => None,
                })
                .collect();
            self.bodies.literal.retain(|key, _| live.contains(key));
        }
    }

    /// A row's view went: its measurement goes too, unless a disclosure
    /// still clips it.
    fn forget_height(&mut self, key: &RowKey) {
        if !self.disclosure_clips.contains_key(key) {
            self.row_heights.remove(key);
        }
    }

    pub(crate) fn retry_native_body(&mut self, row: &Row, cx: &mut Context<Self>) {
        if let Some(state) = self.bodies.native.get_mut(&row.key()) {
            state.error = None;
        }
        if let Row::Text { id, slot, .. } = row.semantic() {
            let (id, slot) = (id.clone(), *slot);
            self.respace(&id, slot, cx);
        }
    }

    pub(crate) fn native_window_edge(
        &mut self,
        key: &RowKey,
        start: usize,
        cx: &mut Context<Self>,
    ) {
        let Some(state) = self.bodies.native.get_mut(key) else {
            return;
        };
        let start = start.min(state.count.saturating_sub(1));
        let window = start
            ..start
                .saturating_add(body::NATIVE_BODY_WINDOW)
                .min(state.count);
        if state.window == window {
            return;
        }
        state.window = window;
        let window = state.window.clone();
        state.painted.retain(|ordinal| window.contains(ordinal));
        if let Some((Row::Text { id, slot, .. }, _)) = self
            .rows
            .iter()
            .map(|(row, version)| (row.semantic(), version))
            .find(|(row, _)| row.key() == *key)
        {
            let (id, slot) = (id.clone(), *slot);
            self.respace(&id, slot, cx);
        }
    }

    pub(crate) fn source_window_edge(
        &mut self,
        id: &ItemId,
        slot: Slot,
        revision: u64,
        offset: usize,
        requested: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(source) = self.bodies.stored.get(&(id.clone(), slot)).cloned() else {
            return;
        };
        let Some(item) = self.view.as_ref().and_then(|view| view.item(id)) else {
            return;
        };
        if source.revision() != revision
            || !matches!(slot.text(item),
                Some(artisan_api::TextContent::Stored(reference)) if reference == source.source_ref())
            || source.loaded_range().start == offset
        {
            return;
        }
        // A reply to a page asked for before the latest one is stale.
        if self
            .bodies
            .pages
            .get(&(id.clone(), slot))
            .is_some_and(|latest| *latest != offset)
            && !requested
        {
            return;
        }
        self.bodies.pages.insert((id.clone(), slot), offset);
        let owner = cx.entity().downgrade();
        let notify_id = id.clone();
        let result = self.bodies.markdown.request_stored_window(
            format!("{id}:{slot}:index").into(),
            &source,
            offset as u64,
            cx,
            move |cx| {
                owner
                    .update(cx, |this, cx| {
                        this.source_window_edge(&notify_id, slot, revision, offset, false, cx);
                    })
                    .ok();
            },
        );
        if let editor_ui::markdown_renderer::StoredPreparation::Ready(source) = result {
            self.bodies.stored.insert((id.clone(), slot), source);
            self.respace(id, slot, cx);
        }
    }

    /// Arm arrivals only for source events observed on a visible native Item.
    /// Source-index pagination, first loading and scroll remounts never call this.
    fn observe_live_prose_arrival(&mut self, id: &ItemId) {
        let top = self.list.logical_scroll_top().item_ix;
        let visible = self.row_context.item_rows.get(id).is_some_and(|indices| {
            indices.iter().any(|index| {
                *index >= top && self.list.item_is_below_viewport(*index) == Some(false)
            })
        });
        if !visible {
            return;
        }
        let blocks = self.bodies.prose_blocks(id);
        let arrivals = &mut self.bodies.arrivals;
        if !arrivals.contains_key(id)
            && arrivals.len() >= 128
            && let Some(oldest) = arrivals
                .iter()
                .min_by_key(|(_, arrival)| arrival.seen)
                .map(|(id, _)| id.clone())
        {
            arrivals.remove(&oldest);
        }
        arrivals
            .entry(id.clone())
            .or_insert_with(|| motion::LiveBlockArrivals::baseline(blocks))
            .observed();
    }

    /// Folds the items, run and UI toggles into the transcript's rows,
    /// then publishes them (see [`Self::reexpand`]).
    fn rebuild(&mut self, cx: &mut Context<Self>) {
        let run = self.run(cx);
        if let (Some(view), Some(run)) = (self.view.as_mut(), &run) {
            view.remember_run(run.clone());
        }
        let folded = self
            .view
            .as_ref()
            .map(|view| fold::fold(view, run.as_ref(), self.agent.as_ref()));
        let (rows, runs) = folded
            .map(|folded| (folded.rows, folded.runs))
            .unwrap_or_default();
        if let Some(view) = &self.view {
            let resource_owners: Vec<_> = view
                .items
                .values()
                .chain(view.context.values())
                .flat_map(|item| {
                    if self.bodies.agent_scope {
                        vec![
                            vec![0, item.position],
                            vec![1, item.position],
                            vec![2, item.position],
                        ]
                    } else {
                        vec![vec![item.position]]
                    }
                })
                .collect();
            self.bodies
                .markdown
                .retain_resource_owners(&resource_owners, cx);
            self.bodies.retain(|id| view.item(id).is_some());
        }
        let rows = self
            .view
            .as_ref()
            .map(|view| fold::expanded(view, rows.clone(), &runs))
            .unwrap_or(rows);
        self.folded = (rows, runs);
        self.sync_goal(cx);
        self.sync_questionnaires(cx);
        let live = run.is_some_and(|run| run.status.is_live());
        if live && self.clock.is_none() {
            self.clock = Some(cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(Duration::from_secs(1)).await;
                    if this
                        .update(cx, |this, cx| {
                            let now = artisan_api::now();
                            this.row_context.now = now;
                            for row in this.row_views.iter().flatten() {
                                row.update(cx, |row, cx| row.tick_header(now, cx));
                            }
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            }));
        } else if !live {
            self.clock = None;
        }
        self.reexpand(cx);
    }

    /// Publishes the folded rows with each text body split into its
    /// prepared blocks and native windows: hands changed rows their new
    /// model and tells the list which rows came and went. Unchanged rows
    /// keep their views and stay cached. A body's own preparation re-expands
    /// only that body (see [`Self::respace`]).
    pub(crate) fn reexpand(&mut self, cx: &mut Context<Self>) {
        let run = self.run(cx);
        let mut rows = Vec::new();
        for (row, version) in self.folded.0.clone() {
            rows.extend(self.body_rows(row, version));
        }
        let rows = self.retain_disclosure_rows(rows);
        let previous = std::mem::replace(&mut self.rows, rows);
        self.prune_sources();
        let rows = std::mem::replace(&mut self.rows, previous);
        let rows = self.project_native_windows(rows);
        self.index_disclosure_rows(&rows);
        // A disclosure in flight keeps its group to the rows it measured.
        let rows: Vec<_> = rows
            .into_iter()
            .filter(|(row, _)| {
                self.disclosures
                    .iter()
                    .all(|(header, flight)| flight.admits(header, row))
            })
            .collect();
        self.publish(rows, run.as_ref(), cx);
    }

    /// Publishes a whole new set of rows and what they share.
    fn publish(&mut self, rows: Vec<(Row, u64)>, run: Option<&Run>, cx: &mut Context<Self>) {
        let agents = self.view.as_ref().map(agent_names).unwrap_or_default();
        #[expect(
            clippy::collection_is_never_read,
            reason = "the sorted names are hashed below, a read the lint does not see"
        )]
        let mut sorted: Vec<_> = agents.iter().collect();
        sorted.sort();
        let mut hasher = DefaultHasher::new();
        sorted.hash(&mut hasher);
        self.row_context.agents_version = hasher.finish();
        self.row_context.agents = Arc::new(agents);
        self.row_context.runs = self.folded.1.clone();
        self.row_context.now = artisan_api::now();
        self.row_context.end_space = self.end_space();
        self.row_context.read_boundary = self.view.as_ref().and_then(|view| {
            if self.agent.is_some() || view.window().gap.is_some() {
                return None;
            }
            // The reader sees the run through when its last row, in thread
            // order, has painted. A call settled late (a background job
            // cancelled as the run ends) is newer but sits higher up: it
            // must not keep the thread unread. A run that failed before
            // writing anything ends at the message that started it. A note
            // stored after the run settles has no row to paint.
            let rowed = || view.shown().filter(|item| !fold::in_composer_strip(item));
            let last = rowed().max_by_key(|item| item.position)?;
            let newest = rowed().map(|item| item.seq).max()?;
            let run = run?;
            let ends_run = last.run_id.as_ref() == Some(&run.id)
                || rowed().all(|item| item.run_id.as_ref() != Some(&run.id));
            (!run.status.is_live() && ends_run && run.seq >= newest)
                .then(|| (last.id.clone(), run.seq))
        });
        let len = self.rows.len();
        self.splice_rows(0..len, rows, cx);
    }

    /// Replaces the rows in `range` with `rows`: the list learns which came
    /// and went there, rows that stay keep their views, and only the rows
    /// whose model changed (the new ones and their neighbours' spacing)
    /// draw again.
    pub(crate) fn splice_rows(
        &mut self,
        range: std::ops::Range<usize>,
        rows: Vec<(Row, u64)>,
        cx: &mut Context<Self>,
    ) {
        let old_keys: Vec<RowKey> = self.rows[range.clone()]
            .iter()
            .map(|(row, _)| row.key())
            .collect();
        let new_keys: Vec<RowKey> = rows.iter().map(|(row, _)| row.key()).collect();
        // The row the reader is on going away (a body splitting into its
        // blocks) keeps the reader on what replaced it.
        let top = self.list.logical_scroll_top();
        let replaced = !self.list.is_following_tail()
            && range.contains(&top.item_ix)
            && !new_keys.contains(&old_keys[top.item_ix - range.start])
            && !new_keys.is_empty();
        splice(&self.list, range.start, &old_keys, &new_keys);
        if replaced {
            self.list.scroll_to(ListOffset {
                item_ix: range.start,
                offset_in_item: px(0.0),
            });
        }
        let mut kept: HashMap<RowKey, Entity<RowView>> = old_keys
            .into_iter()
            .zip(self.row_views.drain(range.clone()))
            .filter_map(|(key, view)| Some((key, view?)))
            .collect();
        let views: Vec<_> = new_keys.iter().map(|key| kept.remove(key)).collect();
        for key in kept.into_keys() {
            self.forget_height(&key);
        }
        let count = rows.len();
        self.rows.splice(range.clone(), rows);
        self.row_views.splice(range.start..range.start, views);
        let context = &mut self.row_context;
        context
            .gaps
            .splice(range.clone(), std::iter::repeat_n(0.0, count));
        context
            .versions
            .splice(range.clone(), std::iter::repeat_n(0, count));
        let mut item_rows: HashMap<ItemId, Vec<usize>> = HashMap::new();
        let mut key_rows = HashMap::new();
        let mut status_rows: HashMap<artisan_api::RunId, Vec<usize>> = HashMap::new();
        for (index, (row, _)) in self.rows.iter().enumerate() {
            key_rows.insert(row.key(), index);
            if let Some(id) = row.anchor() {
                item_rows.entry(id.clone()).or_default().push(index);
            }
            if let Row::Status(run) = row {
                status_rows.entry(run.clone()).or_default().push(index);
            }
        }
        context.item_rows = item_rows;
        context.key_rows = key_rows;
        context.status_rows = status_rows;
        // The rows around the splice change spacing; the last row carries
        // the end space.
        let end = (range.start + count + 1).min(self.rows.len());
        let mut changed: Vec<usize> = (range.start.saturating_sub(1)..end).collect();
        if let Some(last) = self.rows.len().checked_sub(1)
            && !changed.contains(&last)
        {
            changed.push(last);
        }
        for index in changed {
            self.update_row(index, cx);
        }
        self.navigator_markers = None;
        self.row_cache_order = self
            .row_views
            .iter()
            .enumerate()
            .filter_map(|(index, view)| view.as_ref().map(|_| index))
            .collect();
        self.publish_reveal(cx);
        cx.notify();
    }

    /// Recomputes one row's spacing and version, and hands its view a new
    /// model when what it shows changed.
    fn update_row(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(view) = self.view.as_ref() else {
            return;
        };
        let gap = fold::gap(view, &self.rows, index);
        let end = if index + 1 == self.rows.len() {
            self.row_context.end_space
        } else {
            0.0
        };
        let (row, base) = self.rows[index].clone();
        let images = self.row_images(&row, cx);
        let mut hasher = DefaultHasher::new();
        base.hash(&mut hasher);
        self.row_context.agents_version.hash(&mut hasher);
        (gap.to_bits(), end.to_bits()).hash(&mut hasher);
        let mut image_order: Vec<_> = images.iter().collect();
        image_order.sort_by_key(|(hash, _)| *hash);
        for (hash, blob) in image_order {
            (hash, std::mem::discriminant(blob)).hash(&mut hasher);
        }
        let version = hasher.finish();
        self.row_context.gaps[index] = gap;
        self.row_context.versions[index] = version;
        if let Some(row_view) = self.row_views[index].clone()
            && row_view.read(cx).version() != version
        {
            let model = self.model(index, images, cx);
            row_view.update(cx, |row_view, cx| row_view.set(model, version, cx));
            self.list.remeasure_items(index..index + 1);
        }
    }

    /// A snapshot of what the row at `index` shows.
    fn model(
        &mut self,
        index: usize,
        images: HashMap<BlobHash, Blob>,
        cx: &mut Context<Self>,
    ) -> RowModel {
        let row = self.rows[index].0.clone();
        let text = self.text_body(&row, cx);
        let context = &self.row_context;
        let mut model = RowModel {
            agents: Arc::clone(&context.agents),
            run: row.run().and_then(|run| context.runs.get(run)).cloned(),
            items: HashMap::new(),
            live: HashMap::new(),
            ui: HashMap::new(),
            images,
            now: context.now,
            gap: context.gaps.get(index).copied().unwrap_or_default(),
            end: if index + 1 == self.rows.len() {
                context.end_space
            } else {
                0.0
            },
            text,
            row,
        };
        let Some(view) = self.view.as_ref() else {
            return model;
        };
        let ids: Vec<ItemId> = match &model.row {
            Row::Work { .. } => Vec::new(),
            Row::Chain { ids, .. } => ids.clone(),
            other @ (Row::Projected { .. }
            | Row::EarlierWork { .. }
            | Row::WorkEntry { .. }
            | Row::Text { .. }
            | Row::Prompt(_)
            | Row::Compaction(_)
            | Row::Image(_)
            | Row::Gap
            | Row::Status(_)
            | Row::AgentResult(_)
            | Row::AgentStatus(..)) => other.anchor().cloned().into_iter().collect(),
        };
        for id in ids {
            if let Some(item) = view.shared_item(&id) {
                model.items.insert(id.clone(), item);
            }
            if let Some(ui) = view.ui.get(&id) {
                model.ui.insert(id, ui.clone());
            }
        }
        model.live = view
            .live
            .iter()
            .filter(|(target, _)| match target {
                artisan_api::LiveTarget::Text { item_id }
                | artisan_api::LiveTarget::Shell { item_id }
                | artisan_api::LiveTarget::Progress { item_id } => {
                    model.items.contains_key(item_id)
                }
                artisan_api::LiveTarget::Reasoning { run_id }
                | artisan_api::LiveTarget::Status { run_id } => model.row.run() == Some(run_id),
            })
            .map(|(target, text)| (target.clone(), Arc::clone(text)))
            .collect();
        model
    }

    /// A new view for the row at `index`, as the list first shows it.
    pub(crate) fn new_row_view(&mut self, index: usize, cx: &mut Context<Self>) -> Entity<RowView> {
        let row = self.rows[index].0.clone();
        let images = self.row_images(&row, cx);
        let model = self.model(index, images, cx);
        let version = self
            .row_context
            .versions
            .get(index)
            .copied()
            .unwrap_or_default();
        let prose_fresh = match row.semantic() {
            Row::Text {
                id,
                slot: Slot::Prose,
                block,
                ..
            } => self.bodies.arrivals.get_mut(id).map_or_else(
                || model.ui.get(id).is_some_and(thread::ItemUi::fresh),
                |arrival| arrival.claim(*block),
            ),
            Row::Projected { .. }
            | Row::Work { .. }
            | Row::EarlierWork { .. }
            | Row::WorkEntry { .. }
            | Row::Chain { .. }
            | Row::Text { .. }
            | Row::Prompt(_)
            | Row::Compaction(_)
            | Row::Image(_)
            | Row::Gap
            | Row::Status(_)
            | Row::AgentResult(_)
            | Row::AgentStatus(..) => false,
        };
        let env = RowEnv {
            theme: self.theme,
            conversation: cx.entity().downgrade(),
            markdown: Rc::clone(&self.bodies.markdown),
            reveal: Rc::clone(&self.reveal),
            scope: self.bodies.scope.clone(),
            agent_scope: self.bodies.agent_scope,
            host: self.host(cx).map(|host| host.downgrade()),
        };
        let disclosure = self.row_disclosure(&row);
        cx.new(|_| RowView::new(model, version, env, prose_fresh, disclosure))
    }

    /// The cached images a row shows: its anchor's,
    /// and in a work section every item's (a steer sent with images sits
    /// there, not on a row of its own).
    fn row_images(&self, row: &Row, cx: &Context<Self>) -> HashMap<BlobHash, Blob> {
        let ids: Vec<ItemId> = match row {
            Row::Work { .. } | Row::Chain { .. } => Vec::new(),
            other @ (Row::Projected { .. }
            | Row::EarlierWork { .. }
            | Row::WorkEntry { .. }
            | Row::Text { .. }
            | Row::Prompt(_)
            | Row::Compaction(_)
            | Row::Image(_)
            | Row::Gap
            | Row::Status(_)
            | Row::AgentResult(_)
            | Row::AgentStatus(..)) => other.anchor().cloned().into_iter().collect(),
        };
        ids.iter()
            .filter_map(|id| self.view.as_ref()?.item(id))
            .flat_map(|item| match &item.kind {
                ItemKind::UserMessage { images, .. } => images.clone(),
                ItemKind::Image { blob, .. } => vec![blob.clone()],
                ItemKind::AssistantMessage { .. }
                | ItemKind::Tool { .. }
                | ItemKind::Agent { .. }
                | ItemKind::Question { .. }
                | ItemKind::Plan { .. }
                | ItemKind::Compaction { .. }
                | ItemKind::Notice { .. }
                | ItemKind::Error { .. } => Vec::new(),
            })
            .filter_map(|hash| {
                let blob = self.host(cx).and_then(|host| host.read(cx).image(&hash));
                Some((hash, blob?))
            })
            .collect()
    }

    fn toggle_work(&mut self, run: &artisan_api::RunId, open: bool, cx: &mut Context<Self>) {
        if !cx.reduce_motion() {
            let headers: Vec<_> = self
                .rows
                .iter()
                .filter_map(|(row, _)| match row {
                    Row::Work {
                        run: Some(row_run), ..
                    } if row_run == run => Some(row.key()),
                    Row::Projected { .. }
                    | Row::Work { .. }
                    | Row::EarlierWork { .. }
                    | Row::WorkEntry { .. }
                    | Row::Chain { .. }
                    | Row::Text { .. }
                    | Row::Prompt(_)
                    | Row::Compaction(_)
                    | Row::Image(_)
                    | Row::Gap
                    | Row::Status(_)
                    | Row::AgentResult(_)
                    | Row::AgentStatus(..) => None,
                })
                .collect();
            for header in headers {
                self.start_disclosure(header, open);
            }
        }
        if let Some(view) = self.view.as_mut() {
            view.work_open.insert(run.clone(), open);
            self.rebuild(cx);
        }
    }

    fn toggle_chain(&mut self, id: &ItemId, open: bool, cx: &mut Context<Self>) {
        if !cx.reduce_motion() {
            self.start_disclosure(RowKey::Chain(id.clone()), open);
        }
        if let Some(view) = self.view.as_mut() {
            view.ui.entry(id.clone()).or_default().chain_open = Some(open);
            self.rebuild(cx);
        }
    }

    fn thread_composer(&self, cx: &App) -> Option<Entity<Composer>> {
        let (host, thread) = self.open.as_ref()?;
        let composer = self.composer.as_ref()?;
        matches!(composer.read(cx).target(), ComposerTarget::Thread { host: target_host, thread: target_thread }
            if target_host == host && target_thread == thread).then(|| composer.clone())
    }

    /// The questions waiting on the user, oldest first, to the composer,
    /// where they are answered.
    fn sync_questionnaires(&self, cx: &mut Context<Self>) {
        let Some(composer) = self.thread_composer(cx) else {
            return;
        };
        let Some(summary) = self.host(cx).and_then(|host| {
            let thread = &self.view.as_ref()?.thread;
            host.read(cx).thread_summary(thread)?.value.clone()
        }) else {
            return;
        };
        let open = summary
            .questions
            .iter()
            .map(|question| ComposerQuestionnaire {
                item: question.id.clone(),
                questions: question.questions.clone(),
            })
            .collect();
        composer.update(cx, |composer, cx| composer.set_questionnaires(open, cx));
    }

    /// The thread's goal, as its latest goal notice left it, and when a
    /// parked turn is picked up again, on the composer's strip.
    fn sync_goal(&self, cx: &mut Context<Self>) {
        self.sync_note(cx);
        let Some(composer) = self.thread_composer(cx) else {
            return;
        };
        let parked = self.host(cx).and_then(|host| {
            let thread = &self.view.as_ref()?.thread;
            host.read(cx).threads.get(thread)?.parked_until
        });
        composer.update(cx, |composer, cx| composer.set_parked(parked, cx));
        let Some(summary) = self.host(cx).and_then(|host| {
            let thread = &self.view.as_ref()?.thread;
            host.read(cx).thread_summary(thread)?.value.clone()
        }) else {
            return;
        };
        let goal = summary.goal.as_ref().map(|goal| ComposerGoal {
            objective: goal.summary.clone().into(),
            state: match goal.state {
                artisan_api::ThreadGoalState::Active => ComposerGoalState::Active,
                artisan_api::ThreadGoalState::Paused => ComposerGoalState::Paused,
                artisan_api::ThreadGoalState::Blocked => ComposerGoalState::Blocked,
                artisan_api::ThreadGoalState::UsageLimited => ComposerGoalState::UsageLimited,
                artisan_api::ThreadGoalState::BudgetLimited => ComposerGoalState::BudgetLimited,
            },
            detail: None,
        });
        // The first page of items carries the latest turn's note.
        let settled = self
            .view
            .as_ref()
            .is_some_and(|view| view.window().loaded());
        composer.update(cx, |composer, cx| {
            composer.set_goal(goal, cx);
            if settled {
                composer.settle_strip();
            }
        });
    }

    /// The latest turn's note on the composer's strip, ahead of the goal;
    /// gone once another turn starts.
    fn sync_note(&self, cx: &mut Context<Self>) {
        let Some(composer) = self.thread_composer(cx) else {
            return;
        };
        let latest = self.run(cx).map(|run| run.id);
        let note = self.view.as_ref().and_then(|view| {
            view.shown().rev().find_map(|item| match &item.kind {
                ItemKind::Notice {
                    kind: artisan_api::NoticeKind::YouShouldKnow { heads_up },
                    text,
                } if item.run_id.is_some() && item.run_id == latest => Some(ComposerNote {
                    id: item.id.to_string().into(),
                    heads_up: *heads_up,
                    text: one_line(text.inline().unwrap_or_default()).into(),
                }),
                ItemKind::UserMessage { .. }
                | ItemKind::AssistantMessage { .. }
                | ItemKind::Tool { .. }
                | ItemKind::Agent { .. }
                | ItemKind::Question { .. }
                | ItemKind::Plan { .. }
                | ItemKind::Image { .. }
                | ItemKind::Compaction { .. }
                | ItemKind::Notice { .. }
                | ItemKind::Error { .. } => None,
            })
        });
        composer.update(cx, |composer, cx| composer.set_note(note, cx));
    }

    /// The space under the last row (`TRANSCRIPT_END_SPACE_PX`, or more
    /// when the composer card is taller): the latest words always clear
    /// the floating composer.
    fn end_space(&self) -> f32 {
        const END_SPACE: f32 = 192.0;
        // No composer floats over the transcript (none yet, or an agent's
        // view, which hides it): only the gap.
        if self.composer.is_none() || self.agent.is_some() {
            return render::COMPOSER_GAP;
        }
        END_SPACE.max(self.composer_height + render::COMPOSER_BOTTOM + render::COMPOSER_GAP)
    }

    /// The composer card's measured height changed: the end space follows.
    fn composer_measured(&mut self, height: f32, cx: &mut Context<Self>) {
        if (height - self.composer_height).abs() >= 0.5 {
            self.composer_height = height;
            self.reexpand(cx);
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
            this.update(cx, |this, cx| {
                if let Some(ui) = this.view.as_mut().and_then(|view| view.ui.get_mut(&item)) {
                    ui.sending = false;
                    if let Err(error) = &result {
                        tracing::warn!(item = %item, "couldn't send the answer: {error:#}");
                        ui.error = Some(format!("Couldn't send the answer: {error:#}"));
                    }
                }
                this.rebuild(cx);
            })
            .ok();
        })
        .detach();
    }

    fn retry_image(&mut self, hash: &BlobHash, cx: &mut Context<Self>) {
        let Some(host) = self.host(cx) else {
            return;
        };
        host.update(cx, |host, cx| host.load_image(hash.clone(), true, cx));
        self.rebuild(cx);
    }

    fn load_image(&self, hash: &BlobHash, cx: &mut Context<Self>) {
        let Some(host) = self.host(cx) else {
            return;
        };
        host.update(cx, |host, cx| host.load_image(hash.clone(), false, cx));
    }
}

/// The thread's agents by their item, named by their task: the name work
/// rows give them ("Messaged Finish the rework").
fn agent_names(view: &ThreadView) -> HashMap<ItemId, String> {
    view.items
        .values()
        .chain(view.context.values())
        .filter_map(|item| {
            let ItemKind::Agent { description, .. } = &item.kind else {
                return None;
            };
            let task = description
                .inline()
                .and_then(|text| text.lines().map(str::trim).find(|line| !line.is_empty()))
                .map(str::to_owned)
                .or_else(|| item.agent_label())?;
            Some((item.id.clone(), task))
        })
        .collect()
}

/// Tells the list which rows came and went between two key orders.
fn splice(list: &ListState, offset: usize, old: &[RowKey], new: &[RowKey]) {
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
            list.splice(offset + j..offset + j + removed, 0);
            i += removed;
            continue;
        }
        let inserted = new[j..]
            .iter()
            .take_while(|key| !old_set.contains(key))
            .count();
        if inserted > 0 {
            list.splice(offset + j..offset + j, inserted);
            j += inserted;
            continue;
        }
        // Rows changed order (rare): replace the rest.
        list.splice(offset + j..offset + j + (old.len() - i), new.len() - j);
        break;
    }
}

/// A note's markdown explanation as one line for the strip's ticker: list
/// marks and line breaks become sentence breaks.
fn one_line(text: &str) -> String {
    text.lines()
        .map(|line| {
            line.trim()
                .trim_start_matches(['-', '*', '•'])
                .trim()
                .replace("**", "")
                .replace('`', "")
        })
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("  ·  ")
}
