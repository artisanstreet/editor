//! The open thread. One copy of its items, a viewport, and a pure render.
//!
//! Items come from `GET /threads/{id}/items` pages and the host's event
//! stream and are replaced by id; immutable live Rope snapshots are shared with the host. The
//! transcript is a fold of items + live text + UI toggles into rows, shown
//! in a GPUI list.

mod activity;
mod admission;
mod collections;
mod content;
mod disclosure;
mod fold;
mod motion;
mod phase;
mod render;
mod row;
mod source_actions;
mod thread;
mod viewport;

#[cfg(test)]
mod tests;

pub use phase::{Phase, turn_phase};

use artisan_api::{Answer, BlobHash, Item, ItemId, ItemKind, Run, ThreadId};
use editor_composer::{
    Composer, ComposerEvent, ComposerGoal, ComposerGoalState, ComposerQuestionnaire, ComposerTarget,
};
use editor_core::{Core, CoreEvent, HostData, HostEvent, HostId, Screen};
use editor_ui::markdown_renderer::MarkdownRenderer;
use editor_ui::theme::{ArtisanTheme, ThemeMode};
use editor_ui::wheel_glide::WheelGlide;
use fold::{Row, RowKey};
use gpui::{
    App, AppContext as _, Context, Entity, EventEmitter, FollowMode, ListAlignment, ListOffset,
    ListState, ScrollWheelEvent, SharedString, Subscription, Task, Window, px,
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
    agents: Arc<HashMap<String, String>>,
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
    open: Option<(HostId, ThreadId)>,
    view: Option<ThreadView>,
    /// The folded rows and, in the same order, their views.
    rows: Vec<(Row, u64)>,
    row_views: Vec<Option<Entity<RowView>>>,
    row_context: RowContext,
    disclosures: HashMap<RowKey, disclosure::Flight>,
    admitted_frontier: Option<RowKey>,
    reveal_rows_deferred: bool,
    row_cache_order: std::collections::VecDeque<usize>,
    list: ListState,
    composer: Option<Entity<Composer>>,
    composer_events: Option<Subscription>,
    shared: Rc<Shared>,
    theme: ArtisanTheme,
    /// Why the last page failed to load.
    load_error: Option<String>,
    /// Height of the composer card floating over the transcript; the end
    /// space under the last row clears it.
    composer_height: f32,
    /// The pointer is over the turn navigator (rail or menu).
    navigator_hovered: bool,
    navigator_hover: Rc<RefCell<editor_ui::picker_motion::SlidingHoverState>>,
    navigator_hover_surface: Rc<RefCell<Option<gpui::Bounds<gpui::Pixels>>>>,
    /// Rebuilt only when transcript rows change; labels retain bounded first lines.
    navigator_markers: Option<Arc<[render::Marker]>>,
    navigator_labels: HashMap<ItemId, (artisan_api::Seq, SharedString)>,
    /// Per tick (by message), whether it was last active and when that
    /// changed, while it eases between its two looks.
    navigator_ticks: HashMap<ItemId, (bool, Option<std::time::Instant>)>,
    /// Whether a tick was active last frame: the first one placed does not
    /// ease in.
    navigator_had_active: bool,
    /// Invalidates page responses across host/thread reopen and stream reset.
    open_generation: u64,
    page_task: Option<Task<()>>,
    #[cfg(test)]
    page_completion_gate: Option<(
        Arc<std::sync::atomic::AtomicBool>,
        futures::channel::oneshot::Receiver<bool>,
    )>,
    #[cfg(test)]
    page_requests: usize,
    agent_task: Option<Task<()>>,
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
                &this.shared.selection_scope.borrow(),
                selection_window,
                cx,
            );
            this.shared.markdown.clear(cx);
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
                let _ = this.update(cx, |this, cx| this.scrolled(top, cx));
            });
        });
        let theme = ArtisanTheme::for_mode(ThemeMode::Dark);
        let shared = Rc::new(Shared {
            reveal_queue: RefCell::default(),
            prose_arrivals: RefCell::default(),
            row_heights: RefCell::default(),
            disclosure_clips: RefCell::default(),
            disclosure_hidden: RefCell::default(),
            disclosure_fractions: RefCell::default(),
            markdown: Rc::new(MarkdownRenderer::new().with_prose_measure(px(row::PROSE_MEASURE))),
            theme,
            conversation: cx.entity().downgrade(),
            sources: RefCell::default(),
            prose_shapes: RefCell::default(),
            stored_sources: RefCell::default(),
            selection_scope: RefCell::default(),
            agent_scope: std::cell::Cell::new(false),
            plain_sources: RefCell::default(),
            literal_sources: RefCell::default(),
            native_windows: RefCell::default(),
            source_frontiers: RefCell::default(),
        });
        let mut conversation = Self {
            core,
            agent: None,
            open: None,
            view: None,
            rows: Vec::new(),
            row_views: Vec::new(),
            row_context: RowContext::default(),
            disclosures: HashMap::new(),
            admitted_frontier: None,
            reveal_rows_deferred: false,
            row_cache_order: std::collections::VecDeque::new(),
            list,
            composer: None,
            composer_events: None,
            shared,
            theme,
            load_error: None,
            composer_height: 0.0,
            navigator_hovered: false,
            navigator_hover: Rc::default(),
            navigator_hover_surface: Rc::default(),
            navigator_markers: None,
            navigator_labels: HashMap::new(),
            navigator_ticks: HashMap::new(),
            navigator_had_active: false,
            open_generation: 0,
            page_task: None,
            #[cfg(test)]
            page_completion_gate: None,
            #[cfg(test)]
            page_requests: 0,
            agent_task: None,
            frame_started: Rc::default(),
            glide: WheelGlide::default(),
            glide_scheduled: false,
            arrival: None,
            clock: None,
            host_events: None,
            _core_events: core_events,
            window: WindowState::of(window),
            selection_window,
            _activation: activation,
            _release: release,
        };
        conversation.sync_screen(window, cx);
        conversation
    }

    fn update_selection_scope(&self) {
        self.shared.agent_scope.set(self.agent.is_some());
        if let Some((host, thread)) = &self.open {
            let scope: SharedString = format!(
                "{host}/{thread}/{}",
                self.agent.as_ref().map_or("root", ItemId::as_str)
            )
            .into();
            self.shared.markdown.set_selection_scope(scope.clone());
            *self.shared.selection_scope.borrow_mut() = scope;
        }
    }
    fn release_selection(&self, cx: &mut App) {
        editor_ui::selectable_text::release_selection_scope(
            &self.shared.selection_scope.borrow(),
            self.selection_window,
            cx,
        );
        self.shared.plain_sources.borrow_mut().clear();
        self.shared.literal_sources.borrow_mut().clear();
        self.shared.native_windows.borrow_mut().clear();
        self.shared.source_frontiers.borrow_mut().clear();
    }

    /// The thread on screen, if any.
    #[must_use]
    pub fn open_thread(&self) -> Option<&(HostId, ThreadId)> {
        self.open.as_ref()
    }

    /// The loaded items of the open thread, in order.
    pub fn items(&self) -> impl Iterator<Item = &Item> {
        self.view
            .iter()
            .flat_map(|view| view.items.values().map(Arc::as_ref))
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

    /// Loading only until the first usable content or empty tail arrives.
    pub(crate) fn landing(&self) -> bool {
        self.agent.is_none()
            && self.load_error.is_none()
            && self
                .view
                .as_ref()
                .is_some_and(|view| view.items.is_empty() && view.window().in_flight)
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
            _ => None,
        }))
    }

    pub fn open_agent(&mut self, agent: Option<ItemId>, cx: &mut Context<Self>) {
        if self.agent == agent {
            return;
        }
        self.open_generation = self.open_generation.wrapping_add(1);
        self.page_task = None;
        self.agent_task = None;
        self.load_error = None;
        self.release_selection(cx);
        self.shared.markdown.clear(cx);
        self.shared.sources.borrow_mut().clear();
        self.shared.stored_sources.borrow_mut().clear();
        self.shared.prose_shapes.borrow_mut().clear();
        self.shared.prose_arrivals.borrow_mut().clear();
        self.disclosures.clear();
        self.admitted_frontier = None;
        self.reveal_rows_deferred = false;
        self.shared.disclosure_clips.borrow_mut().clear();
        self.shared.disclosure_hidden.borrow_mut().clear();
        self.shared.disclosure_fractions.borrow_mut().clear();
        self.shared.row_heights.borrow_mut().clear();
        let context = agent
            .as_ref()
            .and_then(|id| self.view.as_ref()?.shared_item(id));
        self.agent = agent.clone();
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
            let generation = self.open_generation;
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
                            next = item.parent_id.clone();
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
                let _ = this.update(cx, |this, cx| {
                    if this.open_generation != generation
                        || this.open != open
                        || this.agent.as_ref() != Some(&requested_agent)
                    {
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
                });
            }));
        }
        // Another transcript: build it fresh and land on its end.
        self.rows.clear();
        self.navigator_markers = None;
        self.navigator_labels.clear();
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

    fn clear_page_frontiers(&self) {
        self.shared.source_frontiers.borrow_mut().clear();
        for state in self.shared.native_windows.borrow_mut().values_mut() {
            state.frontier = None;
        }
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
        self.open_generation = self.open_generation.wrapping_add(1);
        self.page_task = None;
        self.agent_task = None;
        self.release_selection(cx);
        self.shared.markdown.clear(cx);
        self.shared.sources.borrow_mut().clear();
        self.shared.stored_sources.borrow_mut().clear();
        self.shared.prose_shapes.borrow_mut().clear();
        self.shared.prose_arrivals.borrow_mut().clear();
        self.disclosures.clear();
        self.admitted_frontier = None;
        self.reveal_rows_deferred = false;
        self.shared.disclosure_clips.borrow_mut().clear();
        self.shared.disclosure_hidden.borrow_mut().clear();
        self.shared.disclosure_fractions.borrow_mut().clear();
        self.shared.row_heights.borrow_mut().clear();
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
        self.shared.markdown.set_media_fetcher(Some(
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
                            .map(|bytes| std::sync::Arc::<[u8]>::from(bytes))
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
        self.navigator_labels.clear();
        self.row_views.clear();
        self.row_cache_order.clear();
        self.perform(effects, cx);
        self.rebuild(cx);
        cx.emit(ConversationEvent::ItemsChanged);
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
                let directed = self.agent.as_ref().and_then(|agent| view.item(agent)).is_some_and(|agent| {
                    matches!((&agent.kind, &item.kind),
                        (ItemKind::Agent { agent_id: Some(address), .. }, ItemKind::Tool { tool: artisan_api::ToolKind::AgentMessage { recipient, .. }, .. }) if address == recipient)
                });
                if item.thread_id != thread
                    || (item.parent_id != self.agent
                        && !view.context.contains_key(&item.id)
                        && !directed)
                {
                    return;
                }
                let changed = view.apply_item(item.clone(), watched);
                view.set_live(host.read(cx).live_targets(&thread));
                if changed {
                    if watched && matches!(item.kind, ItemKind::AssistantMessage { .. }) {
                        self.observe_live_prose_arrival(&item.id);
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
            HostEvent::ImagesChanged => self.rebuild(cx),
            HostEvent::Live { thread_id, target } => {
                if *thread_id == thread {
                    let text = host
                        .read(cx)
                        .live_snapshot(&thread, target)
                        .map(|snapshot| snapshot.text);
                    if view.update_live(target, text) {
                        if watched {
                            if let artisan_api::LiveTarget::Text { item_id } = target {
                                self.observe_live_prose_arrival(item_id);
                            }
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
            HostEvent::EngineInstall { .. } => {}
            HostEvent::Changed => cx.notify(),
            HostEvent::Reset => {
                self.open_generation = self.open_generation.wrapping_add(1);
                self.page_task = None;
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

    fn fetch(&mut self, fetch: Fetch, cx: &mut Context<Self>) {
        let (Some(host), Some(view)) = (self.host(cx), self.view.as_ref()) else {
            return;
        };
        let client = host.read(cx).client.clone();
        let thread = view.thread.clone();
        let query = thread::query(fetch, thread::PAGE);
        let open = self.open.clone();
        let generation = self.open_generation;
        let agent = self.agent.clone();
        #[cfg(test)]
        let completion_gate = {
            self.page_requests += 1;
            self.page_completion_gate.take()
        };
        self.page_task = Some(cx.spawn(async move |this, cx| {
            let result = match agent.as_ref() {
                Some(agent) => client.agent_items(&thread, agent, &query).await,
                None => client.items(&thread, &query).await,
            };
            #[cfg(test)]
            let result = if let Some((arrived, release)) = completion_gate {
                arrived.store(true, std::sync::atomic::Ordering::Release);
                if release.await.unwrap_or(false) {
                    Err(anyhow::anyhow!("injected page failure"))
                } else {
                    result
                }
            } else {
                result
            };
            let _ = this.update(cx, |this, cx| {
                if this.open_generation != generation || this.open != open {
                    return;
                }
                let Some(view) = this.view.as_mut().filter(|view| view.thread == thread) else {
                    return;
                };
                // A page can replace the work row at the loaded edge. Native
                // splice cannot keep an anchor whose key disappears; retain the
                // first surviving visible row's measured position instead.
                let anchors: Vec<_> = if matches!(fetch, Fetch::Older { .. } | Fetch::Newer { .. })
                    && !this.list.is_following_tail()
                {
                    let viewport = this.list.viewport_bounds();
                    this.rows
                        .iter()
                        .enumerate()
                        .filter_map(|(index, (row, _))| {
                            let bounds = this.list.bounds_for_item(index)?;
                            (bounds.bottom() > viewport.top() && bounds.top() < viewport.bottom())
                                .then(|| {
                                    (
                                        row.key(),
                                        row.anchor().cloned(),
                                        viewport.top() - bounds.top(),
                                    )
                                })
                        })
                        .collect()
                } else {
                    Vec::new()
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
                if let Some((item_ix, offset_in_item)) =
                    anchors.iter().find_map(|(key, item, offset)| {
                        this.rows
                            .iter()
                            .position(|(row, _)| row.key() == *key && row.anchor() == item.as_ref())
                            .map(|index| (index, *offset))
                    })
                {
                    this.list.scroll_to(ListOffset {
                        item_ix,
                        offset_in_item,
                    });
                }
                this.perform(effects, cx);
                cx.emit(ConversationEvent::ItemsChanged);
            });
        }));
    }

    /// Glides a wheel tick over the transcript. Trackpad
    /// deltas and reduced motion go to the list, which moves directly.
    pub(crate) fn transcript_wheel(
        &mut self,
        event: &ScrollWheelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let max = f32::from(self.list.max_offset_for_scrollbar().y).max(0.0);
        if max <= 0.0 {
            if f32::from(event.delta.pixel_delta(window.line_height()).y) > 0.0
                && let Some(anchor) = self.view.as_ref().and_then(|view| view.window().oldest)
            {
                // Even a short list has an explicit reader position when the
                // reader asks for older content. Stop native tail following
                // before that page changes the height of the transcript.
                self.list.scroll_to(self.list.logical_scroll_top());
                self.viewport(vec![Input::ScrolledAway { anchor }, Input::TopReached], cx);
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
        let source_prefix = self.rows.first().is_some_and(|(row, _)| {
            row.visual().is_some_and(|visual| visual > 0)
                || match row.semantic() {
                    Row::PlainBlock { id, section, block } => {
                        *block > 0
                            && self
                                .shared
                                .stored_sources
                                .borrow()
                                .contains_key(&(id.clone(), section.slot()))
                    }
                    Row::ProseBlock { id, block } => {
                        *block > 0
                            && self
                                .shared
                                .stored_sources
                                .borrow()
                                .contains_key(&(id.clone(), "prose"))
                    }
                    Row::TextBlock {
                        id, section, block, ..
                    } => {
                        *block > 0
                            && self
                                .shared
                                .stored_sources
                                .borrow()
                                .contains_key(&(id.clone(), section.slot()))
                    }
                    _ => false,
                }
        });
        let Some(view) = self.view.as_mut() else {
            return;
        };
        let mut inputs = Vec::new();
        // Root and agent scopes page through the same bounded window.
        if wants_older(&self.list) && !source_prefix {
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
        };
        for index in indices.into_iter().flatten() {
            if let Some(row) = self.row_views.get(*index).and_then(Option::as_ref) {
                row.update(cx, |row, cx| row.set_live(target, text.as_ref(), cx));
                self.list.remeasure_items(*index..*index + 1);
            }
        }
        cx.notify();
    }

    /// Native geometry belongs to the current immutable literal source leaf.
    fn current_native_leaf(
        &self,
        row: &Row,
        leaf: &editor_ui::markdown_renderer::LiteralLeaf,
    ) -> bool {
        let Row::PlainBlock { id, section, block } = row.semantic() else {
            return false;
        };
        self.view.as_ref().and_then(|view| view.item(id))
            .and_then(|item| section.text(item)).is_some_and(|text|
                matches!(text, artisan_api::TextContent::Stored(reference) if reference == &leaf.source_ref))
            && self.shared.stored_sources.borrow().get(&(id.clone(), section.slot())).is_some_and(|source|
                source.source_ref() == &leaf.source_ref && source.leaf_descriptor(*block).is_some_and(|descriptor|
                    descriptor.source_start == leaf.source_range.start as u64
                        && descriptor.source_end == leaf.source_range.end as u64))
    }

    pub(crate) fn native_body_ready(
        &mut self,
        row: &Row,
        leaf: &editor_ui::markdown_renderer::LiteralLeaf,
        ready: editor_ui::selectable_text::NativeRowReady,
        painted: bool,
        cx: &mut Context<Self>,
    ) {
        if !self.current_native_leaf(row, leaf) || ready.row_count == 0 {
            return;
        }
        let Row::PlainBlock { id, section, block } = row.semantic() else {
            return;
        };
        let selections = self.shared.literal_sources.borrow();
        let Some(source) = selections.get(&(id.clone(), section.slot(), *block)) else {
            return;
        };
        let native_key: SharedString = format!("{id}:{}:leaf:{block}", section.slot()).into();
        // A deferred old-width/style frame must not replace the currently owned
        // native layout generation after resize or a source presentation change.
        if self.shared.markdown.native_row_count(
            &native_key,
            &editor_ui::markdown_renderer::NativeInput::Inline(source.visible.clone()),
            ready.font_size,
            ready.width,
            &ready.page.runs,
        ) != Some(ready.row_count)
        {
            return;
        }
        drop(selections);
        let key = row.semantic().key();
        let mut windows = self.shared.native_windows.borrow_mut();
        let first = !windows.contains_key(&key);
        let state = windows.entry(key).or_insert_with(|| row::NativeBodyWindow {
            reference: leaf.source_ref.clone(),
            range: leaf.source_range.clone(),
            width: ready.width,
            font_size: ready.font_size,
            runs: ready.page.runs.clone(),
            count: ready.row_count,
            window: ready.row_count.saturating_sub(row::NATIVE_BODY_WINDOW)..ready.row_count,
            frontier: None,
            painted: Default::default(),
            error: None,
        });
        let changed = first
            || state.reference != leaf.source_ref
            || state.range != leaf.source_range
            || state.width != ready.width
            || state.font_size != ready.font_size
            || !Arc::ptr_eq(&state.runs, &ready.page.runs)
            || state.count != ready.row_count;
        if changed {
            *state = row::NativeBodyWindow {
                reference: leaf.source_ref.clone(),
                range: leaf.source_range.clone(),
                width: ready.width,
                font_size: ready.font_size,
                runs: ready.page.runs.clone(),
                count: ready.row_count,
                window: ready.row_count.saturating_sub(row::NATIVE_BODY_WINDOW)..ready.row_count,
                frontier: None,
                painted: Default::default(),
                error: None,
            };
        }
        state.error = None;
        let newly_painted =
            painted && state.window.contains(&ready.ordinal) && state.painted.insert(ready.ordinal);
        drop(windows);
        // Geometry discovery/resize expands only the current window; already
        // registered rows keep native RowKeys and ListState's existing anchor.
        let registered = self
            .rows
            .iter()
            .filter(|(candidate, _)| candidate.semantic().key() == row.key())
            .count();
        let desired = self
            .shared
            .native_windows
            .borrow()
            .get(&row.key())
            .unwrap()
            .window
            .len();
        if changed || registered != desired {
            self.rebuild(cx);
        } else if newly_painted {
            if let Row::PlainBlock { id, section, block } = row.semantic() {
                self.source_leaf_ready(id, section.slot(), *block, cx);
            }
        }
    }

    pub(crate) fn native_body_failed(
        &mut self,
        row: &Row,
        leaf: &editor_ui::markdown_renderer::LiteralLeaf,
        error: String,
        cx: &mut Context<Self>,
    ) {
        if !self.current_native_leaf(row, leaf) {
            return;
        }
        let mut windows = self.shared.native_windows.borrow_mut();
        if let Some(state) = windows.get_mut(&row.key()) {
            if state.error.as_ref() == Some(&error) {
                return;
            }
            state.error = Some(error);
            state.painted.clear();
        } else {
            // No geometry has been published yet. Preserve an explicit failure
            // on the original semantic row, with zero painted evidence.
            windows.insert(
                row.key(),
                row::NativeBodyWindow {
                    reference: leaf.source_ref.clone(),
                    range: leaf.source_range.clone(),
                    width: px(0.0),
                    font_size: px(0.0),
                    runs: Arc::from([]),
                    count: 1,
                    window: 0..1,
                    frontier: None,
                    painted: Default::default(),
                    error: Some(error),
                },
            );
        }
        drop(windows);
        if let Row::PlainBlock { id, section, block } = row.semantic() {
            self.source_leaf_ready(id, section.slot(), *block, cx);
        }
    }

    pub(crate) fn prune_literal_sources(&self) {
        let live: HashSet<_> = self
            .row_cache_order
            .iter()
            .filter_map(|index| {
                self.rows
                    .get(*index)
                    .and_then(|(row, _)| match row.semantic() {
                        Row::PlainBlock { id, section, block } => {
                            Some((id.clone(), section.slot(), *block))
                        }
                        _ => None,
                    })
            })
            .collect();
        self.shared
            .literal_sources
            .borrow_mut()
            .retain(|key, _| live.contains(key));
    }

    pub(crate) fn retry_native_body(&mut self, row: &Row, cx: &mut Context<Self>) {
        if let Some(state) = self.shared.native_windows.borrow_mut().get_mut(&row.key()) {
            state.error = None;
        }
        if let Row::PlainBlock { id, section, block } = row.semantic() {
            self.source_leaf_ready(id, section.slot(), *block, cx);
        }
    }

    pub(crate) fn native_window_edge(&mut self, key: RowKey, start: usize, cx: &mut Context<Self>) {
        let mut windows = self.shared.native_windows.borrow_mut();
        let Some(state) = windows.get_mut(&key) else {
            return;
        };
        let start = start.min(state.count.saturating_sub(1));
        let window = start
            ..start
                .saturating_add(row::NATIVE_BODY_WINDOW)
                .min(state.count);
        if state.window == window {
            return;
        }
        state.window = window;
        state
            .painted
            .retain(|ordinal| state.window.contains(ordinal));
        drop(windows);
        self.rebuild(cx);
    }

    pub(crate) fn source_leaf_ready(
        &mut self,
        id: &ItemId,
        slot: &str,
        block: usize,
        cx: &mut Context<Self>,
    ) {
        for index in self.row_context.item_rows.get(id).into_iter().flatten() {
            // One completed leaf changes only its own geometry. Invalidating
            // every sibling makes source preparation repeatedly churn the
            // bounded native row cache while measuring a long message.
            let current = match self.rows[*index].0.semantic() {
                Row::ProseBlock { block: ordinal, .. } => slot == "prose" && *ordinal == block,
                Row::TextBlock {
                    section,
                    block: ordinal,
                    ..
                } => section.slot() == slot && *ordinal == block,
                Row::PlainBlock {
                    section,
                    block: ordinal,
                    ..
                } => section.slot() == slot && *ordinal == block,
                _ => false,
            };
            if !current {
                continue;
            }
            if let Some(row) = self.row_views.get(*index).and_then(Option::as_ref) {
                row.update(cx, |_, cx| cx.notify());
                self.list.remeasure_items(*index..*index + 1);
            }
        }
        cx.notify();
    }

    pub(crate) fn source_window_edge(
        &mut self,
        id: ItemId,
        slot: &'static str,
        revision: u64,
        offset: usize,
        cx: &mut Context<Self>,
    ) {
        let Some(source) = self
            .shared
            .stored_sources
            .borrow()
            .get(&(id.clone(), slot))
            .cloned()
        else {
            return;
        };
        let Some(item) = self.view.as_ref().and_then(|view| view.item(&id)) else {
            return;
        };
        let current = if slot == "prose" {
            match &item.kind {
                ItemKind::AssistantMessage { text, .. } => Some(text),
                _ => None,
            }
        } else {
            [
                fold::TextSection::Proposal,
                fold::TextSection::AgentResult,
                fold::TextSection::Message,
            ]
            .into_iter()
            .find(|section| section.slot() == slot)
            .and_then(|section| section.text(item))
            .or_else(|| {
                [
                    fold::PlainSection::User,
                    fold::PlainSection::AgentTask,
                    fold::PlainSection::AgentMessage,
                    fold::PlainSection::Error,
                    fold::PlainSection::ImageCaption,
                ]
                .into_iter()
                .find(|section| section.slot() == slot)
                .and_then(|section| section.text(item))
            })
        };
        if source.revision() != revision
            || !current.is_some_and(|text| {
                matches!(text,
                artisan_api::TextContent::Stored(reference) if reference == source.source_ref())
            })
        {
            return;
        }
        let key: SharedString = format!("{id}:{slot}:index").into();
        let owner = cx.entity().downgrade();
        if source.loaded_range().start == offset {
            return;
        }
        let notify_id = id.clone();
        let result = self.shared.markdown.request_stored_window(
            key,
            &source,
            offset as u64,
            cx,
            move |cx| {
                let _ = owner.update(cx, |this, cx| {
                    this.source_window_edge(notify_id.clone(), slot, revision, offset, cx);
                });
            },
        );
        if let editor_ui::markdown_renderer::StoredPreparation::Ready(source) = result {
            self.shared
                .stored_sources
                .borrow_mut()
                .insert((id, slot), source);
            self.rebuild(cx);
        }
    }

    /// Arm arrivals only for source events observed on a visible native Item.
    /// Source-index pagination, first loading and scroll remounts never call this.
    fn observe_live_prose_arrival(&self, id: &ItemId) {
        let top = self.list.logical_scroll_top().item_ix;
        let visible = self.row_context.item_rows.get(id).is_some_and(|indices| {
            indices.iter().any(|index| {
                *index >= top && self.list.item_is_below_viewport(*index) == Some(false)
            })
        });
        if !visible {
            return;
        }
        let blocks = self
            .shared
            .prose_shapes
            .borrow()
            .get(id)
            .map_or(0, |(blocks, _)| *blocks);
        let mut arrivals = self.shared.prose_arrivals.borrow_mut();
        if !arrivals.contains_key(id) && arrivals.len() >= 128 {
            if let Some(oldest) = arrivals
                .iter()
                .min_by_key(|(_, arrival)| arrival.seen)
                .map(|(id, _)| id.clone())
            {
                arrivals.remove(&oldest);
            }
        }
        arrivals
            .entry(id.clone())
            .or_insert_with(|| motion::LiveBlockArrivals::baseline(blocks))
            .observed();
    }

    /// Reflow only when preparation changes the native leaf count.
    pub(crate) fn prose_ready(&mut self, id: &ItemId, cx: &mut Context<Self>) {
        if self.view.as_ref().and_then(|view| view.item(id)).is_none() {
            return;
        }
        let Some(prepared) = self.shared.markdown.prepared_source(&id.to_string().into()) else {
            return;
        };
        let blocks = prepared.block_count().max(1);
        let key: SharedString = id.to_string().into();
        let current = self.view.as_ref().and_then(|view| {
            view.live.get(&artisan_api::LiveTarget::Text {
                item_id: id.clone(),
            })
        });
        let caught_up =
            current.is_none_or(|rope| self.shared.markdown.is_rope_source_prepared(&key, rope));
        let initially_fresh = self
            .view
            .as_ref()
            .and_then(|view| view.ui.get(id))
            .is_some_and(thread::ItemUi::fresh);
        {
            let mut arrivals = self.shared.prose_arrivals.borrow_mut();
            if arrivals.contains_key(id) || arrivals.len() < 128 {
                arrivals
                    .entry(id.clone())
                    .or_insert_with(|| motion::LiveBlockArrivals::baseline(0))
                    .prepared(blocks, caught_up, initially_fresh);
                // Owners that were already mounted keep their own reveal clock.
                // Consume their token so evicting and remounting cannot replay it.
                if let Some(arrival) = arrivals.get_mut(id) {
                    for index in self.row_context.item_rows.get(id).into_iter().flatten() {
                        if self.row_views.get(*index).is_some_and(Option::is_some) {
                            match self.rows[*index].0.semantic() {
                                Row::Reply(_) => {
                                    arrival.claim(0);
                                }
                                Row::ProseBlock { block, .. } => {
                                    arrival.claim(*block);
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }
        }
        let indices = self.row_context.item_rows.get(id);
        let current_blocks = indices
            .into_iter()
            .flatten()
            .filter(|index| matches!(self.rows[**index].0, Row::ProseBlock { .. }))
            .count();
        if current_blocks != blocks {
            self.rebuild(cx);
            return;
        }
        self.shared
            .prose_shapes
            .borrow_mut()
            .insert(id.clone(), (blocks, prepared.revision()));
        for index in indices.into_iter().flatten() {
            if let Some(row) = self.row_views.get(*index).and_then(Option::as_ref) {
                row.update(cx, |_, cx| cx.notify());
                self.list.remeasure_items(*index..*index + 1);
            }
        }
        cx.notify();
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
        if let Some(view) = &self.view {
            let resource_owners: Vec<_> = view
                .items
                .values()
                .chain(view.context.values())
                .flat_map(|item| {
                    if self.shared.agent_scope.get() {
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
            self.shared
                .markdown
                .retain_resource_owners(&resource_owners, cx);
            self.shared
                .stored_sources
                .borrow_mut()
                .retain(|(id, _), _| view.item(id).is_some());
            self.shared
                .sources
                .borrow_mut()
                .retain(|(id, _), _| view.item(id).is_some());
            self.shared
                .prose_shapes
                .borrow_mut()
                .retain(|id, _| view.item(id).is_some());
            self.shared
                .prose_arrivals
                .borrow_mut()
                .retain(|id, _| view.item(id).is_some());
            self.shared
                .plain_sources
                .borrow_mut()
                .retain(|(id, _), _| view.item(id).is_some());
        }
        let rows = self
            .view
            .as_ref()
            .map(|view| fold::expanded(view, rows.clone(), &runs))
            .unwrap_or(rows);
        let rows: Vec<_> = rows
            .into_iter()
            .flat_map(|(row, version)| {
                let formatted = match &row {
                    Row::Reply(id) => Some((id, "prose", None)),
                    Row::TextBlock { id, section, .. } => Some((id, section.slot(), Some(*section))),
                    _ => None,
                };
                if let Some((id, slot, section)) = formatted {
                    if let Some(index) = self.shared.stored_sources.borrow().get(&(id.clone(), slot)) {
                        let current = self.view.as_ref().and_then(|view| view.item(id)).and_then(|item| {
                            if let Some(section) = section { section.text(item) } else {
                                match &item.kind { ItemKind::AssistantMessage { text, .. } => Some(text), _ => None }
                            }
                        });
                        if !(section.is_none() && self.view.as_ref().is_some_and(|view| view.item(id).is_some_and(|item| item.status == artisan_api::Status::Running) && view.live.contains_key(&artisan_api::LiveTarget::Text { item_id: id.clone() }))) && current.is_some_and(|text| matches!(text, artisan_api::TextContent::Stored(reference) if reference == index.source_ref())) {
                            let mut hasher = DefaultHasher::new();
                            version.hash(&mut hasher);
                            index.revision().hash(&mut hasher);
                            let version = hasher.finish();
                            // An empty semantic document still keeps its item wrapper.
                            let loaded = index.loaded_range();
                            if loaded.is_empty() { return vec![(row, version)]; }
                            return loaded.map(|block| {
                                let row = if let Some(section) = section {
                                    let Row::TextBlock { run, .. } = &row else { unreachable!() };
                                    Row::TextBlock { id: id.clone(), section, run: run.clone(), block }
                                } else { Row::ProseBlock { id: id.clone(), block } };
                                (row, version)
                            }).collect::<Vec<_>>();
                        }
                    }
                }
                let uses_stored = formatted.is_some_and(|(id, _, section)| self.view.as_ref().and_then(|view| view.item(id)).is_some_and(|item| {
                    let stored = if let Some(section) = section { section.text(item).is_some_and(|text| matches!(text, artisan_api::TextContent::Stored(_))) }
                        else { matches!(&item.kind, ItemKind::AssistantMessage { text: artisan_api::TextContent::Stored(_), .. }) };
                    stored && !(section.is_none() && item.status == artisan_api::Status::Running && self.view.as_ref().is_some_and(|view| view.live.contains_key(&artisan_api::LiveTarget::Text { item_id: id.clone() })))
                }));
                if uses_stored { return vec![(row, version)]; }
                if let Row::Reply(id) = &row {
                    if let Some(prepared) =
                        self.shared.markdown.prepared_source(&id.to_string().into())
                    {
                        self.shared.prose_shapes.borrow_mut().insert(
                            id.clone(),
                            (prepared.block_count().max(1), prepared.revision()),
                        );
                    }
                    if let Some((blocks, revision)) =
                        self.shared.prose_shapes.borrow().get(id).copied()
                    {
                        let mut hasher = DefaultHasher::new();
                        version.hash(&mut hasher);
                        revision.hash(&mut hasher);
                        let version = hasher.finish();
                        return (0..blocks)
                            .map(|block| {
                                (
                                    Row::ProseBlock {
                                        id: id.clone(),
                                        block,
                                    },
                                    version,
                                )
                            })
                            .collect::<Vec<_>>();
                    }
                }
                if let Row::TextBlock {
                    id, section, run, ..
                } = &row
                {
                    let key: gpui::SharedString = format!("{id}:{}", section.slot()).into();
                    if let Some(prepared) = self.shared.markdown.prepared_source(&key) {
                        let mut hasher = DefaultHasher::new();
                        version.hash(&mut hasher);
                        prepared.revision().hash(&mut hasher);
                        let version = hasher.finish();
                        return (0..prepared.block_count().max(1))
                            .map(|block| {
                                (
                                    Row::TextBlock {
                                        id: id.clone(),
                                        section: *section,
                                        run: run.clone(),
                                        block,
                                    },
                                    version,
                                )
                            })
                            .collect::<Vec<_>>();
                    }
                }
                if let Row::Image(id) = &row {
                    if let Some(item) = self.view.as_ref().and_then(|view| view.item(id)).filter(|item| matches!(&item.kind, ItemKind::Image { caption: Some(text), .. } if text.byte_len() > 0)) {
                        let blocks = self.shared.prepare_plain(item, fold::PlainSection::ImageCaption);
                        let mut rows = vec![(row.clone(), version)];
                        rows.extend((0..blocks).map(|block| (Row::PlainBlock { id: id.clone(), section: fold::PlainSection::ImageCaption, block }, version)));
                        return rows;
                    }
                }
                if let Row::Image(id) = &row {
                    if let Some(item) = self.view.as_ref().and_then(|view| view.item(id)) {
                        let section = fold::PlainSection::ImageCaption;
                        if let Some(text) = section.text(item) {
                            let mut result = vec![(row.clone(), version)];
                            let range = if let artisan_api::TextContent::Stored(reference) = text {
                                self.shared.stored_sources.borrow().get(&(id.clone(), section.slot()))
                                    .filter(|source| source.source_ref() == reference
                                        && source.format() == artisan_api::SourceIndexFormat::Literal
                                       )
                                    .map_or(0..1, |source| source.loaded_range())
                            } else { 0..self.shared.prepare_plain(item, section) };
                            result.extend(range.map(|block| (Row::PlainBlock { id: id.clone(), section, block }, version)));
                            return result;
                        }
                    }
                }
                let plain = match &row {
                    Row::User(id) => Some((id, fold::PlainSection::User)),
                    Row::AgentTask(id) => Some((id, fold::PlainSection::AgentTask)),
                    Row::AgentMessage(id) => Some((id, fold::PlainSection::AgentMessage)),
                    Row::Error(id) => Some((id, fold::PlainSection::Error)),
                    Row::WorkEntry { id, .. }
                        if self
                            .view
                            .as_ref()
                            .and_then(|view| view.item(id))
                            .is_some_and(|item| {
                                matches!(item.kind, ItemKind::UserMessage { .. })
                            }) =>
                    {
                        Some((id, fold::PlainSection::User))
                    }
                    _ => None,
                };
                if let Some((id, section)) = plain {
                    if let Some(item) = self.view.as_ref().and_then(|view| view.item(id)) {
                        if let Some(artisan_api::TextContent::Stored(reference)) = section.text(item) {
                            let sources = self.shared.stored_sources.borrow();
                            if let Some(source) = sources.get(&(id.clone(), section.slot()))
                                .filter(|source| source.source_ref() == reference
                                    && source.format() == artisan_api::SourceIndexFormat::Literal
                                   ) {
                                let mut hasher = DefaultHasher::new();
                                version.hash(&mut hasher); source.revision().hash(&mut hasher);
                                let version = hasher.finish();
                                return source.loaded_range().map(|block| (Row::PlainBlock {
                                    id: id.clone(), section, block,
                                }, version)).collect::<Vec<_>>();
                            }
                            return vec![(Row::PlainBlock { id: id.clone(), section, block: 0 }, version)];
                        }
                        let blocks = self.shared.prepare_plain(item, section);
                        return (0..blocks)
                            .map(|block| {
                                (
                                    Row::PlainBlock {
                                        id: id.clone(),
                                        section,
                                        block,
                                    },
                                    version,
                                )
                            })
                            .collect::<Vec<_>>();
                    }
                }
                vec![(row, version)]
            })
            .collect();
        let rows = self.retain_disclosure_rows(rows);
        // Mounted source owners retain raw allocation permits only while the
        // exact indexed page is resident. Descriptor count never allocates a
        // complete-source selection registry.
        self.shared
            .literal_sources
            .borrow_mut()
            .retain(|(id, slot, ordinal), entry| {
                self.shared
                    .stored_sources
                    .borrow()
                    .get(&(id.clone(), *slot))
                    .is_some_and(|source| {
                        source.source_ref() == &entry.leaf.source_ref
                            && source.loaded_range().contains(ordinal)
                    })
                    && self.view.as_ref().is_some_and(|view| {
                        view.item(id).is_some_and(|item| {
                            [
                                fold::PlainSection::User,
                                fold::PlainSection::AgentTask,
                                fold::PlainSection::AgentMessage,
                                fold::PlainSection::Error,
                                fold::PlainSection::ImageCaption,
                            ]
                            .into_iter()
                            .find(|section| section.slot() == *slot)
                            .and_then(|section| section.text(item))
                            .is_some_and(|text| {
                                matches!(text, artisan_api::TextContent::Stored(reference)
                        if reference == &entry.leaf.source_ref)
                            })
                        })
                    })
            });
        self.shared
            .source_frontiers
            .borrow_mut()
            .retain(|key, (reference, _, _)| {
                self.shared
                    .stored_sources
                    .borrow()
                    .get(key)
                    .is_some_and(|source| source.source_ref() == reference)
                    && self
                        .view
                        .as_ref()
                        .is_some_and(|view| view.item(&key.0).is_some())
            });
        self.shared
            .native_windows
            .borrow_mut()
            .retain(|key, state| {
                // First literal rows retain their original Item/AgentTask keys.
                // Resolve the current semantic row instead of assuming its key
                // encodes a PlainBlock variant.
                let Some((Row::PlainBlock { id, section, block }, _)) =
                    rows.iter().find(|(row, _)| row.key() == *key)
                else {
                    return false;
                };
                self.shared
                    .stored_sources
                    .borrow()
                    .get(&(id.clone(), section.slot()))
                    .is_some_and(|source| {
                        source.source_ref() == &state.reference
                            && source.leaf_descriptor(*block).is_some_and(|descriptor| {
                                descriptor.source_start == state.range.start as u64
                                    && descriptor.source_end == state.range.end as u64
                            })
                    })
            });
        let rows: Vec<_> = rows
            .into_iter()
            .flat_map(|(row, version)| {
                let windows = self.shared.native_windows.borrow();
                let Some(state) = windows.get(&row.key()) else {
                    return vec![(row, version)];
                };
                if state.count <= 1 {
                    return vec![(row, version)];
                }
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
                    .collect::<Vec<_>>()
            })
            .collect();
        self.index_disclosure_rows(&rows);
        let rows = self.admit_native_rows(rows, cx);
        let old_keys: Vec<RowKey> = self.rows.iter().map(|(row, _)| row.key()).collect();
        let new_keys: Vec<RowKey> = rows.iter().map(|(row, _)| row.key()).collect();
        splice(&self.list, &old_keys, &new_keys);
        let mut kept: HashMap<RowKey, Entity<RowView>> = old_keys
            .into_iter()
            .zip(self.row_views.drain(..))
            .filter_map(|(key, view)| Some((key, view?)))
            .collect();
        let agents = Arc::new(self.view.as_ref().map(agent_names).unwrap_or_default());
        let agents_version = {
            let mut sorted: Vec<_> = agents.iter().collect();
            sorted.sort();
            let mut hasher = DefaultHasher::new();
            sorted.hash(&mut hasher);
            hasher.finish()
        };
        let mut views = Vec::with_capacity(rows.len());
        let mut versions = Vec::with_capacity(rows.len());
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
            let mut image_order: Vec<_> = images.iter().collect();
            image_order.sort_by_key(|(hash, _)| *hash);
            for (hash, blob) in image_order {
                (hash, std::mem::discriminant(blob)).hash(&mut hasher);
            }
            let version = hasher.finish();
            versions.push(version);
            let model = |this: &Self| {
                this.model(row, summary.clone(), images.clone(), now, gap, end, &agents)
            };
            let view = if let Some(view) = kept.remove(&key) {
                if view.read(cx).version() != version {
                    let model = model(self);
                    view.update(cx, |view, cx| view.set(model, version, cx));
                    self.list.remeasure_items(index..index + 1);
                }
                Some(view)
            } else {
                None
            };
            views.push(view);
        }
        self.rows = rows;
        self.navigator_markers = None;
        self.row_views = views;
        self.row_cache_order = self
            .row_views
            .iter()
            .enumerate()
            .filter_map(|(index, view)| view.as_ref().map(|_| index))
            .collect();
        let read_boundary = self.view.as_ref().and_then(|view| {
            if self.agent.is_some() || view.window().gap.is_some() {
                return None;
            }
            let latest = view.shown().max_by_key(|item| item.seq)?;
            let run = run.as_ref()?;
            (!run.status.is_live()
                && latest.run_id.as_ref() == Some(&run.id)
                && run.seq >= latest.seq)
                .then(|| (latest.id.clone(), run.seq))
        });
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
        self.row_context = RowContext {
            key_rows,
            item_rows,
            status_rows,
            read_boundary,
            runs,
            agents,
            gaps,
            versions,
            now,
            end_space,
        };
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
        cx.notify();
    }

    /// A snapshot of what one row shows.
    #[allow(clippy::too_many_arguments)]
    fn model(
        &self,
        row: &Row,
        run: Option<fold::RunSummary>,
        images: HashMap<BlobHash, Blob>,
        now: artisan_api::Timestamp,
        gap: f32,
        end: f32,
        agents: &Arc<HashMap<String, String>>,
    ) -> RowModel {
        let mut model = RowModel {
            agents: agents.clone(),
            row: row.clone(),
            items: HashMap::new(),
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

        let mut stack: Vec<ItemId> = match row {
            Row::Work { compaction, .. } => compaction.iter().cloned().collect(),
            Row::Chain { ids, .. } => ids.clone(),
            other => other.anchor().cloned().into_iter().collect(),
        };
        // A subagent's task and report read the agent alone.
        for id in stack.drain(..) {
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
                | artisan_api::LiveTarget::Status { run_id } => row.run() == Some(run_id),
            })
            .map(|(target, text)| (target.clone(), text.clone()))
            .collect();

        model
    }

    /// The cached images a row shows: its anchor's,
    /// and in a work section every item's (a steer sent with images sits
    /// there, not on a row of its own).
    fn row_images(&mut self, row: &Row, cx: &mut Context<Self>) -> HashMap<BlobHash, Blob> {
        let ids: Vec<ItemId> = match row {
            Row::Work { .. } | Row::Chain { .. } => Vec::new(),
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
                    _ => None,
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
    fn sync_questionnaires(&mut self, cx: &mut Context<Self>) {
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

    /// The thread's goal, as its latest goal notice left it, on the
    /// composer's strip.
    fn sync_goal(&mut self, cx: &mut Context<Self>) {
        let Some(composer) = self.thread_composer(cx) else {
            return;
        };
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
            },
            detail: None,
        });
        composer.update(cx, |composer, cx| composer.set_goal(goal, cx));
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

    fn retry_image(&mut self, hash: &BlobHash, cx: &mut Context<Self>) {
        let Some(host) = self.host(cx) else {
            return;
        };
        host.update(cx, |host, cx| host.load_image(hash.clone(), true, cx));
        self.rebuild(cx);
    }

    fn load_image(&mut self, hash: &BlobHash, cx: &mut Context<Self>) {
        let Some(host) = self.host(cx) else {
            return;
        };
        host.update(cx, |host, cx| host.load_image(hash.clone(), false, cx));
    }
}

/// The thread's agents by their canonical recipient address: their
/// names, for "Talked to …".
fn agent_names(view: &ThreadView) -> HashMap<String, String> {
    view.items
        .values()
        .chain(view.context.values())
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
